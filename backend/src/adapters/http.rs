//! Axum/Tower Stage 11 HTTP and durable WebSocket adapter.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, HeaderName, HeaderValue, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Router, http};
use serde::Deserialize;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::adapters::sqlite::SqliteStateStore;
use crate::adapters::system_clock::SystemClock;
use crate::application::authentication::DeviceAuthenticator;
use crate::application::command_gateway::{
    CommandGateway, CommandGatewayError, CommandGatewayErrorKind,
};
use crate::application::command_service::CommandServiceErrorKind;
use crate::application::publication::{
    PublicStateService, PublicationError, PublicationErrorKind, encode_public_event_frame,
};
use crate::application::runtime::ControlledShutdown;
use crate::application::scheduler::SchedulerNotifier;
use crate::application::transport::{CommandCommitEffects, CursorBroadcaster, MutationAdmission};
use crate::bootstrap::health::{FatalReasonCode, Health, HealthState};
use crate::domain::{AuthenticatedDevice, ConversationId, IdempotencyKey, UtcTimestamp, WorkId};
use crate::ports::clock::Clock;
use crate::protocol::{
    BootstrapResponse, CANCELLATION_BODY_LIMIT, CancellationRequest, CancellationResponse,
    ErrorEnvelope, HTTP_CONCURRENCY_LIMIT, HealthResponse, HealthStatus, MESSAGE_BODY_LIMIT,
    MUTATION_CONCURRENCY_LIMIT, MessageRequest, MessageResponse, ProtocolVersion, ReplayCursor,
    RequestId, SyncCompleteEnvelope, WEBSOCKET_CONNECTION_LIMIT, WEBSOCKET_OUTBOUND_FRAMES,
};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);
const REPLAY_PAGE_TIMEOUT: Duration = Duration::from_secs(5);
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const WS_FALLBACK_SCAN: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct HttpState {
    store: Arc<SqliteStateStore>,
    clock: Arc<SystemClock>,
    health: Health,
    command_gateway: Arc<CommandGateway<SqliteStateStore, SystemClock, CommandCommitEffects>>,
    cursors: CursorBroadcaster,
    ws_limit: Arc<tokio::sync::Semaphore>,
    ws_shutdown: tokio::sync::watch::Sender<bool>,
    connections: ConnectionRegistry,
    fatal: tokio::sync::watch::Sender<bool>,
    mutation_admission: MutationAdmission,
    controlled_shutdown: Option<Arc<dyn ControlledShutdown>>,
    allowed_hosts: Arc<Vec<String>>,
    #[cfg(test)]
    ws_send_stall: Option<Arc<AtomicBool>>,
    #[cfg(test)]
    upgrade_gate: Option<Arc<TestUpgradeGate>>,
    #[cfg(test)]
    post_commit_gate: Option<Arc<TestPostCommitGate>>,
}

impl HttpState {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        store: Arc<SqliteStateStore>,
        clock: Arc<SystemClock>,
        health: Health,
        admission: MutationAdmission,
        cursors: CursorBroadcaster,
        fatal: tokio::sync::watch::Sender<bool>,
        ws_shutdown: tokio::sync::watch::Sender<bool>,
        connections: ConnectionRegistry,
        allowed_hosts: Vec<String>,
        controlled_shutdown: Option<Arc<dyn ControlledShutdown>>,
        scheduler_notifier: Option<SchedulerNotifier>,
    ) -> Self {
        let effects = CommandCommitEffects::new(cursors.clone(), scheduler_notifier);
        let command_gateway = Arc::new(CommandGateway::new(
            Arc::clone(&store),
            Arc::clone(&clock),
            health.clone(),
            admission.clone(),
            effects,
        ));
        Self {
            store,
            clock,
            health,
            command_gateway,
            cursors,
            ws_limit: Arc::new(tokio::sync::Semaphore::new(WEBSOCKET_CONNECTION_LIMIT)),
            ws_shutdown,
            connections,
            fatal,
            mutation_admission: admission,
            controlled_shutdown,
            allowed_hosts: Arc::new(allowed_hosts),
            #[cfg(test)]
            ws_send_stall: None,
            #[cfg(test)]
            upgrade_gate: None,
            #[cfg(test)]
            post_commit_gate: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_ws_send_stall(mut self, stall: Arc<AtomicBool>) -> Self {
        self.ws_send_stall = Some(stall);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_test_upgrade_gate(mut self, gate: Arc<TestUpgradeGate>) -> Self {
        self.upgrade_gate = Some(gate);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_test_post_commit_gate(mut self, gate: Arc<TestPostCommitGate>) -> Self {
        self.post_commit_gate = Some(gate);
        self
    }

    fn fatal_protocol(&self) {
        let _ = self.health.mark_fatal(FatalReasonCode::Internal);
        let _ = self.fatal.send(true);
    }

    async fn handle_shared_server_failure(&self) {
        let _ = self.health.mark_fatal(FatalReasonCode::Internal);
        let _ = self.fatal.send(true);
        self.mutation_admission.close_and_wait().await;
        if let Some(shutdown) = &self.controlled_shutdown {
            let _ = shutdown.request_controlled_shutdown().await;
        }
        self.ws_shutdown.send_replace(true);
    }

    fn shutdown_is_requested(&self) -> bool {
        self.controlled_shutdown
            .as_ref()
            .is_some_and(|shutdown| shutdown.shutdown_is_requested())
    }
}

#[cfg(test)]
pub(crate) struct TestUpgradeGate {
    held: AtomicBool,
    hold_cancellation: AtomicBool,
    panic_next: AtomicBool,
    entered: AtomicUsize,
    changed: tokio::sync::Notify,
    cancellation_seen: tokio::sync::Semaphore,
    cancellation_released: tokio::sync::Semaphore,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestPostCommitGate {
    armed: AtomicBool,
    reached: tokio::sync::Semaphore,
    released: tokio::sync::Semaphore,
}

#[cfg(test)]
impl TestPostCommitGate {
    pub(crate) fn armed() -> Self {
        Self {
            armed: AtomicBool::new(true),
            reached: tokio::sync::Semaphore::new(0),
            released: tokio::sync::Semaphore::new(0),
        }
    }

    async fn hold_once(&self) {
        if !self.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        self.reached.add_permits(1);
        self.released
            .acquire()
            .await
            .expect("postcommit test gate must remain open")
            .forget();
    }

    pub(crate) async fn wait_until_reached(&self) {
        self.reached
            .acquire()
            .await
            .expect("postcommit test gate must remain open")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.released.add_permits(1);
    }
}

#[cfg(test)]
impl TestUpgradeGate {
    pub(crate) fn held() -> Self {
        Self {
            held: AtomicBool::new(true),
            hold_cancellation: AtomicBool::new(false),
            panic_next: AtomicBool::new(false),
            entered: AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
            cancellation_seen: tokio::sync::Semaphore::new(0),
            cancellation_released: tokio::sync::Semaphore::new(0),
        }
    }

    pub(crate) fn held_after_cancellation() -> Self {
        Self {
            held: AtomicBool::new(true),
            hold_cancellation: AtomicBool::new(true),
            panic_next: AtomicBool::new(false),
            entered: AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
            cancellation_seen: tokio::sync::Semaphore::new(0),
            cancellation_released: tokio::sync::Semaphore::new(0),
        }
    }

    pub(crate) fn panic_once() -> Self {
        Self {
            held: AtomicBool::new(false),
            hold_cancellation: AtomicBool::new(false),
            panic_next: AtomicBool::new(true),
            entered: AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
            cancellation_seen: tokio::sync::Semaphore::new(0),
            cancellation_released: tokio::sync::Semaphore::new(0),
        }
    }

    async fn enter(&self, cancellation: &mut tokio::sync::watch::Receiver<bool>) -> bool {
        self.entered.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
        while self.held.load(Ordering::Acquire) {
            let changed = self.changed.notified();
            if !self.held.load(Ordering::Acquire) {
                break;
            }
            tokio::select! {
                biased;
                result = cancellation.changed() => {
                    if result.is_err() || *cancellation.borrow() {
                        self.cancellation_seen.add_permits(1);
                        if self.hold_cancellation.load(Ordering::Acquire) {
                            self.cancellation_released
                                .acquire()
                                .await
                                .expect("cancellation test gate must remain open")
                                .forget();
                        }
                        return false;
                    }
                }
                () = changed => {}
            }
        }
        if *cancellation.borrow() {
            return false;
        }
        assert!(
            !self.panic_next.swap(false, Ordering::AcqRel),
            "injected WebSocket upgrade callback panic"
        );
        true
    }

    pub(crate) async fn wait_for_entries(&self, expected: usize) {
        loop {
            let changed = self.changed.notified();
            if self.entered.load(Ordering::Acquire) >= expected {
                return;
            }
            changed.await;
        }
    }

    pub(crate) fn release(&self) {
        self.held.store(false, Ordering::Release);
        self.changed.notify_waiters();
    }

    pub(crate) async fn wait_for_cancellation(&self) {
        self.cancellation_seen
            .acquire()
            .await
            .expect("cancellation test gate must remain open")
            .forget();
    }

    pub(crate) fn release_cancellation(&self) {
        self.cancellation_released.add_permits(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionOwnershipState {
    PendingUpgrade,
    Active,
    Closing,
    Finished,
}

struct ConnectionOwnership {
    state: ConnectionOwnershipState,
    callback_observed: bool,
    cancellation: tokio::sync::watch::Sender<bool>,
}

enum UpgradeCallbackOutcome {
    Returned,
    Panicked,
}

enum UpgradeTerminalOutcome {
    Failed,
    Cancelled,
    Panicked,
    CallbackCancelled,
    CallbackPanicked,
}

struct ConnectionActivation {
    id: u64,
    socket: WebSocket,
    hints: tokio::sync::broadcast::Receiver<ReplayCursor>,
    after: ReplayCursor,
    replay_high_water: ReplayCursor,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

enum ConnectionEvent {
    Activate(Box<ConnectionActivation>),
    CallbackCompleted {
        id: u64,
        outcome: UpgradeCallbackOutcome,
    },
    UpgradeCompleted {
        id: u64,
        outcome: UpgradeTerminalOutcome,
    },
    Stop {
        abort: bool,
    },
}

struct ConnectionRegistryInner {
    next_id: AtomicU64,
    accepting: AtomicBool,
    entries: std::sync::Mutex<BTreeMap<u64, ConnectionOwnership>>,
    changed: tokio::sync::Notify,
    events: tokio::sync::mpsc::UnboundedSender<ConnectionEvent>,
    receiver: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ConnectionEvent>>>,
    observed_callbacks: AtomicUsize,
    observed_panics: AtomicUsize,
    observed_completions: AtomicUsize,
}

#[derive(Clone)]
pub struct ConnectionRegistry {
    inner: Arc<ConnectionRegistryInner>,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            inner: Arc::new(ConnectionRegistryInner {
                next_id: AtomicU64::new(1),
                accepting: AtomicBool::new(true),
                entries: std::sync::Mutex::new(BTreeMap::new()),
                changed: tokio::sync::Notify::new(),
                events,
                receiver: std::sync::Mutex::new(Some(receiver)),
                observed_callbacks: AtomicUsize::new(0),
                observed_panics: AtomicUsize::new(0),
                observed_completions: AtomicUsize::new(0),
            }),
        }
    }
}

impl ConnectionRegistry {
    fn reserve(
        &self,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<PendingUpgrade, tokio::sync::OwnedSemaphorePermit> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(permit);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::AcqRel);
        let (cancellation, cancellation_receiver) = tokio::sync::watch::channel(false);
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(permit);
        }
        let previous = entries.insert(
            id,
            ConnectionOwnership {
                state: ConnectionOwnershipState::PendingUpgrade,
                callback_observed: false,
                cancellation,
            },
        );
        debug_assert!(previous.is_none());
        drop(entries);
        self.inner.changed.notify_waiters();
        Ok(PendingUpgrade {
            inner: Arc::new(PendingUpgradeInner {
                id,
                stage: AtomicU8::new(PENDING_UPGRADE),
                permit: std::sync::Mutex::new(Some(permit)),
                events: self.inner.events.clone(),
                cancellation: cancellation_receiver,
            }),
        })
    }

    fn take_receiver(&self) -> tokio::sync::mpsc::UnboundedReceiver<ConnectionEvent> {
        self.inner
            .receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("connection supervisor receiver must be owned exactly once")
    }

    fn stop_accepting(&self) {
        self.inner.accepting.store(false, Ordering::Release);
    }

    fn mark_active(&self, id: u64) -> bool {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = entries.get_mut(&id) else {
            return false;
        };
        if entry.state != ConnectionOwnershipState::PendingUpgrade {
            return false;
        }
        entry.state = ConnectionOwnershipState::Active;
        self.inner.changed.notify_waiters();
        true
    }

    fn observe_callback(&self, id: u64, outcome: UpgradeCallbackOutcome) -> bool {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = entries.get_mut(&id) else {
            return false;
        };
        if entry.callback_observed || entry.state == ConnectionOwnershipState::PendingUpgrade {
            return false;
        }
        entry.callback_observed = true;
        self.inner.observed_callbacks.fetch_add(1, Ordering::AcqRel);
        if matches!(outcome, UpgradeCallbackOutcome::Panicked) {
            self.inner.observed_panics.fetch_add(1, Ordering::AcqRel);
        }
        let remove = entry.state == ConnectionOwnershipState::Finished;
        if remove {
            entries.remove(&id);
        }
        drop(entries);
        self.inner.changed.notify_waiters();
        true
    }

    fn observe_upgrade_terminal(&self, id: u64, outcome: UpgradeTerminalOutcome) -> bool {
        let removed = {
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !entries
                .get(&id)
                .is_some_and(|entry| entry.state == ConnectionOwnershipState::PendingUpgrade)
            {
                return false;
            }
            entries.remove(&id).is_some()
        };
        debug_assert!(removed);
        if matches!(
            outcome,
            UpgradeTerminalOutcome::Panicked | UpgradeTerminalOutcome::CallbackPanicked
        ) {
            self.inner.observed_panics.fetch_add(1, Ordering::AcqRel);
        }
        if matches!(
            outcome,
            UpgradeTerminalOutcome::CallbackCancelled | UpgradeTerminalOutcome::CallbackPanicked
        ) {
            self.inner.observed_callbacks.fetch_add(1, Ordering::AcqRel);
        }
        self.record_consumed_completion();
        self.inner.changed.notify_waiters();
        true
    }

    fn observe_connection_completion(&self, id: u64, panicked: bool) -> bool {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = entries.get_mut(&id) else {
            return false;
        };
        if !matches!(
            entry.state,
            ConnectionOwnershipState::Active | ConnectionOwnershipState::Closing
        ) {
            return false;
        }
        entry.state = ConnectionOwnershipState::Finished;
        let remove = entry.callback_observed;
        if remove {
            entries.remove(&id);
        }
        drop(entries);
        if panicked {
            self.inner.observed_panics.fetch_add(1, Ordering::AcqRel);
        }
        self.record_consumed_completion();
        self.inner.changed.notify_waiters();
        true
    }

    fn begin_closing(&self) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries.values_mut() {
            if entry.state == ConnectionOwnershipState::Active {
                entry.state = ConnectionOwnershipState::Closing;
            }
        }
        drop(entries);
        self.inner.changed.notify_waiters();
    }

    fn cancel_owned(&self) {
        let entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries.values() {
            entry.cancellation.send_replace(true);
        }
    }

    fn record_consumed_completion(&self) {
        let prior = self
            .inner
            .observed_completions
            .fetch_add(1, Ordering::AcqRel);
        debug_assert!(prior < self.inner.next_id.load(Ordering::Acquire) as usize - 1);
    }

    pub async fn wait_empty(&self) {
        loop {
            let changed = self.inner.changed.notified();
            if self.owned() == 0 {
                return;
            }
            changed.await;
        }
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    ConnectionOwnershipState::Active | ConnectionOwnershipState::Closing
                )
            })
            .count()
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|entry| entry.state == ConnectionOwnershipState::PendingUpgrade)
            .count()
    }

    #[must_use]
    pub fn owned(&self) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    #[cfg(test)]
    pub(crate) fn observed_callbacks(&self) -> usize {
        self.inner.observed_callbacks.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn observed_panics(&self) -> usize {
        self.inner.observed_panics.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn observed_completions(&self) -> usize {
        self.inner.observed_completions.load(Ordering::Acquire)
    }
}

const PENDING_UPGRADE: u8 = 0;
const ACTIVATED_UPGRADE: u8 = 1;
const TERMINAL_UPGRADE: u8 = 2;

#[derive(Clone)]
struct PendingUpgrade {
    inner: Arc<PendingUpgradeInner>,
}

struct PendingUpgradeInner {
    id: u64,
    stage: AtomicU8,
    permit: std::sync::Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    events: tokio::sync::mpsc::UnboundedSender<ConnectionEvent>,
    cancellation: tokio::sync::watch::Receiver<bool>,
}

impl PendingUpgrade {
    fn cancellation(&self) -> tokio::sync::watch::Receiver<bool> {
        self.inner.cancellation.clone()
    }

    fn upgrade_failed(&self) {
        if self
            .inner
            .stage
            .compare_exchange(
                PENDING_UPGRADE,
                TERMINAL_UPGRADE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.inner
                .permit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let _ = self.inner.events.send(ConnectionEvent::UpgradeCompleted {
                id: self.inner.id,
                outcome: UpgradeTerminalOutcome::Failed,
            });
        }
    }

    fn activate(
        &self,
        socket: WebSocket,
        hints: tokio::sync::broadcast::Receiver<ReplayCursor>,
        after: ReplayCursor,
        replay_high_water: ReplayCursor,
    ) {
        if self
            .inner
            .stage
            .compare_exchange(
                PENDING_UPGRADE,
                ACTIVATED_UPGRADE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        let permit = self
            .inner
            .permit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("pending upgrade must retain its connection permit");
        let _ = self
            .inner
            .events
            .send(ConnectionEvent::Activate(Box::new(ConnectionActivation {
                id: self.inner.id,
                socket,
                hints,
                after,
                replay_high_water,
                _permit: permit,
            })));
    }

    fn finish_callback(&self, panicked: bool) {
        if self
            .inner
            .stage
            .compare_exchange(
                PENDING_UPGRADE,
                TERMINAL_UPGRADE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.inner
                .permit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let outcome = if panicked {
                UpgradeTerminalOutcome::CallbackPanicked
            } else {
                UpgradeTerminalOutcome::CallbackCancelled
            };
            let _ = self.inner.events.send(ConnectionEvent::UpgradeCompleted {
                id: self.inner.id,
                outcome,
            });
        } else if self.inner.stage.load(Ordering::Acquire) == ACTIVATED_UPGRADE {
            let outcome = if panicked {
                UpgradeCallbackOutcome::Panicked
            } else {
                UpgradeCallbackOutcome::Returned
            };
            let _ = self.inner.events.send(ConnectionEvent::CallbackCompleted {
                id: self.inner.id,
                outcome,
            });
        }
    }
}

impl Drop for PendingUpgradeInner {
    fn drop(&mut self) {
        if self
            .stage
            .compare_exchange(
                PENDING_UPGRADE,
                TERMINAL_UPGRADE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.permit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let outcome = if std::thread::panicking() {
                UpgradeTerminalOutcome::Panicked
            } else {
                UpgradeTerminalOutcome::Cancelled
            };
            let _ = self.events.send(ConnectionEvent::UpgradeCompleted {
                id: self.id,
                outcome,
            });
        }
    }
}

struct UpgradeCallbackGuard {
    upgrade: PendingUpgrade,
}

impl UpgradeCallbackGuard {
    fn new(upgrade: &PendingUpgrade) -> Self {
        Self {
            upgrade: upgrade.clone(),
        }
    }
}

impl Drop for UpgradeCallbackGuard {
    fn drop(&mut self) {
        self.upgrade.finish_callback(std::thread::panicking());
    }
}

async fn supervise_connections(
    state: HttpState,
    connections: ConnectionRegistry,
    mut events: tokio::sync::mpsc::UnboundedReceiver<ConnectionEvent>,
) {
    let mut tasks = tokio::task::JoinSet::<u64>::new();
    let mut task_connections = HashMap::new();
    let mut stopping = false;
    loop {
        if stopping && tasks.is_empty() && connections.owned() == 0 {
            return;
        }
        tokio::select! {
            Some(event) = events.recv() => {
                match event {
                    ConnectionEvent::Activate(activation) => {
                        if connections.mark_active(activation.id) {
                            let connection_state = state.clone();
                            let id = activation.id;
                            let task = tasks.spawn(async move {
                                run_websocket(
                                    activation.socket,
                                    connection_state,
                                    activation.hints,
                                    activation.after,
                                    activation.replay_high_water,
                                ).await;
                                id
                            });
                            task_connections.insert(task.id(), id);
                        }
                    }
                    ConnectionEvent::CallbackCompleted { id, outcome } => {
                        if !connections.observe_callback(id, outcome) {
                            state.handle_shared_server_failure().await;
                        }
                    }
                    ConnectionEvent::UpgradeCompleted { id, outcome } => {
                        if !connections.observe_upgrade_terminal(id, outcome) {
                            state.handle_shared_server_failure().await;
                        }
                    }
                    ConnectionEvent::Stop { abort } => {
                        stopping = true;
                        connections.stop_accepting();
                        connections.begin_closing();
                        if abort {
                            connections.cancel_owned();
                            tasks.abort_all();
                        }
                    }
                }
            }
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                match result {
                    Some(Ok((task_id, connection_id))) => {
                        task_connections.remove(&task_id);
                        if !connections.observe_connection_completion(connection_id, false) {
                            state.handle_shared_server_failure().await;
                        }
                    }
                    Some(Err(error)) => {
                        if let Some(connection_id) = task_connections.remove(&error.id()) {
                            if !connections.observe_connection_completion(
                                connection_id,
                                error.is_panic(),
                            ) {
                                state.handle_shared_server_failure().await;
                            }
                        } else {
                            state.handle_shared_server_failure().await;
                        }
                    }
                    None => {}
                }
            }
        }
    }
}

pub struct ServerHandle {
    accept_shutdown: tokio::sync::watch::Sender<bool>,
    ws_shutdown: tokio::sync::watch::Sender<bool>,
    forced_abort: Arc<AtomicBool>,
    execution_abort: tokio::task::AbortHandle,
    supervisor: tokio::task::JoinHandle<Result<(), ServerError>>,
    connection_supervisor: tokio::task::JoinHandle<()>,
    connections: ConnectionRegistry,
    #[cfg(test)]
    injected_failure: tokio::sync::mpsc::UnboundedSender<InjectedServerOutcome>,
    #[cfg(test)]
    secondary_cleanup_failures: Arc<AtomicUsize>,
}

#[cfg(test)]
enum InjectedServerOutcome {
    Failure,
    Panic,
    Return,
}

impl ServerHandle {
    pub fn start(listener: TcpListener, state: HttpState) -> Self {
        let (accept_shutdown, mut accept_receiver) = tokio::sync::watch::channel(false);
        let forced_abort = Arc::new(AtomicBool::new(false));
        let connections = state.connections.clone();
        let ws_shutdown = state.ws_shutdown.clone();
        let connection_events = connections.take_receiver();
        let connection_supervisor = tokio::spawn(supervise_connections(
            state.clone(),
            connections.clone(),
            connection_events,
        ));
        let router = router(state.clone());
        #[cfg(test)]
        let (injected_failure, mut injected_receiver) = tokio::sync::mpsc::unbounded_channel();
        let execution = tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                while !*accept_receiver.borrow() {
                    if accept_receiver.changed().await.is_err() {
                        break;
                    }
                }
            });
            #[cfg(test)]
            {
                tokio::select! {
                    result = server => result.map_err(ServerError::Serve),
                    injected = injected_receiver.recv() => match injected {
                        Some(InjectedServerOutcome::Failure) => Err(ServerError::InjectedSharedFailure),
                        Some(InjectedServerOutcome::Panic) => panic!("injected shared server task panic"),
                        Some(InjectedServerOutcome::Return) => Ok(()),
                        None => std::future::pending::<Result<(), ServerError>>().await,
                    }
                }
            }
            #[cfg(not(test))]
            server.await.map_err(ServerError::Serve)
        });
        let execution_abort = execution.abort_handle();
        let supervisor_forced = Arc::clone(&forced_abort);
        let supervisor = tokio::spawn(async move {
            let execution_outcome = execution.await;
            let shutdown_latched = state.shutdown_is_requested();
            let outcome = match execution_outcome {
                Ok(Ok(())) if shutdown_latched => return Ok(()),
                Ok(Ok(())) => ServerError::UnexpectedExit,
                Ok(Err(error)) => error,
                Err(error) if error.is_cancelled() && supervisor_forced.load(Ordering::Acquire) => {
                    ServerError::ShutdownDeadline
                }
                Err(error) => ServerError::ExecutionTask(error),
            };
            state.handle_shared_server_failure().await;
            Err(outcome)
        });
        #[cfg(test)]
        let secondary_cleanup_failures = Arc::new(AtomicUsize::new(0));
        Self {
            accept_shutdown,
            ws_shutdown,
            forced_abort,
            execution_abort,
            supervisor,
            connection_supervisor,
            connections,
            #[cfg(test)]
            injected_failure,
            #[cfg(test)]
            secondary_cleanup_failures,
        }
    }

    pub fn stop_accepting(&self) {
        self.connections.stop_accepting();
        let _ = self.accept_shutdown.send(true);
    }

    pub fn close_websockets(&self) {
        self.connections.begin_closing();
        self.ws_shutdown.send_replace(true);
    }

    #[cfg(test)]
    pub(crate) fn inject_shared_failure(&self) {
        let _ = self.injected_failure.send(InjectedServerOutcome::Failure);
    }

    #[cfg(test)]
    pub(crate) fn inject_server_panic(&self) {
        let _ = self.injected_failure.send(InjectedServerOutcome::Panic);
    }

    #[cfg(test)]
    pub(crate) fn inject_server_return(&self) {
        let _ = self.injected_failure.send(InjectedServerOutcome::Return);
    }

    #[cfg(test)]
    pub(crate) fn inject_connection_supervisor_panic(&self) {
        self.connection_supervisor.abort();
    }

    #[cfg(test)]
    pub(crate) fn secondary_cleanup_failures(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.secondary_cleanup_failures)
    }

    pub async fn join(self) -> Result<(), ServerError> {
        self.join_inner(None).await
    }

    pub async fn join_before(self, deadline: tokio::time::Instant) -> Result<(), ServerError> {
        self.join_inner(Some(deadline)).await
    }

    async fn join_inner(
        mut self,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<(), ServerError> {
        let server_result = if let Some(deadline) = deadline {
            match tokio::time::timeout_at(deadline, &mut self.supervisor).await {
                Ok(result) => result
                    .map_err(ServerError::SupervisorTask)
                    .and_then(|result| result),
                Err(_) => {
                    self.forced_abort.store(true, Ordering::Release);
                    self.execution_abort.abort();
                    self.supervisor
                        .await
                        .map_err(ServerError::SupervisorTask)
                        .and_then(|result| result)
                }
            }
        } else {
            self.supervisor
                .await
                .map_err(ServerError::SupervisorTask)
                .and_then(|result| result)
        };

        let drained = if let Some(deadline) = deadline {
            tokio::time::timeout_at(deadline, self.connections.wait_empty())
                .await
                .is_ok()
        } else {
            self.connections.wait_empty().await;
            true
        };
        let _ = self
            .connections
            .inner
            .events
            .send(ConnectionEvent::Stop { abort: !drained });
        let cleanup_result = self
            .connection_supervisor
            .await
            .map_err(ServerError::ConnectionSupervisorTask);
        match (server_result, cleanup_result) {
            (Err(primary), Err(secondary)) => {
                #[cfg(test)]
                self.secondary_cleanup_failures
                    .fetch_add(1, Ordering::AcqRel);
                tracing::error!(
                    error = %secondary,
                    "WebSocket cleanup also failed after the primary server failure"
                );
                Err(primary)
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Ok(()), Ok(())) if drained => Ok(()),
            (Ok(()), Ok(())) => Err(ServerError::ShutdownDeadline),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerErrorKind {
    Serve,
    UnexpectedExit,
    ExecutionTask,
    SupervisorTask,
    ConnectionSupervisorTask,
    ShutdownDeadline,
    #[cfg(test)]
    InjectedSharedFailure,
}

#[derive(Debug)]
pub enum ServerError {
    Serve(std::io::Error),
    UnexpectedExit,
    ExecutionTask(tokio::task::JoinError),
    SupervisorTask(tokio::task::JoinError),
    ConnectionSupervisorTask(tokio::task::JoinError),
    ShutdownDeadline,
    #[cfg(test)]
    InjectedSharedFailure,
}

impl ServerError {
    #[must_use]
    pub const fn kind(&self) -> ServerErrorKind {
        match self {
            Self::Serve(_) => ServerErrorKind::Serve,
            Self::UnexpectedExit => ServerErrorKind::UnexpectedExit,
            Self::ExecutionTask(_) => ServerErrorKind::ExecutionTask,
            Self::SupervisorTask(_) => ServerErrorKind::SupervisorTask,
            Self::ConnectionSupervisorTask(_) => ServerErrorKind::ConnectionSupervisorTask,
            Self::ShutdownDeadline => ServerErrorKind::ShutdownDeadline,
            #[cfg(test)]
            Self::InjectedSharedFailure => ServerErrorKind::InjectedSharedFailure,
        }
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind() {
            ServerErrorKind::Serve => "shared server execution failed",
            ServerErrorKind::UnexpectedExit => "shared server exited before shutdown",
            ServerErrorKind::ExecutionTask => "shared server execution task failed",
            ServerErrorKind::SupervisorTask => "shared server supervisor task failed",
            ServerErrorKind::ConnectionSupervisorTask => {
                "WebSocket connection supervisor task failed"
            }
            ServerErrorKind::ShutdownDeadline => "shared server shutdown deadline elapsed",
            #[cfg(test)]
            ServerErrorKind::InjectedSharedFailure => "injected shared server failure",
        })
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serve(error) => Some(error),
            Self::ExecutionTask(error)
            | Self::SupervisorTask(error)
            | Self::ConnectionSupervisorTask(error) => Some(error),
            Self::UnexpectedExit | Self::ShutdownDeadline => None,
            #[cfg(test)]
            Self::InjectedSharedFailure => None,
        }
    }
}

fn router(state: HttpState) -> Router {
    let health = Router::new()
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            HEALTH_TIMEOUT,
        ));

    let message = Router::new()
        .route(
            "/conversations/{conversation_id}/messages",
            post(submit_message),
        )
        .layer(DefaultBodyLimit::max(MESSAGE_BODY_LIMIT));
    let cancellation = Router::new()
        .route("/work-items/{work_id}/cancel", post(cancel_work))
        .layer(DefaultBodyLimit::max(CANCELLATION_BODY_LIMIT));
    let mutations = Router::new()
        .merge(message)
        .merge(cancellation)
        .layer(ConcurrencyLimitLayer::new(MUTATION_CONCURRENCY_LIMIT));
    let protected = Router::new()
        .route("/bootstrap", get(bootstrap))
        .merge(mutations)
        .route("/events", get(events))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn({
            let authentication_state = state.clone();
            move |request: Request, next: Next| {
                authenticate(authentication_state.clone(), request, next)
            }
        }));

    Router::new()
        .merge(health)
        .nest("/v1", protected)
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state.clone())
        .layer(
            ServiceBuilder::new()
                .layer(middleware::from_fn(request_identity))
                .layer(SetSensitiveRequestHeadersLayer::new(std::iter::once(
                    AUTHORIZATION,
                )))
                .layer(safe_trace_layer())
                .layer(middleware::from_fn(safe_response_trace))
                .layer(ConcurrencyLimitLayer::new(HTTP_CONCURRENCY_LIMIT))
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-store"),
                ))
                .layer(SetResponseHeaderLayer::overriding(
                    HeaderName::from_static("x-content-type-options"),
                    HeaderValue::from_static("nosniff"),
                ))
                .layer(middleware::from_fn({
                    let host_state = state.clone();
                    move |request: Request, next: Next| {
                        validate_host(host_state.clone(), request, next)
                    }
                })),
        )
}

async fn safe_response_trace(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(axum::extract::MatchedPath::as_str)
        .unwrap_or("unmatched")
        .to_owned();
    let request_id = request
        .extensions()
        .get::<RequestContext>()
        .map(|context| context.request_id.to_string())
        .unwrap_or_else(|| "unassigned".to_owned());
    let response = next.run(request).await;
    tracing::info!(
        request_id = %request_id,
        method = %method,
        matched_route = %route,
        status = response.status().as_u16(),
        latency_ms = started.elapsed().as_millis(),
        "http request completed"
    );
    response
}

fn safe_trace_layer() -> TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    impl Clone + Fn(&http::Request<Body>) -> tracing::Span,
> {
    TraceLayer::new_for_http().make_span_with(|request: &http::Request<Body>| {
        let route = request
            .extensions()
            .get::<axum::extract::MatchedPath>()
            .map(axum::extract::MatchedPath::as_str)
            .unwrap_or("unmatched");
        let request_id = request
            .extensions()
            .get::<RequestContext>()
            .map(|context| context.request_id.to_string())
            .unwrap_or_else(|| "unassigned".to_owned());
        tracing::info_span!(
            "http.request",
            request_id = %request_id,
            method = %request.method(),
            matched_route = %route,
        )
    })
}

#[derive(Clone)]
struct RequestContext {
    request_id: RequestId,
}

async fn request_identity(mut request: Request, next: Next) -> Response {
    request.headers_mut().remove("x-request-id");
    let context = RequestContext {
        request_id: RequestId::generate(),
    };
    request.extensions_mut().insert(context.clone());
    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&context.request_id.to_string()) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

async fn validate_host(state: HttpState, request: Request, next: Next) -> Response {
    let valid = one_header(request.headers(), HOST)
        .and_then(|value| value.to_str().map_err(|_| ()))
        .is_ok_and(|host| state.allowed_hosts.iter().any(|allowed| allowed == host));
    if !valid {
        return ApiError::invalid_request(context_id(&request)).into_response();
    }
    next.run(request).await
}

async fn authenticate(state: HttpState, mut request: Request, next: Next) -> Response {
    let request_id = context_id(&request);
    let token = match parse_authorization(request.headers()) {
        Ok(token) => token,
        Err(()) => return ApiError::authentication(request_id).into_response(),
    };
    let observed_at = match current_time(state.clock.as_ref()) {
        Ok(value) => value,
        Err(()) => return ApiError::authentication(request_id).into_response(),
    };
    let authenticated = match DeviceAuthenticator::new(state.store.as_ref())
        .authenticate_bearer(token, observed_at)
        .await
    {
        Ok(value) => value,
        Err(_) => return ApiError::authentication(request_id).into_response(),
    };
    request.extensions_mut().insert(authenticated);
    next.run(request).await
}

fn parse_authorization(headers: &HeaderMap) -> Result<String, ()> {
    let value = one_header(headers, AUTHORIZATION)?
        .to_str()
        .map_err(|_| ())?;
    let bytes = value.as_bytes();
    if bytes.len() != 71
        || !bytes[..6].eq_ignore_ascii_case(b"bearer")
        || bytes[6] != b' '
        || !bytes[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(());
    }
    Ok(value[7..].to_owned())
}

fn one_header(headers: &HeaderMap, name: HeaderName) -> Result<&HeaderValue, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(())?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(value)
}

async fn liveness(State(state): State<HttpState>) -> impl IntoResponse {
    let (status, health_status) = if state.health.snapshot().state() == HealthState::Fatal {
        (StatusCode::SERVICE_UNAVAILABLE, HealthStatus::Fatal)
    } else {
        (StatusCode::OK, HealthStatus::Live)
    };
    (
        status,
        Json(HealthResponse {
            protocol_version: ProtocolVersion,
            status: health_status,
        }),
    )
}

async fn readiness(State(state): State<HttpState>) -> impl IntoResponse {
    let current = state.health.snapshot().state();
    let (status, health_status) = match current {
        HealthState::Ready => (StatusCode::OK, HealthStatus::Ready),
        HealthState::LiveUnready => (StatusCode::SERVICE_UNAVAILABLE, HealthStatus::LiveUnready),
        HealthState::Draining => (StatusCode::SERVICE_UNAVAILABLE, HealthStatus::Draining),
        HealthState::Fatal => (StatusCode::SERVICE_UNAVAILABLE, HealthStatus::Fatal),
    };
    (
        status,
        Json(HealthResponse {
            protocol_version: ProtocolVersion,
            status: health_status,
        }),
    )
}

async fn bootstrap(
    State(state): State<HttpState>,
    Extension(context): Extension<RequestContext>,
) -> Result<Json<BootstrapResponse>, ApiError> {
    if !matches!(
        state.health.snapshot().state(),
        HealthState::LiveUnready | HealthState::Ready
    ) {
        return Err(ApiError::service_unavailable(context.request_id));
    }
    let result = tokio::time::timeout(
        BOOTSTRAP_TIMEOUT,
        PublicStateService::new(state.store.as_ref()).bootstrap(),
    )
    .await
    .map_err(|_| ApiError::command_timeout(context.request_id.clone()))?
    .map_err(|error| map_publication_error(&state, context.request_id, error))?;
    Ok(Json(result))
}

async fn submit_message(
    State(state): State<HttpState>,
    Extension(context): Extension<RequestContext>,
    Extension(authenticated): Extension<AuthenticatedDevice>,
    Path(conversation_id): Path<ConversationId>,
    headers: HeaderMap,
    body: Result<Json<MessageRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    require_json(&headers, &context.request_id)?;
    let Json(request) = body.map_err(|error| map_json_rejection(&context.request_id, error))?;
    let key = parse_idempotency_key(&headers, &context.request_id)?;
    key.require_message_id(request.client_message_id)
        .map_err(|_| ApiError::invalid_request(context.request_id.clone()))?;
    let client_message_id = request.client_message_id;
    let content = request
        .into_content()
        .map_err(|_| ApiError::invalid_request(context.request_id.clone()))?;
    let outcome = tokio::time::timeout(
        COMMAND_TIMEOUT,
        state.command_gateway.accept_message(
            authenticated,
            conversation_id,
            key,
            client_message_id,
            content,
        ),
    )
    .await
    .map_err(|_| ApiError::command_timeout(context.request_id.clone()))?
    .map_err(|error| map_gateway_error(&state, context.request_id.clone(), error))?;
    let duplicate = outcome.is_replay();
    let receipt = outcome.into_receipt();
    #[cfg(test)]
    if let Some(gate) = &state.post_commit_gate {
        gate.hold_once().await;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(MessageResponse {
            protocol_version: ProtocolVersion,
            message_id: receipt.message_id,
            work_id: receipt.work_id,
            work_state: receipt.work_state(),
            conversation_work_ordinal: receipt.work_ordinal,
            committed_cursor: receipt.committed_cursor,
            duplicate,
        }),
    )
        .into_response())
}

async fn cancel_work(
    State(state): State<HttpState>,
    Extension(context): Extension<RequestContext>,
    Extension(authenticated): Extension<AuthenticatedDevice>,
    Path(work_id): Path<WorkId>,
    headers: HeaderMap,
    body: Result<Json<CancellationRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    require_json(&headers, &context.request_id)?;
    let Json(request) = body.map_err(|error| map_json_rejection(&context.request_id, error))?;
    let key = parse_idempotency_key(&headers, &context.request_id)?;
    key.require_command_id(request.client_command_id)
        .map_err(|_| ApiError::invalid_request(context.request_id.clone()))?;
    let outcome = tokio::time::timeout(
        COMMAND_TIMEOUT,
        state
            .command_gateway
            .cancel_work(authenticated, work_id, key, request.client_command_id),
    )
    .await
    .map_err(|_| ApiError::command_timeout(context.request_id.clone()))?
    .map_err(|error| map_gateway_error(&state, context.request_id.clone(), error))?;
    let duplicate = outcome.is_replay();
    let receipt = outcome.into_receipt();
    let status = StatusCode::from_u16(receipt.http_status())
        .map_err(|_| ApiError::internal(context.request_id.clone()))?;
    Ok((
        status,
        Json(CancellationResponse {
            protocol_version: ProtocolVersion,
            work_id: receipt.work_id,
            work_state: receipt.resulting_work_state,
            committed_cursor: receipt.committed_cursor,
            duplicate,
            cleanup_pending: receipt.cleanup.is_pending(),
        }),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsQuery {
    after: String,
}

async fn events(
    State(state): State<HttpState>,
    Extension(context): Extension<RequestContext>,
    Query(query): Query<EventsQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if !matches!(
        state.health.snapshot().state(),
        HealthState::LiveUnready | HealthState::Ready
    ) {
        return Err(ApiError::service_unavailable(context.request_id));
    }
    let after = query
        .after
        .parse::<ReplayCursor>()
        .map_err(|_| ApiError::invalid_request(context.request_id.clone()))?;
    let permit = Arc::clone(&state.ws_limit)
        .try_acquire_owned()
        .map_err(|_| ApiError::overloaded(context.request_id.clone()))?;
    // Subscription precedes the high-water read so commits racing the read are only latency hints.
    let receiver = state.cursors.subscribe();
    let mut shutdown = state.ws_shutdown.subscribe();
    let public_state = PublicStateService::new(state.store.as_ref());
    let high_water = tokio::select! {
        biased;
        () = wait_for_shutdown(&mut shutdown) => {
            return Err(ApiError::service_unavailable(context.request_id));
        }
        result = tokio::time::timeout(
            REPLAY_PAGE_TIMEOUT,
            public_state.current_high_water(),
        ) => {
            result
                .map_err(|_| ApiError::overloaded(context.request_id.clone()))?
                .map_err(|error| map_publication_error(&state, context.request_id.clone(), error))?
        }
    };
    if after > high_water {
        return Err(ApiError::invalid_request(context.request_id));
    }
    let ownership = state
        .connections
        .reserve(permit)
        .map_err(|_| ApiError::service_unavailable(context.request_id.clone()))?;
    let failed_ownership = ownership.clone();
    // Construct the guard before handing the callback to Axum. If Axum drops the callback closure
    // or its future before polling it, dropping this owned wrapper still emits a real terminal
    // outcome instead of leaving the reservation silently unreachable.
    let callback_completion = UpgradeCallbackGuard::new(&ownership);
    #[cfg(test)]
    let upgrade_gate = state.upgrade_gate.clone();
    Ok(upgrade
        .on_failed_upgrade(move |_| failed_ownership.upgrade_failed())
        .on_upgrade(move |socket| {
            let completion = callback_completion;
            async move {
                let _completion = completion;
                let cancellation = ownership.cancellation();
                #[cfg(test)]
                let mut cancellation = cancellation;
                #[cfg(test)]
                if let Some(gate) = upgrade_gate
                    && !gate.enter(&mut cancellation).await
                {
                    return;
                }
                if *cancellation.borrow() {
                    return;
                }
                ownership.activate(socket, receiver, after, high_water);
            }
        })
        .into_response())
}

async fn run_websocket(
    mut socket: WebSocket,
    state: HttpState,
    mut hints: tokio::sync::broadcast::Receiver<ReplayCursor>,
    after: ReplayCursor,
    replay_high_water: ReplayCursor,
) {
    let mut shutdown = state.ws_shutdown.subscribe();
    if shutdown_is_latched(&shutdown) {
        close(&mut socket, close_code::AWAY, "server shutdown").await;
        return;
    }
    let mut scanned = after;
    match scan_and_send(
        &mut socket,
        &state,
        &mut shutdown,
        &mut scanned,
        replay_high_water,
    )
    .await
    {
        Ok(()) => {}
        Err(WebSocketFlowError::Shutdown) => {
            close(&mut socket, close_code::AWAY, "server shutdown").await;
            return;
        }
        Err(WebSocketFlowError::Failed) => return,
    }
    let mut pending_live_scan = false;
    loop {
        match hints.try_recv() {
            Ok(cursor) if cursor <= replay_high_water => {}
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                pending_live_scan = true;
                break;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
    match send_json(
        &mut socket,
        &state,
        &mut shutdown,
        &SyncCompleteEnvelope::new(replay_high_water),
    )
    .await
    {
        Ok(()) => {}
        Err(WebSocketFlowError::Shutdown) => {
            close(&mut socket, close_code::AWAY, "server shutdown").await;
            return;
        }
        Err(WebSocketFlowError::Failed) => {
            close(&mut socket, close_code::AGAIN, "temporary overload").await;
            return;
        }
    }
    if pending_live_scan {
        match live_scan(&mut socket, &state, &mut shutdown, &mut scanned).await {
            Ok(()) => {}
            Err(WebSocketFlowError::Shutdown) => {
                close(&mut socket, close_code::AWAY, "server shutdown").await;
                return;
            }
            Err(WebSocketFlowError::Failed) => return,
        }
    }

    let mut fallback = tokio::time::interval(WS_FALLBACK_SCAN);
    fallback.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    fallback.tick().await;
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    close(&mut socket, close_code::AWAY, "server shutdown").await;
                    return;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                        close(&mut socket, close_code::POLICY, "server delivery only").await;
                        return;
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                }
            }
            hint = hints.recv() => {
                match hint {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        match live_scan(&mut socket, &state, &mut shutdown, &mut scanned).await {
                            Ok(()) => {}
                            Err(WebSocketFlowError::Shutdown) => {
                                close(&mut socket, close_code::AWAY, "server shutdown").await;
                                return;
                            }
                            Err(WebSocketFlowError::Failed) => return,
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }
            _ = fallback.tick() => {
                match live_scan(&mut socket, &state, &mut shutdown, &mut scanned).await {
                    Ok(()) => {}
                    Err(WebSocketFlowError::Shutdown) => {
                        close(&mut socket, close_code::AWAY, "server shutdown").await;
                        return;
                    }
                    Err(WebSocketFlowError::Failed) => return,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebSocketFlowError {
    Shutdown,
    Failed,
}

fn shutdown_is_latched(shutdown: &tokio::sync::watch::Receiver<bool>) -> bool {
    *shutdown.borrow()
}

async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    while !shutdown_is_latched(shutdown) {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn live_scan(
    socket: &mut WebSocket,
    state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    scanned: &mut ReplayCursor,
) -> Result<(), WebSocketFlowError> {
    let public_state = PublicStateService::new(state.store.as_ref());
    let high_water = match tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => return Err(WebSocketFlowError::Shutdown),
        result = tokio::time::timeout(
            REPLAY_PAGE_TIMEOUT,
            public_state.current_high_water(),
        ) => result,
    } {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return replay_failure(socket, state, error).await,
        Err(_) => {
            close(socket, close_code::AGAIN, "temporary overload").await;
            return Err(WebSocketFlowError::Failed);
        }
    };
    scan_and_send(socket, state, shutdown, scanned, high_water).await
}

async fn scan_and_send(
    socket: &mut WebSocket,
    state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    scanned: &mut ReplayCursor,
    through: ReplayCursor,
) -> Result<(), WebSocketFlowError> {
    while *scanned < through {
        let public_state = PublicStateService::new(state.store.as_ref());
        let page = match tokio::select! {
            biased;
            () = wait_for_shutdown(shutdown) => return Err(WebSocketFlowError::Shutdown),
            result = tokio::time::timeout(
                REPLAY_PAGE_TIMEOUT,
                public_state.replay_page(*scanned, through),
            ) => result,
        } {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return replay_failure(socket, state, error).await,
            Err(_) => {
                close(socket, close_code::AGAIN, "temporary overload").await;
                return Err(WebSocketFlowError::Failed);
            }
        };
        match send_events(socket, state, shutdown, page.events).await {
            Ok(()) => {}
            Err(WebSocketFlowError::Shutdown) => return Err(WebSocketFlowError::Shutdown),
            Err(WebSocketFlowError::Failed) => {
                close(socket, close_code::AGAIN, "slow consumer").await;
                return Err(WebSocketFlowError::Failed);
            }
        }
        if page.scanned_through <= *scanned || page.scanned_through > through {
            state.fatal_protocol();
            close(socket, close_code::ERROR, "protocol invariant").await;
            return Err(WebSocketFlowError::Failed);
        }
        *scanned = page.scanned_through;
        if !page.has_more && *scanned != through {
            state.fatal_protocol();
            close(socket, close_code::ERROR, "protocol invariant").await;
            return Err(WebSocketFlowError::Failed);
        }
    }
    Ok(())
}

async fn send_events(
    socket: &mut WebSocket,
    state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    events: Vec<crate::protocol::DurableEventEnvelope>,
) -> Result<(), WebSocketFlowError> {
    let mut outbound = VecDeque::with_capacity(WEBSOCKET_OUTBOUND_FRAMES);
    for event in events {
        let encoded = match encode_public_event_frame(&event) {
            Ok(encoded) => encoded,
            Err(_) => {
                state.fatal_protocol();
                close(socket, close_code::ERROR, "protocol invariant").await;
                return Err(WebSocketFlowError::Failed);
            }
        };
        outbound.push_back(Message::Text(encoded.into()));
        if outbound.len() == WEBSOCKET_OUTBOUND_FRAMES {
            flush_outbound(socket, state, shutdown, &mut outbound).await?;
        }
    }
    flush_outbound(socket, state, shutdown, &mut outbound).await
}

async fn flush_outbound(
    socket: &mut WebSocket,
    state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    outbound: &mut VecDeque<Message>,
) -> Result<(), WebSocketFlowError> {
    while let Some(frame) = outbound.pop_front() {
        send_frame(socket, state, shutdown, frame).await?;
    }
    Ok(())
}

async fn replay_failure(
    socket: &mut WebSocket,
    state: &HttpState,
    error: PublicationError,
) -> Result<(), WebSocketFlowError> {
    match error.kind() {
        PublicationErrorKind::Storage | PublicationErrorKind::Invariant => {
            state.fatal_protocol();
            close(socket, close_code::ERROR, "replay failure").await;
        }
        PublicationErrorKind::BootstrapLimitExceeded => {
            close(socket, close_code::ERROR, "protocol invariant").await;
        }
    }
    Err(WebSocketFlowError::Failed)
}

async fn send_json<T: serde::Serialize>(
    socket: &mut WebSocket,
    state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    value: &T,
) -> Result<(), WebSocketFlowError> {
    let encoded = serde_json::to_string(value).map_err(|_| WebSocketFlowError::Failed)?;
    send_frame(socket, state, shutdown, Message::Text(encoded.into())).await
}

async fn send_frame(
    socket: &mut WebSocket,
    _state: &HttpState,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    frame: Message,
) -> Result<(), WebSocketFlowError> {
    tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => Err(WebSocketFlowError::Shutdown),
        result = tokio::time::timeout(WS_SEND_TIMEOUT, async {
            #[cfg(test)]
            if _state
                .ws_send_stall
                .as_ref()
                .is_some_and(|stall| stall.load(Ordering::Acquire))
            {
                std::future::pending::<()>().await;
            }
            socket.send(frame).await
        }) => {
            result
                .map_err(|_| WebSocketFlowError::Failed)?
                .map_err(|_| WebSocketFlowError::Failed)
        }
    }
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = tokio::time::timeout(
        WS_SEND_TIMEOUT,
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
}

fn require_json(headers: &HeaderMap, request_id: &RequestId) -> Result<(), ApiError> {
    let content_type = one_header(headers, CONTENT_TYPE)
        .and_then(|value| value.to_str().map_err(|_| ()))
        .map_err(|_| ApiError::unsupported_media_type(request_id.clone()))?;
    let media_type = content_type.split(';').next().unwrap_or("").trim();
    if !media_type.eq_ignore_ascii_case("application/json") {
        return Err(ApiError::unsupported_media_type(request_id.clone()));
    }
    Ok(())
}

fn parse_idempotency_key(
    headers: &HeaderMap,
    request_id: &RequestId,
) -> Result<IdempotencyKey, ApiError> {
    let name = HeaderName::from_static("idempotency-key");
    let value = one_header(headers, name)
        .and_then(|value| value.to_str().map_err(|_| ()))
        .map_err(|_| ApiError::invalid_request(request_id.clone()))?;
    IdempotencyKey::parse_canonical(value)
        .map_err(|_| ApiError::invalid_request(request_id.clone()))
}

fn map_json_rejection(request_id: &RequestId, error: JsonRejection) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::payload_too_large(request_id.clone())
    } else if error.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE {
        ApiError::unsupported_media_type(request_id.clone())
    } else {
        ApiError::invalid_request(request_id.clone())
    }
}

fn map_gateway_error(
    state: &HttpState,
    request_id: RequestId,
    error: CommandGatewayError,
) -> ApiError {
    match (error.kind(), error.command_kind()) {
        (CommandGatewayErrorKind::Unavailable, _) => ApiError::service_unavailable(request_id),
        (CommandGatewayErrorKind::Clock, _) => {
            state.fatal_protocol();
            ApiError::internal(request_id)
        }
        (CommandGatewayErrorKind::Command, Some(CommandServiceErrorKind::IdempotencyConflict)) => {
            ApiError::idempotency_conflict(request_id)
        }
        (CommandGatewayErrorKind::Command, Some(CommandServiceErrorKind::TargetNotFound)) => {
            ApiError::not_found(request_id)
        }
        (
            CommandGatewayErrorKind::Command,
            Some(CommandServiceErrorKind::CommandValidationFailed),
        ) => ApiError::invalid_request(request_id),
        (CommandGatewayErrorKind::Command, Some(CommandServiceErrorKind::StorageFailure)) => {
            ApiError::service_unavailable(request_id)
        }
        (CommandGatewayErrorKind::Command, Some(CommandServiceErrorKind::StorageInconsistent))
        | (CommandGatewayErrorKind::Command, None) => {
            state.fatal_protocol();
            ApiError::internal(request_id)
        }
    }
}

fn map_publication_error(
    state: &HttpState,
    request_id: RequestId,
    error: PublicationError,
) -> ApiError {
    match error.kind() {
        PublicationErrorKind::BootstrapLimitExceeded => ApiError::bootstrap_limit(request_id),
        PublicationErrorKind::Storage => ApiError::service_unavailable(request_id),
        PublicationErrorKind::Invariant => {
            state.fatal_protocol();
            ApiError::internal(request_id)
        }
    }
}

fn current_time(clock: &dyn Clock) -> Result<UtcTimestamp, ()> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| ())?).map_err(|_| ())
}

fn context_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestContext>()
        .map(|context| context.request_id.clone())
        .unwrap_or_else(RequestId::generate)
}

async fn not_found(Extension(context): Extension<RequestContext>) -> ApiError {
    ApiError::not_found(context.request_id)
}

async fn method_not_allowed(Extension(context): Extension<RequestContext>) -> ApiError {
    ApiError::method_not_allowed(context.request_id)
}

struct ApiError {
    status: StatusCode,
    envelope: ErrorEnvelope,
    authenticate: bool,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        retryable: bool,
        request_id: RequestId,
    ) -> Self {
        Self {
            status,
            envelope: ErrorEnvelope::new(code, message, retryable, request_id),
            authenticate: false,
        }
    }

    fn invalid_request(id: RequestId) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is invalid.",
            false,
            id,
        )
    }

    fn authentication(id: RequestId) -> Self {
        let mut error = Self::new(
            StatusCode::UNAUTHORIZED,
            "authentication_failed",
            "Authentication failed.",
            false,
            id,
        );
        error.authenticate = true;
        error
    }

    fn not_found(id: RequestId) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
            false,
            id,
        )
    }

    fn method_not_allowed(id: RequestId) -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
            "The method is not allowed for this resource.",
            false,
            id,
        )
    }

    fn idempotency_conflict(id: RequestId) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "idempotency_conflict",
            "The idempotency key was already used for different command material.",
            false,
            id,
        )
    }

    fn payload_too_large(id: RequestId) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "The request payload is too large.",
            false,
            id,
        )
    }

    fn unsupported_media_type(id: RequestId) -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "The request content type is not supported.",
            false,
            id,
        )
    }

    fn service_unavailable(id: RequestId) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "The service is temporarily unavailable.",
            true,
            id,
        )
    }

    fn overloaded(id: RequestId) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded",
            "The service is temporarily overloaded.",
            true,
            id,
        )
    }

    fn command_timeout(id: RequestId) -> Self {
        Self::new(
            StatusCode::GATEWAY_TIMEOUT,
            "command_timeout",
            "The command response timed out; retry with the same idempotency key.",
            true,
            id,
        )
    }

    fn internal(id: RequestId) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "An internal error occurred.",
            false,
            id,
        )
    }

    fn bootstrap_limit(id: RequestId) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "bootstrap_limit_exceeded",
            "The bootstrap snapshot exceeds the supported limit.",
            false,
            id,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.envelope)).into_response();
        if self.authenticate {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_header_grammar_is_exact_and_scheme_only_is_case_insensitive() {
        for scheme in ["Bearer", "bearer", "BEARER", "BeArEr"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("{scheme} {}", "01".repeat(32))).unwrap(),
            );
            assert_eq!(parse_authorization(&headers).unwrap(), "01".repeat(32));
        }
        for value in [
            "",
            "Bearer",
            "Bearer  00",
            "Bearer\t00",
            "Bearer 00,00",
            "Bearer 00 extra",
            &format!("Bearer {}", "AB".repeat(32)),
            &format!("Bearer {} ", "01".repeat(32)),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            assert!(parse_authorization(&headers).is_err(), "{value}");
        }
        let mut duplicate = HeaderMap::new();
        duplicate.append(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", "01".repeat(32))).unwrap(),
        );
        duplicate.append(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", "02".repeat(32))).unwrap(),
        );
        assert!(parse_authorization(&duplicate).is_err());
    }

    #[tokio::test]
    async fn stage11_second_repair_duplicate_or_late_terminal_record_is_rejected_without_double_accounting()
     {
        let connections = ConnectionRegistry::default();
        let permit = Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap();
        let pending = connections.reserve(permit).unwrap();
        let id = pending.inner.id;
        pending.upgrade_failed();
        let mut events = connections.take_receiver();
        let outcome = match events.recv().await.unwrap() {
            ConnectionEvent::UpgradeCompleted {
                id: observed,
                outcome,
            } => {
                assert_eq!(observed, id);
                outcome
            }
            _ => panic!("expected one real terminal record"),
        };
        assert!(connections.observe_upgrade_terminal(id, outcome));
        assert!(!connections.observe_upgrade_terminal(id, UpgradeTerminalOutcome::Failed));
        assert_eq!(connections.owned(), 0);
        assert_eq!(connections.observed_completions(), 1);
    }
}
