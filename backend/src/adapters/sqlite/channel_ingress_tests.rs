use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::Row as _;

use crate::application::channel_ingress::{
    ChannelIngressErrorKind, ChannelIngressService, DurableInboundOutcome,
    InboundAdmissionDisposition, UnsupportedInboundKind, VerifiedInboundEvent,
    VerifiedInboundPayload,
};
use crate::application::command_service::CommandPostCommit;
use crate::application::delivery_planner::ChannelDeliveryProfileRegistry;
use crate::application::delivery_worker::{
    DeliveryJitterSource, DeliveryNotifier, DeliveryWorkerError, start_delivery_worker,
};
use crate::application::transport::MutationAdmission;
use crate::bootstrap::health::Health;
use crate::domain::{
    ChannelAccount, ChannelAccountId, ChannelAccountLifecycle, ChannelDeliveryProfile,
    ChannelDispatchResult, ChannelProviderId, ContentBlock, ConversationBinding,
    ConversationBindingId, ConversationBindingLifecycle, ConversationId, CorrelationId, CraxiiId,
    DeliveryFailure, DeliveryFailureClass, DiagnosticPid, ExternalAccountId,
    ExternalConversationId, ExternalEventId, ExternalIdentity, ExternalIdentityId,
    ExternalIdentityLifecycle, ExternalMessageId, ExternalSubjectId, InboundDelivery,
    InboundDeliveryId, InboundReceiptState, JournalActor, JournalEventId, JournalEventPayload,
    LinuxBootId, MessageAcceptedOriginV2, MessageContent, OutboundDeliveryAttemptId,
    OutboundDeliveryState, PackageVersion, PreparedChannelDispatch, RuntimeInstanceId,
    RuntimeStartEvidence, RuntimeStartEvidenceInput, SchemaVersion, UserId, UtcTimestamp, WorkId,
    WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::channel_delivery::{
    ChannelDeliveryAdapter, ChannelDeliveryAdapterRegistry, ChannelDeliveryFuture,
};
use crate::ports::channel_identity::{ChannelIdentityStore, DisableChannelAccountOutcome};
use crate::ports::clock::{Clock, ClockError, MonotonicInstant, TestClock};
use crate::ports::delivery_store::{
    ClaimDeliveryRequest, DeliveryClaim, DeliveryRecoveryReceipt, DeliveryRoute, DeliveryStore,
    DeliveryStoreError, DeliveryStoreErrorKind, DeliveryStoreFuture, ListDeliverySummariesRequest,
    LoadDeliveryRouteRequest, PersistDispatchResultDisposition, PersistDispatchResultRequest,
    RecoverDeliveriesRequest, ShutdownDeliveryRequest,
};
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ClaimNextWorkRequest, CreateRuntimeRequest,
    ExecutionCapabilityObservation, LoadOrBootstrapIdentityRequest, RuntimeStateStore,
    SchedulerStateStore, V0IdentityReference,
};

use super::ch3_test_support::create_conversation;
use super::journal::load_global_events;
use super::{SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2026-09-16T01:02:03.000000Z";
const T1: &str = "2026-09-16T01:02:04.000000Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-ch2-test-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn at(value: &str) -> UtcTimestamp {
    value.parse().unwrap()
}

fn text(value: &str) -> MessageContent {
    MessageContent::try_new(vec![ContentBlock::text(value).unwrap()]).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectKind {
    Message,
    ActiveCancellation,
    DirectCancellation,
}

#[derive(Clone, Default)]
struct EffectRecorder(Arc<Mutex<Vec<(EffectKind, WorkId, i64)>>>);

#[derive(Clone)]
enum FakeDispatchMode {
    Result(ChannelDispatchResult),
    PendingWithDrop(Arc<AtomicBool>),
    Wait(Arc<tokio::sync::Notify>, ChannelDispatchResult),
    Panic,
}

struct DropMarker(Arc<AtomicBool>);

impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct FakeDeliveryAdapter {
    provider: ChannelProviderId,
    runtime: super::SqliteRuntime,
    mode: FakeDispatchMode,
    calls: Arc<Mutex<Vec<PreparedChannelDispatch>>>,
    invoked: tokio::sync::mpsc::UnboundedSender<()>,
}

impl ChannelDeliveryAdapter for FakeDeliveryAdapter {
    fn provider_id(&self) -> &ChannelProviderId {
        &self.provider
    }

    fn dispatch(&self, dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.unwrap();
            let evidence = sqlx::query(
                "SELECT d.state, d.attempt_count, a.completed_at, a.dispatch_material_sha256 \
                 FROM outbound_deliveries d JOIN outbound_delivery_attempts a \
                   ON a.outbound_delivery_id = d.outbound_delivery_id \
                 WHERE d.outbound_delivery_id = ? AND a.outbound_delivery_attempt_id = ?",
            )
            .bind(dispatch.outbound_delivery_id.to_string())
            .bind(dispatch.outbound_delivery_attempt_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
            assert_eq!(evidence.get::<String, _>("state"), "dispatching");
            assert_eq!(
                evidence.get::<i64, _>("attempt_count"),
                i64::from(dispatch.attempt_number)
            );
            assert_eq!(evidence.get::<Option<String>, _>("completed_at"), None);
            assert_eq!(
                evidence.get::<String, _>("dispatch_material_sha256"),
                dispatch.dispatch_material_sha256.to_string()
            );
            // A write lock can be acquired while adapter I/O is active: the claim transaction
            // was committed before this callback.
            sqlx::query(
                "UPDATE outbound_deliveries SET updated_at = updated_at \
                 WHERE outbound_delivery_id = ?",
            )
            .bind(dispatch.outbound_delivery_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
            drop(connection);
            self.calls.lock().unwrap().push(dispatch);
            let _ = self.invoked.send(());
            match &self.mode {
                FakeDispatchMode::Result(result) => result.clone(),
                FakeDispatchMode::PendingWithDrop(dropped) => {
                    let _marker = DropMarker(Arc::clone(dropped));
                    std::future::pending().await
                }
                FakeDispatchMode::Wait(release, result) => {
                    release.notified().await;
                    result.clone()
                }
                FakeDispatchMode::Panic => panic!("synthetic adapter panic"),
            }
        })
    }
}

struct FixedJitter(u64);

impl DeliveryJitterSource for FixedJitter {
    fn sample_inclusive(&mut self, upper_bound_millis: u64) -> u64 {
        self.0.min(upper_bound_millis)
    }
}

pub(super) struct BlockingDeliveryStore {
    inner: Arc<dyn DeliveryStore>,
    block_next_claim: AtomicBool,
    fail_claim: bool,
    fail_interrupt: bool,
    claim_calls: AtomicUsize,
    interrupt_calls: AtomicUsize,
    claim_entered: tokio::sync::mpsc::UnboundedSender<()>,
    release_claim: tokio::sync::Notify,
}

impl BlockingDeliveryStore {
    pub(super) fn new(
        inner: Arc<dyn DeliveryStore>,
        block_next_claim: bool,
        fail_claim: bool,
        fail_interrupt: bool,
    ) -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let (claim_entered, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                inner,
                block_next_claim: AtomicBool::new(block_next_claim),
                fail_claim,
                fail_interrupt,
                claim_calls: AtomicUsize::new(0),
                interrupt_calls: AtomicUsize::new(0),
                claim_entered,
                release_claim: tokio::sync::Notify::new(),
            }),
            receiver,
        )
    }

    pub(super) fn release_claim(&self) {
        self.release_claim.notify_one();
    }
}

impl DeliveryStore for BlockingDeliveryStore {
    fn load_delivery_route(
        &self,
        request: LoadDeliveryRouteRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRoute> {
        self.inner.load_delivery_route(request)
    }

    fn claim_next_delivery(
        &self,
        request: ClaimDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryClaim> {
        Box::pin(async move {
            self.claim_calls.fetch_add(1, Ordering::AcqRel);
            if self.block_next_claim.swap(false, Ordering::AcqRel) {
                let released = self.release_claim.notified();
                let _ = self.claim_entered.send(());
                released.await;
            }
            if self.fail_claim {
                Err(DeliveryStoreError::new(DeliveryStoreErrorKind::Storage))
            } else {
                self.inner.claim_next_delivery(request).await
            }
        })
    }

    fn persist_dispatch_result(
        &self,
        request: PersistDispatchResultRequest,
    ) -> DeliveryStoreFuture<'_, PersistDispatchResultDisposition> {
        self.inner.persist_dispatch_result(request)
    }

    fn recover_stale_deliveries(
        &self,
        request: RecoverDeliveriesRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt> {
        self.inner.recover_stale_deliveries(request)
    }

    fn interrupt_owned_delivery(
        &self,
        request: ShutdownDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt> {
        Box::pin(async move {
            self.interrupt_calls.fetch_add(1, Ordering::AcqRel);
            if self.fail_interrupt {
                Err(DeliveryStoreError::new(DeliveryStoreErrorKind::Storage))
            } else {
                self.inner.interrupt_owned_delivery(request).await
            }
        })
    }

    fn list_delivery_summaries(
        &self,
        request: ListDeliverySummariesRequest,
    ) -> DeliveryStoreFuture<'_, Vec<crate::ports::delivery_store::DeliverySummary>> {
        self.inner.list_delivery_summaries(request)
    }

    fn verify_delivery_consistency(&self) -> DeliveryStoreFuture<'_, u64> {
        self.inner.verify_delivery_consistency()
    }
}

struct FailAfterFirstWallClock {
    wall: time::OffsetDateTime,
    calls: AtomicUsize,
}

impl Clock for FailAfterFirstWallClock {
    fn utc_now(&self) -> Result<time::OffsetDateTime, ClockError> {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            Ok(self.wall)
        } else {
            Err(ClockError::SynchronizationFailure)
        }
    }

    fn monotonic_now(&self) -> MonotonicInstant {
        MonotonicInstant::from_elapsed(Duration::ZERO)
    }
}

impl EffectRecorder {
    fn snapshot(&self) -> Vec<(EffectKind, WorkId, i64)> {
        self.0.lock().unwrap().clone()
    }
}

impl CommandPostCommit for EffectRecorder {
    fn message_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::Message, work_id, cursor.get()));
    }

    fn active_cancellation_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::ActiveCancellation, work_id, cursor.get()));
    }

    fn direct_cancellation_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::DirectCancellation, work_id, cursor.get()));
    }
}

struct Fixture {
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: Arc<SqliteStateStore>,
    owner: V0IdentityReference,
    account: ChannelAccount,
    identity: ExternalIdentity,
    binding: ConversationBinding,
    effects: EffectRecorder,
}

impl Fixture {
    fn service(&self) -> ChannelIngressService<SqliteStateStore, EffectRecorder> {
        let health = Health::new();
        health.mark_ready().unwrap();
        ChannelIngressService::new(
            Arc::clone(&self.store),
            health,
            MutationAdmission::new(),
            self.effects.clone(),
        )
    }

    fn service_with_delivery_profile(
        &self,
    ) -> ChannelIngressService<SqliteStateStore, EffectRecorder> {
        let profiles = Arc::new(
            ChannelDeliveryProfileRegistry::try_new([ChannelDeliveryProfile::try_new(
                self.account.provider_id.clone(),
                64,
                64,
            )
            .unwrap()])
            .unwrap(),
        );
        self.service()
            .with_delivery(profiles, DeliveryNotifier::new())
    }

    fn event(&self, event_id: &str, message_id: &str, value: &str) -> VerifiedInboundEvent {
        VerifiedInboundEvent::new(
            self.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            Some(ExternalMessageId::try_new(message_id).unwrap()),
            self.identity.external_subject_id.clone(),
            self.binding.external_conversation_id.clone(),
            self.binding.external_thread_id.clone(),
            VerifiedInboundPayload::Text(text(value)),
            Some(at(T0)),
            at(T1),
        )
        .unwrap()
    }

    fn unsupported(&self, event_id: &str) -> VerifiedInboundEvent {
        VerifiedInboundEvent::new(
            self.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            None,
            self.identity.external_subject_id.clone(),
            self.binding.external_conversation_id.clone(),
            self.binding.external_thread_id.clone(),
            VerifiedInboundPayload::Unsupported(UnsupportedInboundKind::NonText),
            None,
            at(T1),
        )
        .unwrap()
    }
}

async fn fixture() -> Fixture {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let owner = store
        .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
            proposed: V0IdentityReference {
                craxii_id: CraxiiId::generate(),
                user_id: UserId::generate(),
                conversation_id: ConversationId::generate(),
                workstation_id: WorkstationId::generate(),
                workspace_id: WorkspaceId::generate(),
            },
            initialized_event_id: JournalEventId::generate(),
            conversation_created_event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
            created_at: at(T0),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".into(),
                os_release: "ch2-test".into(),
                default_shell: "/bin/sh".into(),
                workspace_logical_name: "primary".into(),
                workspace_logical_root: "/workspace".into(),
                workspace_resolved_root: "/workspace".into(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    let identity_store = SqliteChannelIdentityStore::new(guard.runtime().clone());
    let account = ChannelAccount {
        channel_account_id: ChannelAccountId::generate(),
        craxii_id: owner.craxii_id,
        provider_id: ChannelProviderId::try_new("test.adapter").unwrap(),
        external_account_id: ExternalAccountId::try_new("owner-account").unwrap(),
        lifecycle: ChannelAccountLifecycle::Active,
        created_at: at(T0),
        disabled_at: None,
    };
    identity_store
        .persist_channel_account(account.clone())
        .await
        .unwrap();
    let identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: account.channel_account_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("owner-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(identity.clone())
        .await
        .unwrap();
    let binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: account.channel_account_id,
        external_identity_id: identity.external_identity_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        conversation_id: owner.conversation_id,
        external_conversation_id: ExternalConversationId::try_new("owner-conversation").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(binding.clone())
        .await
        .unwrap();
    Fixture {
        _root: root,
        guard,
        store,
        owner,
        account,
        identity,
        binding,
        effects: EffectRecorder::default(),
    }
}

async fn table_count(fixture: &Fixture, table: &str) -> i64 {
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

async fn create_runtime(fixture: &Fixture) -> RuntimeInstanceId {
    let runtime_id = RuntimeInstanceId::generate();
    fixture
        .store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence: RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: fixture.owner.craxii_id,
                workstation_id: fixture.owner.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("ch2-test-boot").unwrap()),
                diagnostic_pid: Some(DiagnosticPid::try_new(42).unwrap()),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: crate::domain::GitRevision::try_new("ch2-test").unwrap(),
                schema_version: SchemaVersion::try_new(7).unwrap(),
                started_at: at(T1),
            }),
            event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
        })
        .await
        .unwrap();
    runtime_id
}

fn delivery_profiles(fixture: &Fixture) -> Arc<ChannelDeliveryProfileRegistry> {
    Arc::new(
        ChannelDeliveryProfileRegistry::try_new([ChannelDeliveryProfile::try_new(
            fixture.account.provider_id.clone(),
            64,
            64,
        )
        .unwrap()])
        .unwrap(),
    )
}

async fn wait_for_delivery_state(fixture: &Fixture, expected: &str) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let mut connection = fixture.guard.runtime().acquire().await.unwrap();
            let state = sqlx::query_scalar::<_, String>("SELECT state FROM outbound_deliveries")
                .fetch_optional(&mut *connection)
                .await
                .unwrap();
            if state.as_deref() == Some(expected) {
                return;
            }
            drop(connection);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("delivery worker did not reach the expected state");
}

async fn enqueue_control_delivery(fixture: &Fixture, tag: &str) {
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event(&format!("{tag}-event"), &format!("{tag}-message"), "stop"))
        .await
        .unwrap();
}

async fn claimed_control_delivery(
    tag: &str,
) -> (Fixture, RuntimeInstanceId, Box<PreparedChannelDispatch>) {
    let fixture = fixture().await;
    enqueue_control_delivery(&fixture, tag).await;
    let runtime_id = create_runtime(&fixture).await;
    let DeliveryClaim::Dispatch(dispatch) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("delivery claim expected");
    };
    (fixture, runtime_id, dispatch)
}

async fn accepted_control_delivery(tag: &str, external_message_id: Option<&str>) -> Fixture {
    let (fixture, runtime_id, dispatch) = claimed_control_delivery(tag).await;
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::Accepted {
                external_message_id: external_message_id
                    .map(|value| ExternalMessageId::try_new(value).unwrap()),
            },
            local_retry_delay: None,
            completed_at: at(T1),
        })
        .await
        .unwrap();
    fixture
}

async fn retry_wait_control_delivery(tag: &str) -> Fixture {
    let (fixture, runtime_id, dispatch) = claimed_control_delivery(tag).await;
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::RetryableFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderRetryable, None)
                    .unwrap(),
                retry_after: None,
            },
            local_retry_delay: Some(Duration::from_secs(1)),
            completed_at: at(T1),
        })
        .await
        .unwrap();
    fixture
}

async fn assert_delivery_consistency_rejects(fixture: Fixture) {
    assert!(fixture.store.verify_delivery_consistency().await.is_err());
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn message_admission_is_atomic_canonical_and_duplicate_safe() {
    let fixture = fixture().await;
    let service = fixture.service();
    let original = fixture.event("event-1", "message-1", "  exact é  ");
    let first = service.classify(original.clone()).await.unwrap();
    assert_eq!(
        first.disposition,
        InboundAdmissionDisposition::NewlyClassified
    );
    assert!(first.acknowledgement_safe());
    let DurableInboundOutcome::MessageAccepted {
        inbound_delivery_id,
        message_id,
        work_id,
        work_ordinal,
        committed_cursor,
    } = first.outcome
    else {
        panic!("message outcome expected");
    };
    assert_eq!(work_ordinal.get(), 1);
    assert_eq!(table_count(&fixture, "inbound_deliveries").await, 1);
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 1);
    assert_eq!(
        fixture.effects.snapshot(),
        vec![(EffectKind::Message, work_id, committed_cursor.get())]
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT d.classification, d.message_id, d.work_id, m.content_json, \
                m.author_user_id, m.inbound_delivery_id, w.reply_binding_id \
         FROM inbound_deliveries d JOIN messages m ON m.message_id = d.message_id \
         JOIN work_items w ON w.work_id = d.work_id WHERE d.inbound_delivery_id = ?",
    )
    .bind(inbound_delivery_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<String, _>("classification").unwrap(),
        "message"
    );
    assert_eq!(
        row.try_get::<String, _>("message_id").unwrap(),
        message_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("work_id").unwrap(),
        work_id.to_string()
    );
    assert!(
        row.try_get::<String, _>("content_json")
            .unwrap()
            .contains("  exact é  ")
    );
    assert_eq!(
        row.try_get::<String, _>("author_user_id").unwrap(),
        fixture.owner.user_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("inbound_delivery_id").unwrap(),
        inbound_delivery_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("reply_binding_id").unwrap(),
        fixture.binding.conversation_binding_id.to_string()
    );
    drop(connection);

    let events = load_global_events(fixture.guard.runtime()).await.unwrap();
    assert!(events.iter().any(|event| {
        event.actor == JournalActor::UserV2(fixture.owner.user_id)
            && matches!(
                &event.payload,
                JournalEventPayload::MessageAcceptedV2(payload)
                    if payload.message_id == message_id
                        && payload.origin == MessageAcceptedOriginV2::InboundDelivery {
                            inbound_delivery_id
                        }
            )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            JournalEventPayload::WorkQueuedV2(payload)
                if payload.work_id == work_id
                    && payload.reply_binding_id == Some(fixture.binding.conversation_binding_id)
        )
    }));

    let duplicate = service.classify(original).await.unwrap();
    assert_eq!(
        duplicate.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(duplicate.outcome, first.outcome);
    let secondary = service
        .classify(fixture.event("event-2", "message-1", "  exact é  "))
        .await
        .unwrap();
    assert_eq!(
        secondary.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(secondary.outcome, first.outcome);
    assert_eq!(fixture.effects.snapshot().len(), 1);
    assert_eq!(table_count(&fixture, "messages").await, 1);

    let conflict = service
        .classify(fixture.event("event-1", "message-1", "changed"))
        .await
        .unwrap_err();
    assert_eq!(
        conflict.kind(),
        ChannelIngressErrorKind::ContradictoryReplay
    );
    assert!(!conflict.acknowledgement_safe());
    let secondary_conflict = service
        .classify(fixture.event("event-3", "message-1", "changed"))
        .await
        .unwrap_err();
    assert_eq!(
        secondary_conflict.kind(),
        ChannelIngressErrorKind::ContradictoryReplay
    );

    assert_eq!(
        service
            .classify(fixture.event("event-real-2", "message-real-2", "second"))
            .await
            .unwrap()
            .disposition,
        InboundAdmissionDisposition::NewlyClassified
    );
    let split_keys = service
        .classify(fixture.event("event-1", "message-real-2", "second"))
        .await
        .unwrap_err();
    assert_eq!(
        split_keys.kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );
    assert_eq!(table_count(&fixture, "messages").await, 2);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn queued_control_is_v2_eventful_and_duplicate_cannot_cancel_next_work() {
    let fixture = fixture().await;
    let service = fixture.service();
    let accepted = service
        .classify(fixture.event("event-message-1", "message-1", "ordinary"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted { work_id, .. } = accepted.outcome else {
        panic!("message outcome expected");
    };
    let control_event = fixture.event("event-control-1", "message-control-1", "  /CaNcEl  ");
    let control = service.classify(control_event.clone()).await.unwrap();
    let DurableInboundOutcome::ControlApplied {
        inbound_delivery_id,
        target_work_id,
    } = control.outcome
    else {
        panic!("control outcome expected");
    };
    assert_eq!(target_work_id, work_id);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "cancelled"
    );
    drop(connection);
    let events = load_global_events(fixture.guard.runtime()).await.unwrap();
    assert!(events.iter().any(|event| {
        event.actor == JournalActor::UserV2(fixture.owner.user_id)
            && matches!(
                &event.payload,
                JournalEventPayload::WorkCancelledV2(payload)
                    if payload.inbound_delivery_id == inbound_delivery_id
                        && payload.transition.work_id == work_id
            )
    }));
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 1);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::DirectCancellation
    );
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let acknowledgement = sqlx::query(
        "SELECT control_outcome, payload_text, state, failure_class \
         FROM outbound_deliveries WHERE source_inbound_delivery_id = ?",
    )
    .bind(inbound_delivery_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        acknowledgement.get::<String, _>("control_outcome"),
        "applied"
    );
    assert_eq!(
        acknowledgement.get::<String, _>("payload_text"),
        "Cancellation requested."
    );
    assert_eq!(
        acknowledgement.get::<String, _>("state"),
        "permanent_failure"
    );
    assert_eq!(
        acknowledgement.get::<String, _>("failure_class"),
        "profile_unavailable"
    );
    drop(connection);

    let second = service
        .classify(fixture.event("event-message-2", "message-2", "next"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: second_work,
        ..
    } = second.outcome
    else {
        panic!("message outcome expected");
    };
    let effect_count = fixture.effects.snapshot().len();
    let duplicate = service.classify(control_event).await.unwrap();
    assert_eq!(
        duplicate.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(duplicate.outcome, control.outcome);
    assert_eq!(fixture.effects.snapshot().len(), effect_count);
    assert_eq!(table_count(&fixture, "outbound_deliveries").await, 1);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(second_work.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "queued"
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn active_control_precedes_queued_and_already_requested_is_eventless() {
    let fixture = fixture().await;
    let service = fixture.service();
    let first = service
        .classify(fixture.event("active-message", "active-message", "first"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: active_work,
        ..
    } = first.outcome
    else {
        panic!("message outcome expected");
    };
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), active_work);
    let second = service
        .classify(fixture.event("queued-message", "queued-message", "second"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: queued_work,
        ..
    } = second.outcome
    else {
        panic!("message outcome expected");
    };

    let first_control = service
        .classify(fixture.event("active-control", "active-control", "STOP"))
        .await
        .unwrap();
    let DurableInboundOutcome::ControlApplied { target_work_id, .. } = first_control.outcome else {
        panic!("control outcome expected");
    };
    assert_eq!(target_work_id, active_work);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::ActiveCancellation
    );
    let journal_count = table_count(&fixture, "journal_events").await;
    let effect_count = fixture.effects.snapshot().len();

    let second_control = service
        .classify(fixture.event("second-control", "second-control", "cancel"))
        .await
        .unwrap();
    assert_eq!(
        second_control.outcome,
        DurableInboundOutcome::ControlApplied {
            inbound_delivery_id: match second_control.outcome {
                DurableInboundOutcome::ControlApplied {
                    inbound_delivery_id,
                    ..
                } => inbound_delivery_id,
                _ => unreachable!(),
            },
            target_work_id: active_work,
        }
    );
    assert_eq!(table_count(&fixture, "journal_events").await, journal_count);
    assert_eq!(fixture.effects.snapshot().len(), effect_count);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(queued_work.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "queued"
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn non_root_active_control_is_conversation_local_and_signals_only_its_work() {
    let fixture = fixture().await;
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());
    let user_b = UserId::generate();
    let conversation_b = create_conversation(
        fixture.guard.runtime(),
        fixture.owner.craxii_id,
        user_b,
        true,
        at(T0),
    )
    .await
    .unwrap()
    .conversation_id;
    let identity_b = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: user_b,
        external_subject_id: ExternalSubjectId::try_new("secondary-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(identity_b.clone())
        .await
        .unwrap();
    let binding_b = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: identity_b.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: user_b,
        conversation_id: conversation_b,
        external_conversation_id: ExternalConversationId::try_new("secondary-destination").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(binding_b.clone())
        .await
        .unwrap();
    let service = fixture.service();
    let event_b = |event_id: &str, message_id: &str, payload: VerifiedInboundPayload| {
        VerifiedInboundEvent::new(
            fixture.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            Some(ExternalMessageId::try_new(message_id).unwrap()),
            identity_b.external_subject_id.clone(),
            binding_b.external_conversation_id.clone(),
            None,
            payload,
            None,
            at(T1),
        )
        .unwrap()
    };
    let admitted_b = service
        .classify(event_b(
            "secondary-message-event",
            "secondary-message",
            VerifiedInboundPayload::Text(text("secondary work")),
        ))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: work_b, ..
    } = admitted_b.outcome
    else {
        panic!("secondary message outcome expected");
    };
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), work_b);

    let admitted_a = service
        .classify(fixture.event("root-message-event", "root-message", "root work"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: work_a, ..
    } = admitted_a.outcome
    else {
        panic!("root message outcome expected");
    };
    let control = service
        .classify(event_b(
            "secondary-control-event",
            "secondary-control",
            VerifiedInboundPayload::Text(text("STOP")),
        ))
        .await
        .unwrap();
    let DurableInboundOutcome::ControlApplied { target_work_id, .. } = control.outcome else {
        panic!("secondary control outcome expected");
    };
    assert_eq!(target_work_id, work_b);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::ActiveCancellation
    );
    assert_eq!(fixture.effects.snapshot().last().unwrap().1, work_b);

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let states: (String, String) = sqlx::query_as(
        "SELECT (SELECT state FROM work_items WHERE work_id = ?), \
                (SELECT state FROM work_items WHERE work_id = ?)",
    )
    .bind(work_a.to_string())
    .bind(work_b.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(states.0, "queued");
    assert_eq!(states.1, "cancel_requested");
    let root_cancellation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? \
         AND event_type IN ('work.cancellation_requested','work.cancelled')",
    )
    .bind(work_a.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(root_cancellation_events, 0);
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn unsupported_rejected_no_target_and_received_replay_are_fail_closed() {
    let fixture = fixture().await;
    let service = fixture.service();
    assert!(matches!(
        service
            .classify(fixture.unsupported("unsupported"))
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::Unsupported { .. }
    ));
    assert!(matches!(
        service
            .classify(fixture.event("no-target", "no-target", "/stop"))
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::ControlNoOp { .. }
    ));

    let missing_identity = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("missing-identity").unwrap(),
        Some(ExternalMessageId::try_new("missing-identity").unwrap()),
        ExternalSubjectId::try_new("unknown-subject").unwrap(),
        fixture.binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(missing_identity).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let received = fixture.event("stuck-received", "stuck-received", "hello");
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());
    identity_store
        .persist_inbound_delivery(InboundDelivery {
            inbound_delivery_id: InboundDeliveryId::generate(),
            channel_account_id: fixture.account.channel_account_id,
            craxii_id: fixture.owner.craxii_id,
            external_event_id: received.external_event_id().clone(),
            external_message_id: received.external_message_id().cloned(),
            external_subject_id: received.sender_subject_id().clone(),
            external_conversation_id: received.external_conversation_id().clone(),
            external_thread_id: received.external_thread_id().cloned(),
            material_sha256: received.material_digest(),
            provider_occurred_at: received.provider_occurred_at(),
            received_at: received.observed_at(),
            receipt_state: InboundReceiptState::Received,
            classification: None,
            external_identity_id: None,
            conversation_binding_id: None,
            user_id: None,
            conversation_id: None,
            message_id: None,
            work_id: None,
            control_target_work_id: None,
            classified_at: None,
        })
        .await
        .unwrap();
    let error = service.classify(received).await.unwrap_err();
    assert_eq!(error.kind(), ChannelIngressErrorKind::StorageInconsistent);
    assert!(!error.acknowledgement_safe());
    assert_eq!(table_count(&fixture, "messages").await, 0);
    assert_eq!(table_count(&fixture, "work_items").await, 0);
    assert_eq!(table_count(&fixture, "outbound_deliveries").await, 1);
    assert_eq!(fixture.effects.snapshot().len(), 0);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn authorization_failures_reject_but_unknown_or_corrupt_topology_fails_closed() {
    let fixture = fixture().await;
    let service = fixture.service();
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());

    let disabled_account = ChannelAccount {
        channel_account_id: ChannelAccountId::generate(),
        craxii_id: fixture.owner.craxii_id,
        provider_id: ChannelProviderId::try_new("disabled.adapter").unwrap(),
        external_account_id: ExternalAccountId::try_new("disabled-account").unwrap(),
        lifecycle: ChannelAccountLifecycle::Disabled,
        created_at: at(T0),
        disabled_at: Some(at(T1)),
    };
    identity_store
        .persist_channel_account(disabled_account.clone())
        .await
        .unwrap();
    let disabled = VerifiedInboundEvent::new(
        disabled_account.channel_account_id,
        ExternalEventId::try_new("disabled-event").unwrap(),
        Some(ExternalMessageId::try_new("disabled-message").unwrap()),
        ExternalSubjectId::try_new("disabled-subject").unwrap(),
        ExternalConversationId::try_new("disabled-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(disabled).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let revoked_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("revoked-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Revoked,
        created_at: at(T0),
        revoked_at: Some(at(T1)),
    };
    identity_store
        .persist_external_identity(revoked_identity.clone())
        .await
        .unwrap();
    let revoked_event = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("revoked-event").unwrap(),
        Some(ExternalMessageId::try_new("revoked-message").unwrap()),
        revoked_identity.external_subject_id.clone(),
        ExternalConversationId::try_new("revoked-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(revoked_event).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let revoked_binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: fixture.identity.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        conversation_id: fixture.owner.conversation_id,
        external_conversation_id: ExternalConversationId::try_new("revoked-destination").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Revoked,
        created_at: at(T0),
        revoked_at: Some(at(T1)),
    };
    identity_store
        .persist_conversation_binding(revoked_binding.clone())
        .await
        .unwrap();
    let revoked_binding_event = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("revoked-binding-event").unwrap(),
        Some(ExternalMessageId::try_new("revoked-binding-message").unwrap()),
        fixture.identity.external_subject_id.clone(),
        revoked_binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service
            .classify(revoked_binding_event)
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let other_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("other-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(other_identity.clone())
        .await
        .unwrap();
    let mismatched_sender = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("mismatched-event").unwrap(),
        Some(ExternalMessageId::try_new("mismatched-message").unwrap()),
        other_identity.external_subject_id.clone(),
        fixture.binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(mismatched_sender).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let second_user = UserId::generate();
    let second_conversation = create_conversation(
        fixture.guard.runtime(),
        fixture.owner.craxii_id,
        second_user,
        true,
        at(T0),
    )
    .await
    .unwrap()
    .conversation_id;
    let second_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: second_user,
        external_subject_id: ExternalSubjectId::try_new("second-user-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(second_identity.clone())
        .await
        .unwrap();
    let second_binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: second_identity.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: second_user,
        conversation_id: second_conversation,
        external_conversation_id: ExternalConversationId::try_new("second-conversation").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(second_binding.clone())
        .await
        .unwrap();
    let non_root = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("non-root-event").unwrap(),
        Some(ExternalMessageId::try_new("non-root-message").unwrap()),
        second_identity.external_subject_id.clone(),
        second_binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    let non_root_outcome = service.classify(non_root).await.unwrap().outcome;
    let (non_root_work_id, non_root_message_id) = match non_root_outcome {
        DurableInboundOutcome::MessageAccepted {
            work_id,
            message_id,
            ..
        } => (work_id, message_id),
        other => panic!("valid non-root admission was not accepted: {other:?}"),
    };
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let route: (String, String, String) = sqlx::query_as(
        "SELECT m.conversation_id, w.conversation_id, w.reply_binding_id \
         FROM messages m JOIN work_items w ON w.work_id = ? \
         WHERE m.message_id = ?",
    )
    .bind(non_root_work_id.to_string())
    .bind(non_root_message_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(route.0, second_conversation.to_string());
    assert_eq!(route.1, second_conversation.to_string());
    assert_eq!(route.2, second_binding.conversation_binding_id.to_string());
    drop(connection);
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), non_root_work_id);
    assert_eq!(claimed.work.conversation_id(), second_conversation);

    let unknown_account = VerifiedInboundEvent::new(
        ChannelAccountId::generate(),
        ExternalEventId::try_new("unknown-account-event").unwrap(),
        Some(ExternalMessageId::try_new("unknown-account-message").unwrap()),
        ExternalSubjectId::try_new("unknown-account-subject").unwrap(),
        ExternalConversationId::try_new("unknown-account-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert_eq!(
        service.classify(unknown_account).await.unwrap_err().kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE external_identities SET craxii_id = ? WHERE external_identity_id = ?")
        .bind(CraxiiId::generate().to_string())
        .bind(fixture.identity.external_identity_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        service
            .classify(fixture.event("corrupt-event", "corrupt-message", "hello"))
            .await
            .unwrap_err()
            .kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn concurrent_events_linearize_dedupe_and_canonical_ordinals() {
    let fixture = fixture().await;
    let service = Arc::new(fixture.service());
    let same = fixture.event("same-event", "same-message", "same");
    let first_service = Arc::clone(&service);
    let first_event = same.clone();
    let first = tokio::spawn(async move { first_service.classify(first_event).await.unwrap() });
    let second_service = Arc::clone(&service);
    let second = tokio::spawn(async move { second_service.classify(same).await.unwrap() });
    let dispositions = [
        first.await.unwrap().disposition,
        second.await.unwrap().disposition,
    ];
    assert_eq!(
        dispositions
            .iter()
            .filter(|value| **value == InboundAdmissionDisposition::NewlyClassified)
            .count(),
        1
    );
    assert_eq!(
        dispositions
            .iter()
            .filter(|value| **value == InboundAdmissionDisposition::Duplicate)
            .count(),
        1
    );

    let a_service = Arc::clone(&service);
    let a_event = fixture.event("event-a", "message-a", "a");
    let a = tokio::spawn(async move { a_service.classify(a_event).await.unwrap() });
    let b_service = Arc::clone(&service);
    let b_event = fixture.event("event-b", "message-b", "b");
    let b = tokio::spawn(async move { b_service.classify(b_event).await.unwrap() });
    let outcomes = [a.await.unwrap().outcome, b.await.unwrap().outcome];
    let mut ordinals = outcomes
        .iter()
        .map(|outcome| match outcome {
            DurableInboundOutcome::MessageAccepted { work_ordinal, .. } => work_ordinal.get(),
            _ => panic!("message outcome expected"),
        })
        .collect::<Vec<_>>();
    ordinals.sort_unstable();
    assert_eq!(ordinals, vec![2, 3]);
    assert_eq!(table_count(&fixture, "messages").await, 3);
    assert_eq!(table_count(&fixture, "work_items").await, 3);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn non_exact_controls_and_whitespace_preserve_native_message_semantics() {
    let fixture = fixture().await;
    let service = fixture.service();
    for (index, value) in [
        "stop please",
        "please stop",
        "don't stop",
        "can you stop?",
        "   ",
    ]
    .into_iter()
    .enumerate()
    {
        let result = service
            .classify(fixture.event(
                &format!("ordinary-event-{index}"),
                &format!("ordinary-message-{index}"),
                value,
            ))
            .await
            .unwrap();
        assert!(matches!(
            result.outcome,
            DurableInboundOutcome::MessageAccepted { .. }
        ));
    }
    assert_eq!(table_count(&fixture, "messages").await, 5);
    assert_eq!(table_count(&fixture, "work_items").await, 5);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 5);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn control_ack_intent_is_atomic_exact_and_duplicate_safe() {
    let fixture = fixture().await;
    let service = fixture.service_with_delivery_profile();
    let event = fixture.event("delivery-control", "delivery-message", "/cancel");
    let first = service.classify(event.clone()).await.unwrap();
    assert!(matches!(
        first.outcome,
        DurableInboundOutcome::ControlNoOp { .. }
    ));
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT source_kind, control_outcome, payload_text, state, part_ordinal, part_count \
         FROM outbound_deliveries",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("source_kind"), "control");
    assert_eq!(row.get::<String, _>("control_outcome"), "no_op");
    assert_eq!(
        row.get::<String, _>("payload_text"),
        "Nothing is currently running or queued."
    );
    assert_eq!(row.get::<String, _>("state"), "queued");
    assert_eq!(row.get::<i64, _>("part_ordinal"), 1);
    assert_eq!(row.get::<i64, _>("part_count"), 1);
    drop(connection);

    let duplicate = service.classify(event).await.unwrap();
    assert_eq!(
        duplicate.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(table_count(&fixture, "outbound_deliveries").await, 1);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
}

#[tokio::test]
async fn claim_commits_attempt_before_result_and_acceptance_is_idempotent() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("accepted-control", "accepted-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let attempt_id = OutboundDeliveryAttemptId::generate();
    let DeliveryClaim::Dispatch(dispatch) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: attempt_id,
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("delivery must be claimed");
    };
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let evidence = sqlx::query(
        "SELECT d.state, d.attempt_count, a.completed_at, a.dispatch_material_sha256 \
         FROM outbound_deliveries d JOIN outbound_delivery_attempts a \
           ON a.outbound_delivery_id = d.outbound_delivery_id",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(evidence.get::<String, _>("state"), "dispatching");
    assert_eq!(evidence.get::<i64, _>("attempt_count"), 1);
    assert_eq!(evidence.get::<Option<String>, _>("completed_at"), None);
    assert_eq!(
        evidence.get::<String, _>("dispatch_material_sha256"),
        dispatch.dispatch_material_sha256.to_string()
    );
    drop(connection);

    let accepted = ChannelDispatchResult::Accepted {
        external_message_id: Some(ExternalMessageId::try_new("provider-message").unwrap()),
    };
    let request = PersistDispatchResultRequest {
        dispatch: dispatch.clone(),
        runtime_instance_id: runtime_id,
        result: accepted.clone(),
        local_retry_delay: None,
        completed_at: at("2026-09-16T01:02:05.000000Z"),
    };
    assert_eq!(
        fixture
            .store
            .persist_dispatch_result(request.clone())
            .await
            .unwrap(),
        PersistDispatchResultDisposition::Applied
    );
    assert_eq!(
        fixture
            .store
            .persist_dispatch_result(request)
            .await
            .unwrap(),
        PersistDispatchResultDisposition::Idempotent
    );
    let inexact_replay = fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch: dispatch.clone(),
            runtime_instance_id: runtime_id,
            result: accepted,
            local_retry_delay: None,
            completed_at: at("2026-09-16T01:02:05.000001Z"),
        })
        .await
        .unwrap_err();
    assert_eq!(inexact_replay.kind(), DeliveryStoreErrorKind::StateConflict);
    let late = fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::OutcomeUnknown {
                failure: DeliveryFailure::classified(DeliveryFailureClass::ProviderOutcomeUnknown),
            },
            local_retry_delay: None,
            completed_at: at("2026-09-16T01:02:06.000000Z"),
        })
        .await
        .unwrap_err();
    assert_eq!(late.kind(), DeliveryStoreErrorKind::StateConflict);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "accepted"
    );
    drop(connection);
    let summaries = fixture
        .store
        .list_delivery_summaries(ListDeliverySummariesRequest {
            states: vec![OutboundDeliveryState::Accepted],
            after: None,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    let debug = format!("{:?}", summaries[0]);
    assert!(!debug.contains("Nothing is currently running or queued."));
    assert!(!debug.contains("owner-conversation"));
    assert!(!debug.contains("provider-message"));
    assert!(
        fixture
            .store
            .list_delivery_summaries(ListDeliverySummariesRequest {
                states: vec![OutboundDeliveryState::Accepted],
                after: Some(summaries[0].outbound_delivery_id),
                limit: 100,
            })
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn delivery_consistency_rejects_dispatch_runtime_owner_mismatch() {
    let (fixture, attempt_runtime, _dispatch) =
        claimed_control_delivery("runtime-owner-mismatch").await;
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let parent_runtime = create_runtime(&fixture).await;
    assert_ne!(attempt_runtime, parent_runtime);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET dispatch_runtime_instance_id = ? \
         WHERE state = 'dispatching'",
    )
    .bind(parent_runtime.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    assert!(
        fixture
            .store
            .verify_application_consistency()
            .await
            .is_err()
    );
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn delivery_consistency_rejects_accepted_attempt_backing_outcome_unknown_parent() {
    let fixture = accepted_control_delivery("accepted-attempt-unknown-parent", None).await;
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'outcome_unknown', \
                accepted_external_message_id = NULL, failure_class = 'stale_dispatch', \
                failure_code = NULL, accepted_at = NULL \
         WHERE state = 'accepted'",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    assert!(
        fixture
            .store
            .verify_application_consistency()
            .await
            .is_err()
    );
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn delivery_consistency_rejects_parent_latest_result_and_evidence_matrix() {
    let accepted_from_retry = retry_wait_control_delivery("accepted-from-retry").await;
    let mut connection = accepted_from_retry.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'accepted', next_attempt_at = NULL, \
                accepted_at = ?, terminal_at = ?",
    )
    .bind(T1)
    .bind(T1)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(accepted_from_retry).await;

    let accepted_id = accepted_control_delivery("accepted-id-mismatch", Some("attempt-id")).await;
    let mut connection = accepted_id.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET accepted_external_message_id = 'different-parent-id'",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(accepted_id).await;

    let retry_from_accepted = accepted_control_delivery("retry-from-accepted", None).await;
    let mut connection = retry_from_accepted.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'retry_wait', next_attempt_at = ?, \
                accepted_at = NULL, terminal_at = NULL",
    )
    .bind("2026-09-16T01:02:05.000000Z")
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(retry_from_accepted).await;

    let retry_schedule = retry_wait_control_delivery("retry-schedule-mismatch").await;
    let mut connection = retry_schedule.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE outbound_deliveries SET next_attempt_at = ?")
        .bind("2026-09-16T01:02:06.000000Z")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(retry_schedule).await;

    let unknown_from_retry = retry_wait_control_delivery("unknown-from-retry").await;
    let mut connection = unknown_from_retry.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'outcome_unknown', next_attempt_at = NULL, \
                failure_class = 'stale_dispatch', terminal_at = ?",
    )
    .bind(T1)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(unknown_from_retry).await;

    let (terminal_failure, runtime_id, dispatch) =
        claimed_control_delivery("terminal-failure-mismatch").await;
    terminal_failure
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::PermanentFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderPermanent, None)
                    .unwrap(),
            },
            local_retry_delay: None,
            completed_at: at(T1),
        })
        .await
        .unwrap();
    let mut connection = terminal_failure.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE outbound_deliveries SET failure_class = 'adapter_unavailable'")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(terminal_failure).await;
}

#[tokio::test]
async fn delivery_consistency_rejects_attempt_history_and_open_attempt_matrix() {
    let queued_history = retry_wait_control_delivery("queued-with-history").await;
    let mut connection = queued_history.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'queued', attempt_count = 0, \
                next_attempt_at = created_at, updated_at = created_at",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(queued_history).await;

    let (cardinality, _runtime_id, _dispatch) =
        claimed_control_delivery("attempt-cardinality").await;
    let mut connection = cardinality.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE outbound_deliveries SET attempt_count = 2")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(cardinality).await;

    let (numbering, _runtime_id, _dispatch) = claimed_control_delivery("attempt-gap").await;
    let mut connection = numbering.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE outbound_delivery_attempts SET attempt_number = 2, prior_state = 'retry_wait'",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(numbering).await;

    let (open_non_dispatching, _runtime_id, _dispatch) =
        claimed_control_delivery("open-non-dispatching").await;
    let mut connection = open_non_dispatching
        .guard
        .runtime()
        .acquire()
        .await
        .unwrap();
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'permanent_failure', \
                dispatch_runtime_instance_id = NULL, failure_class = 'binding_revoked', \
                terminal_at = ?",
    )
    .bind(T1)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(open_non_dispatching).await;

    let (dispatch_without_open, _runtime_id, _dispatch) =
        claimed_control_delivery("dispatch-without-open").await;
    let mut connection = dispatch_without_open
        .guard
        .runtime()
        .acquire()
        .await
        .unwrap();
    sqlx::query(
        "UPDATE outbound_delivery_attempts SET completed_at = ?, \
                result_kind = 'outcome_unknown', failure_class = 'stale_dispatch'",
    )
    .bind(T1)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(dispatch_without_open).await;

    let (open_not_latest, runtime_id, dispatch) = claimed_control_delivery("open-not-latest").await;
    let mut connection = open_not_latest.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO outbound_delivery_attempts (outbound_delivery_attempt_id, \
         outbound_delivery_id, runtime_instance_id, attempt_number, prior_state, \
         dispatch_material_version, dispatch_material_sha256, started_at, completed_at, \
         result_kind, failure_class, selected_retry_delay_ms, scheduled_next_attempt_at) \
         VALUES (?, ?, ?, 2, 'retry_wait', 1, ?, ?, ?, 'retryable_failure', \
                 'provider_retryable', 1000, ?)",
    )
    .bind(OutboundDeliveryAttemptId::generate().to_string())
    .bind(dispatch.outbound_delivery_id.to_string())
    .bind(runtime_id.to_string())
    .bind(dispatch.dispatch_material_sha256.to_string())
    .bind(T1)
    .bind(T1)
    .bind("2026-09-16T01:02:05.000000Z")
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query("UPDATE outbound_deliveries SET attempt_count = 2")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_delivery_consistency_rejects(open_not_latest).await;
}

#[tokio::test]
async fn delivery_schema_blocks_more_than_one_open_attempt() {
    let (fixture, runtime_id, dispatch) = claimed_control_delivery("two-open-attempts").await;
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let second_open = sqlx::query(
        "INSERT INTO outbound_delivery_attempts (outbound_delivery_attempt_id, \
         outbound_delivery_id, runtime_instance_id, attempt_number, prior_state, \
         dispatch_material_version, dispatch_material_sha256, started_at) \
         VALUES (?, ?, ?, 2, 'retry_wait', 1, ?, ?)",
    )
    .bind(OutboundDeliveryAttemptId::generate().to_string())
    .bind(dispatch.outbound_delivery_id.to_string())
    .bind(runtime_id.to_string())
    .bind(dispatch.dispatch_material_sha256.to_string())
    .bind(T1)
    .execute(&mut *connection)
    .await;
    assert!(second_open.is_err());
    drop(connection);
    fixture.store.verify_delivery_consistency().await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn delivery_consistency_accepts_current_writer_state_matrix() {
    let queued = fixture().await;
    enqueue_control_delivery(&queued, "valid-queued").await;
    queued.store.verify_delivery_consistency().await.unwrap();
    queued.guard.shutdown().await;

    let (dispatching, _runtime_id, _dispatch) = claimed_control_delivery("valid-dispatching").await;
    dispatching
        .store
        .verify_delivery_consistency()
        .await
        .unwrap();
    dispatching.guard.shutdown().await;

    let retry_wait = retry_wait_control_delivery("valid-retry-wait").await;
    retry_wait
        .store
        .verify_delivery_consistency()
        .await
        .unwrap();
    retry_wait.guard.shutdown().await;

    let predispatch_after_retry =
        retry_wait_control_delivery("valid-predispatch-after-retry").await;
    let mut connection = predispatch_after_retry
        .guard
        .runtime()
        .acquire()
        .await
        .unwrap();
    sqlx::query(
        "UPDATE conversation_bindings SET lifecycle_state = 'revoked', revoked_at = ? \
         WHERE conversation_binding_id = ?",
    )
    .bind("2026-09-16T01:02:05.000000Z")
    .bind(
        predispatch_after_retry
            .binding
            .conversation_binding_id
            .to_string(),
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let recovery_runtime = create_runtime(&predispatch_after_retry).await;
    assert!(matches!(
        predispatch_after_retry
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: recovery_runtime,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at("2026-09-16T01:02:05.000000Z"),
            })
            .await
            .unwrap(),
        DeliveryClaim::StateAdvanced
    ));
    predispatch_after_retry
        .store
        .verify_delivery_consistency()
        .await
        .unwrap();
    predispatch_after_retry.guard.shutdown().await;

    let accepted = accepted_control_delivery("valid-accepted", Some("provider-accepted")).await;
    accepted.store.verify_delivery_consistency().await.unwrap();
    accepted.guard.shutdown().await;

    let (permanent, runtime_id, dispatch) = claimed_control_delivery("valid-permanent").await;
    permanent
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::PermanentFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderPermanent, None)
                    .unwrap(),
            },
            local_retry_delay: None,
            completed_at: at(T1),
        })
        .await
        .unwrap();
    permanent.store.verify_delivery_consistency().await.unwrap();
    permanent.guard.shutdown().await;

    let (unknown, runtime_id, dispatch) = claimed_control_delivery("valid-unknown").await;
    unknown
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::OutcomeUnknown {
                failure: DeliveryFailure::classified(DeliveryFailureClass::ProviderOutcomeUnknown),
            },
            local_retry_delay: None,
            completed_at: at(T1),
        })
        .await
        .unwrap();
    unknown.store.verify_delivery_consistency().await.unwrap();
    unknown.guard.shutdown().await;
}

#[tokio::test]
async fn retry_guidance_due_gating_unknown_and_stale_recovery_are_conservative() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("retry-control", "retry-message", "cancel"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let DeliveryClaim::Dispatch(first) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("first claim expected");
    };
    let persisted_text = first.text.clone();
    let persisted_digest = first.payload_sha256;
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch: first,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::RetryableFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderRetryable, None)
                    .unwrap(),
                retry_after: Some(std::time::Duration::from_secs(5)),
            },
            local_retry_delay: Some(std::time::Duration::from_secs(2)),
            completed_at: at("2026-09-16T01:02:05.000000Z"),
        })
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at("2026-09-16T01:02:09.999999Z"),
            })
            .await
            .unwrap(),
        DeliveryClaim::NoneDue
    ));
    fixture.store.verify_delivery_consistency().await.unwrap();
    let DeliveryClaim::Dispatch(second) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at("2026-09-16T01:02:10.000000Z"),
        })
        .await
        .unwrap()
    else {
        panic!("due retry expected");
    };
    assert_eq!(second.attempt_number, 2);
    let replacement_profiles =
        ChannelDeliveryProfileRegistry::try_new([ChannelDeliveryProfile::try_new(
            fixture.account.provider_id.clone(),
            128,
            1,
        )
        .unwrap()])
        .unwrap();
    assert_eq!(
        replacement_profiles
            .profile(&fixture.account.provider_id)
            .unwrap()
            .max_text_utf8_bytes(),
        128
    );
    assert_eq!(second.text, persisted_text);
    assert_eq!(second.payload_sha256, persisted_digest);
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch: second,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::OutcomeUnknown {
                failure: DeliveryFailure::classified(DeliveryFailureClass::ProviderOutcomeUnknown),
            },
            local_retry_delay: None,
            completed_at: at("2026-09-16T01:02:11.000000Z"),
        })
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at("2026-09-16T01:02:12.000000Z"),
            })
            .await
            .unwrap(),
        DeliveryClaim::NoneDue
    ));

    let second_fixture = self::fixture().await;
    second_fixture
        .service_with_delivery_profile()
        .classify(second_fixture.event("stale-control", "stale-message", "/stop"))
        .await
        .unwrap();
    let stale_runtime = create_runtime(&second_fixture).await;
    assert!(matches!(
        second_fixture
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: stale_runtime,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at(T1),
            })
            .await
            .unwrap(),
        DeliveryClaim::Dispatch(_)
    ));
    let root = second_fixture._root.path().to_owned();
    second_fixture.guard.shutdown().await;
    let restarted_guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let restarted_store = SqliteStateStore::new(restarted_guard.runtime().clone());
    let recovery = restarted_store
        .recover_stale_deliveries(RecoverDeliveriesRequest {
            recovered_at: at("2026-09-16T01:02:20.000000Z"),
        })
        .await
        .unwrap();
    assert_eq!(recovery.dispatches_marked_unknown, 1);
    let summaries = restarted_store
        .list_delivery_summaries(crate::ports::delivery_store::ListDeliverySummariesRequest {
            states: vec![OutboundDeliveryState::OutcomeUnknown],
            after: None,
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0].failure_class,
        Some(DeliveryFailureClass::StaleDispatch)
    );
    restarted_store.verify_delivery_consistency().await.unwrap();
    restarted_guard.shutdown().await;
}

#[tokio::test]
async fn externally_accepted_but_unpersisted_result_recovers_unknown_and_never_resends() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("lost-result-control", "lost-result-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let DeliveryClaim::Dispatch(dispatch) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("delivery claim expected");
    };
    let (invoked, _) = tokio::sync::mpsc::unbounded_channel();
    let adapter = FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Result(ChannelDispatchResult::Accepted {
            external_message_id: Some(ExternalMessageId::try_new("externally-accepted").unwrap()),
        }),
        calls: Arc::new(Mutex::new(Vec::new())),
        invoked,
    };
    assert!(matches!(
        adapter.dispatch(*dispatch).await,
        ChannelDispatchResult::Accepted { .. }
    ));
    // Deliberately omit result persistence to model the post-side-effect crash window.
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let restarted_guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let restarted_store = SqliteStateStore::new(restarted_guard.runtime().clone());
    restarted_store
        .recover_stale_deliveries(RecoverDeliveriesRequest {
            recovered_at: at("2026-09-16T01:02:20.000000Z"),
        })
        .await
        .unwrap();
    let mut connection = restarted_guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, failure_class, next_attempt_at FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "outcome_unknown");
    assert_eq!(row.get::<String, _>("failure_class"), "stale_dispatch");
    assert_eq!(row.get::<Option<String>, _>("next_attempt_at"), None);
    drop(connection);
    assert!(matches!(
        restarted_store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at("2026-09-16T01:03:00.000000Z"),
            })
            .await
            .unwrap(),
        DeliveryClaim::NoneDue
    ));
    restarted_guard.shutdown().await;
}

#[tokio::test]
async fn shutdown_latched_during_claim_reconciles_committed_attempt_without_adapter_io() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("claim-shutdown-control", "claim-shutdown-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Result(ChannelDispatchResult::Accepted {
            external_message_id: None,
        }),
        calls: Arc::clone(&calls),
        invoked,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (blocking, mut claim_entered) = BlockingDeliveryStore::new(inner, true, false, false);
    let store: Arc<dyn DeliveryStore> = blocking.clone();
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );

    claim_entered
        .recv()
        .await
        .expect("worker did not enter claim");
    let quiesced = worker.stop_claiming_and_wait();
    blocking.release_claim();
    quiesced.await;
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();

    assert!(calls.lock().unwrap().is_empty());
    assert!(invoked_rx.try_recv().is_err());
    assert!(!*fatal_rx.borrow());
    assert_eq!(blocking.claim_calls.load(Ordering::Acquire), 1);
    assert_eq!(blocking.interrupt_calls.load(Ordering::Acquire), 1);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT state, attempt_count, next_attempt_at, failure_class FROM outbound_deliveries",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("state"), "outcome_unknown");
    assert_eq!(row.get::<i64, _>("attempt_count"), 1);
    assert_eq!(row.get::<Option<String>, _>("next_attempt_at"), None);
    assert_eq!(
        row.get::<String, _>("failure_class"),
        "shutdown_interrupted"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM outbound_delivery_attempts \
             WHERE result_kind = 'outcome_unknown' AND failure_class = 'shutdown_interrupted' \
               AND completed_at IS NOT NULL",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    drop(connection);
    assert!(matches!(
        fixture
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at(T1),
            })
            .await
            .unwrap(),
        DeliveryClaim::NoneDue
    ));
    assert_eq!(
        fixture.store.verify_delivery_consistency().await.unwrap(),
        1
    );
}

#[tokio::test]
async fn shutdown_latch_prevents_new_claim_and_preserves_queued_delivery() {
    let fixture = fixture().await;
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Result(ChannelDispatchResult::Accepted {
            external_message_id: None,
        }),
        calls: Arc::clone(&calls),
        invoked,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (counting, _) = BlockingDeliveryStore::new(inner, false, false, false);
    let store: Arc<dyn DeliveryStore> = counting.clone();
    let notifier = DeliveryNotifier::new();
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, _) = tokio::sync::watch::channel(false);
    let mut worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        notifier.clone(),
        FixedJitter(0),
        fatal,
    );
    worker.wait_initial_scan().await.unwrap();
    assert_eq!(counting.claim_calls.load(Ordering::Acquire), 1);

    worker.stop_claiming_and_wait().await;
    fixture
        .service()
        .with_delivery(profiles, notifier)
        .classify(fixture.event("post-latch-control", "post-latch-message", "stop"))
        .await
        .unwrap();
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(counting.claim_calls.load(Ordering::Acquire), 1);
    assert!(calls.lock().unwrap().is_empty());
    assert!(invoked_rx.try_recv().is_err());
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, attempt_count FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "queued");
    assert_eq!(row.get::<i64, _>("attempt_count"), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbound_delivery_attempts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );
    drop(connection);
    assert_eq!(
        fixture.store.verify_delivery_consistency().await.unwrap(),
        1
    );
}

#[tokio::test]
async fn shutdown_while_idle_preserves_retry_wait_for_restart() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("retry-preserve-control", "retry-preserve-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let DeliveryClaim::Dispatch(dispatch) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("initial delivery claim expected");
    };
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::RetryableFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderRetryable, None)
                    .unwrap(),
                retry_after: Some(Duration::from_secs(30)),
            },
            local_retry_delay: Some(Duration::from_secs(1)),
            completed_at: at(T1),
        })
        .await
        .unwrap();
    let profiles = delivery_profiles(&fixture);
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            std::iter::empty::<(ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>(),
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (counting, _) = BlockingDeliveryStore::new(inner, false, false, false);
    let store: Arc<dyn DeliveryStore> = counting.clone();
    let clock = Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let worker_clock: Arc<dyn Clock> = clock.clone();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let mut worker = start_delivery_worker(
        store,
        adapters,
        worker_clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    worker.wait_initial_scan().await.unwrap();
    worker.stop_claiming_and_wait().await;
    clock.advance_wall(time::Duration::seconds(60)).unwrap();
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(counting.claim_calls.load(Ordering::Acquire), 1);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT state, attempt_count, next_attempt_at, failure_class FROM outbound_deliveries",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("state"), "retry_wait");
    assert_eq!(row.get::<i64, _>("attempt_count"), 1);
    assert!(row.get::<Option<String>, _>("next_attempt_at").is_some());
    assert_eq!(row.get::<Option<String>, _>("failure_class"), None);
    drop(connection);
    assert_eq!(
        fixture.store.verify_delivery_consistency().await.unwrap(),
        1
    );
}

#[tokio::test]
async fn claim_returning_none_after_shutdown_latch_exits_without_second_claim() {
    let fixture = fixture().await;
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            std::iter::empty::<(ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>(),
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (blocking, mut claim_entered) = BlockingDeliveryStore::new(inner, true, false, false);
    let store: Arc<dyn DeliveryStore> = blocking.clone();
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );

    claim_entered
        .recv()
        .await
        .expect("worker did not enter claim");
    let quiesced = worker.stop_claiming_and_wait();
    blocking.release_claim();
    quiesced.await;
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(blocking.claim_calls.load(Ordering::Acquire), 1);
    assert_eq!(blocking.interrupt_calls.load(Ordering::Acquire), 0);
    assert!(!*fatal_rx.borrow());
}

#[tokio::test]
async fn claim_error_after_shutdown_latch_is_joined_and_remains_fatal() {
    let fixture = fixture().await;
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            std::iter::empty::<(ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>(),
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (blocking, mut claim_entered) = BlockingDeliveryStore::new(inner, true, true, false);
    let store: Arc<dyn DeliveryStore> = blocking.clone();
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );

    claim_entered
        .recv()
        .await
        .expect("worker did not enter claim");
    let quiesced = worker.stop_claiming_and_wait();
    blocking.release_claim();
    quiesced.await;
    assert_eq!(
        worker
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
            .await,
        Err(DeliveryWorkerError::StateStore)
    );
    assert_eq!(blocking.claim_calls.load(Ordering::Acquire), 1);
    assert_eq!(blocking.interrupt_calls.load(Ordering::Acquire), 0);
    assert!(*fatal_rx.borrow());
}

#[tokio::test]
async fn worker_fallback_claims_lost_wake_after_durable_attempt_and_without_holding_sqlite() {
    let fixture = fixture().await;
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked_tx, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Result(ChannelDispatchResult::Accepted {
            external_message_id: Some(ExternalMessageId::try_new("accepted-1").unwrap()),
        }),
        calls: Arc::clone(&calls),
        invoked: invoked_tx,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let store: Arc<dyn DeliveryStore> = fixture.store.clone();
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    // Let the initial empty scan enter its fallback wait, then commit using a different
    // notifier to model an intent commit whose wake is lost.
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("worker-control", "worker-message", "stop"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), invoked_rx.recv())
        .await
        .expect("fallback scan did not invoke adapter")
        .expect("adapter signal channel closed");
    wait_for_delivery_state(&fixture, "accepted").await;
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].text, "Nothing is currently running or queued.");
        assert_eq!(
            calls[0].external_conversation_id,
            fixture.binding.external_conversation_id
        );
        assert_eq!(calls[0].attempt_number, 1);
    }
    assert!(!*fatal_rx.borrow());
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        fixture.store.verify_delivery_consistency().await.unwrap(),
        1
    );
}

#[tokio::test]
async fn retry_wait_due_time_is_recovered_by_fallback_without_a_wake() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("retry-wake-control", "retry-wake-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let DeliveryClaim::Dispatch(first) = fixture
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: runtime_id,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("first dispatch expected");
    };
    fixture
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch: first,
            runtime_instance_id: runtime_id,
            result: ChannelDispatchResult::RetryableFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderRetryable, None)
                    .unwrap(),
                retry_after: Some(Duration::from_secs(5)),
            },
            local_retry_delay: Some(Duration::from_secs(1)),
            completed_at: at(T1),
        })
        .await
        .unwrap();
    let profiles = delivery_profiles(&fixture);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked_tx, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Result(ChannelDispatchResult::Accepted {
            external_message_id: None,
        }),
        calls: Arc::clone(&calls),
        invoked: invoked_tx,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock = Arc::new(TestClock::new(
        at("2026-09-16T01:02:08.000000Z").to_offset_datetime(),
        Duration::ZERO,
    ));
    let worker_clock: Arc<dyn Clock> = clock.clone();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let store: Arc<dyn DeliveryStore> = fixture.store.clone();
    let worker = start_delivery_worker(
        store,
        adapters,
        worker_clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(calls.lock().unwrap().is_empty());
    clock.advance_wall(time::Duration::seconds(1)).unwrap();
    tokio::time::timeout(Duration::from_secs(3), invoked_rx.recv())
        .await
        .expect("fallback did not claim due retry")
        .expect("adapter invocation channel closed");
    wait_for_delivery_state(&fixture, "accepted").await;
    assert_eq!(calls.lock().unwrap()[0].attempt_number, 2);
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_adapter_is_permanent_and_adapter_panic_is_terminal_unknown() {
    let missing = fixture().await;
    missing
        .service_with_delivery_profile()
        .classify(missing.event("missing-control", "missing-message", "stop"))
        .await
        .unwrap();
    let missing_runtime = create_runtime(&missing).await;
    let profiles = delivery_profiles(&missing);
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            std::iter::empty::<(ChannelProviderId, Arc<dyn ChannelDeliveryAdapter>)>(),
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, _) = tokio::sync::watch::channel(false);
    let store: Arc<dyn DeliveryStore> = missing.store.clone();
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        missing_runtime,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    wait_for_delivery_state(&missing, "permanent_failure").await;
    let mut connection = missing.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT failure_class FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "adapter_unavailable"
    );
    drop(connection);
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    missing.store.verify_delivery_consistency().await.unwrap();

    let panicked = fixture().await;
    panicked
        .service_with_delivery_profile()
        .classify(panicked.event("panic-control", "panic-message", "stop"))
        .await
        .unwrap();
    let panic_runtime = create_runtime(&panicked).await;
    let profiles = delivery_profiles(&panicked);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked_tx, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: panicked.account.provider_id.clone(),
        runtime: panicked.guard.runtime().clone(),
        mode: FakeDispatchMode::Panic,
        calls,
        invoked: invoked_tx,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(panicked.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let store: Arc<dyn DeliveryStore> = panicked.store.clone();
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        panic_runtime,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    invoked_rx.recv().await.unwrap();
    wait_for_delivery_state(&panicked, "outcome_unknown").await;
    let mut connection = panicked.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT failure_class FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "provider_outcome_unknown"
    );
    drop(connection);
    assert!(!*fatal_rx.borrow());
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    panicked.store.verify_delivery_consistency().await.unwrap();
}

#[tokio::test]
async fn shutdown_deadline_marks_active_dispatch_unknown_before_aborting_adapter() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("shutdown-control", "shutdown-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let (invoked_tx, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter_dropped = Arc::new(AtomicBool::new(false));
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::PendingWithDrop(Arc::clone(&adapter_dropped)),
        calls: Arc::new(Mutex::new(Vec::new())),
        invoked: invoked_tx,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, _) = tokio::sync::watch::channel(false);
    let store: Arc<dyn DeliveryStore> = fixture.store.clone();
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    invoked_rx.recv().await.unwrap();
    let admitted = fixture
        .service_with_delivery_profile()
        .classify(fixture.event(
            "concurrent-work-event",
            "concurrent-work-message",
            "continue independently",
        ))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted { work_id, .. } = admitted.outcome else {
        panic!("ordinary work admission expected while delivery adapter is blocked");
    };
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .expect("agent scheduler claim must proceed while delivery is blocked");
    assert_eq!(claimed.work.work_id(), work_id);
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_millis(20))
        .await
        .unwrap();
    assert!(adapter_dropped.load(Ordering::Acquire));
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, failure_class FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "outcome_unknown");
    assert_eq!(
        row.get::<String, _>("failure_class"),
        "shutdown_interrupted"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM outbound_delivery_attempts WHERE completed_at IS NULL",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        0
    );
    drop(connection);
    fixture.store.verify_delivery_consistency().await.unwrap();
}

#[tokio::test]
async fn shutdown_clock_failure_preserves_error_after_aborting_and_joining_worker() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("clock-failure-control", "clock-failure-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let adapter_dropped = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::PendingWithDrop(Arc::clone(&adapter_dropped)),
        calls: Arc::clone(&calls),
        invoked,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(FailAfterFirstWallClock {
        wall: at(T1).to_offset_datetime(),
        calls: AtomicUsize::new(0),
    });
    let store: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    invoked_rx.recv().await.unwrap();

    assert_eq!(
        worker
            .shutdown_before(tokio::time::Instant::now() + Duration::from_millis(20))
            .await,
        Err(DeliveryWorkerError::Clock)
    );
    assert!(adapter_dropped.load(Ordering::Acquire));
    let calls_after_shutdown = calls.lock().unwrap().len();
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), calls_after_shutdown);
    assert_eq!(calls_after_shutdown, 1);
    assert_eq!(
        fixture.store.verify_delivery_consistency().await.unwrap(),
        1
    );
    fixture
        .store
        .interrupt_owned_delivery(ShutdownDeliveryRequest {
            runtime_instance_id: runtime_id,
            interrupted_at: at(T1),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn shutdown_storage_failure_preserves_error_after_aborting_and_joining_worker() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("storage-failure-control", "storage-failure-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let adapter_dropped = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::PendingWithDrop(Arc::clone(&adapter_dropped)),
        calls: Arc::clone(&calls),
        invoked,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let inner: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (failing, _) = BlockingDeliveryStore::new(inner, false, false, true);
    let store: Arc<dyn DeliveryStore> = failing.clone();
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let (fatal, _) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    invoked_rx.recv().await.unwrap();

    assert_eq!(
        worker
            .shutdown_before(tokio::time::Instant::now() + Duration::from_millis(20))
            .await,
        Err(DeliveryWorkerError::StateStore)
    );
    assert!(adapter_dropped.load(Ordering::Acquire));
    assert_eq!(failing.interrupt_calls.load(Ordering::Acquire), 1);
    let calls_after_shutdown = calls.lock().unwrap().len();
    tokio::task::yield_now().await;
    assert_eq!(calls.lock().unwrap().len(), calls_after_shutdown);
    assert_eq!(calls_after_shutdown, 1);
    assert_eq!(failing.verify_delivery_consistency().await.unwrap(), 1);
    fixture
        .store
        .interrupt_owned_delivery(ShutdownDeliveryRequest {
            runtime_instance_id: runtime_id,
            interrupted_at: at(T1),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn active_dispatch_finishing_before_shutdown_deadline_persists_normally_and_joins() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("finish-control", "finish-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let profiles = delivery_profiles(&fixture);
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (invoked, mut invoked_rx) = tokio::sync::mpsc::unbounded_channel();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(FakeDeliveryAdapter {
        provider: fixture.account.provider_id.clone(),
        runtime: fixture.guard.runtime().clone(),
        mode: FakeDispatchMode::Wait(
            Arc::clone(&release),
            ChannelDispatchResult::Accepted {
                external_message_id: None,
            },
        ),
        calls: Arc::clone(&calls),
        invoked,
    });
    let adapters = Arc::new(
        ChannelDeliveryAdapterRegistry::try_new(
            &profiles,
            [(fixture.account.provider_id.clone(), adapter)],
        )
        .unwrap(),
    );
    let clock: Arc<dyn Clock> =
        Arc::new(TestClock::new(at(T1).to_offset_datetime(), Duration::ZERO));
    let store: Arc<dyn DeliveryStore> = fixture.store.clone();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let worker = start_delivery_worker(
        store,
        adapters,
        clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedJitter(0),
        fatal,
    );
    invoked_rx.recv().await.unwrap();
    let shutdown =
        tokio::spawn(worker.shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1)));
    tokio::task::yield_now().await;
    release.notify_one();
    shutdown.await.unwrap().unwrap();

    assert_eq!(calls.lock().unwrap().len(), 1);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, failure_class FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "accepted");
    assert_eq!(row.get::<Option<String>, _>("failure_class"), None);
    drop(connection);
    fixture.store.verify_delivery_consistency().await.unwrap();
}

#[tokio::test]
async fn bounded_retry_exhaustion_and_deadline_terminalize_without_a_ninth_attempt() {
    let exhausted = fixture().await;
    exhausted
        .service_with_delivery_profile()
        .classify(exhausted.event("exhaust-control", "exhaust-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&exhausted).await;
    let mut now = at(T1);
    for expected_attempt in 1..=8 {
        let DeliveryClaim::Dispatch(dispatch) = exhausted
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now,
            })
            .await
            .unwrap()
        else {
            panic!("retry attempt {expected_attempt} was not claimable");
        };
        assert_eq!(dispatch.attempt_number, expected_attempt);
        exhausted
            .store
            .persist_dispatch_result(PersistDispatchResultRequest {
                dispatch,
                runtime_instance_id: runtime_id,
                result: ChannelDispatchResult::RetryableFailure {
                    failure: DeliveryFailure::provider(
                        DeliveryFailureClass::ProviderRetryable,
                        None,
                    )
                    .unwrap(),
                    retry_after: None,
                },
                local_retry_delay: Some(Duration::from_secs(1)),
                completed_at: now,
            })
            .await
            .unwrap();
        now = UtcTimestamp::from_offset_datetime(
            now.to_offset_datetime() + time::Duration::seconds(1),
        )
        .unwrap();
    }
    let mut connection = exhausted.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, failure_class, attempt_count FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "permanent_failure");
    assert_eq!(row.get::<String, _>("failure_class"), "retry_exhausted");
    assert_eq!(row.get::<i64, _>("attempt_count"), 8);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbound_delivery_attempts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        8
    );
    drop(connection);
    exhausted.store.verify_delivery_consistency().await.unwrap();
    assert!(matches!(
        exhausted
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now,
            })
            .await
            .unwrap(),
        DeliveryClaim::NoneDue
    ));

    let deadline = fixture().await;
    deadline
        .service_with_delivery_profile()
        .classify(deadline.event("deadline-control", "deadline-message", "stop"))
        .await
        .unwrap();
    let deadline_runtime = create_runtime(&deadline).await;
    let DeliveryClaim::Dispatch(dispatch) = deadline
        .store
        .claim_next_delivery(ClaimDeliveryRequest {
            runtime_instance_id: deadline_runtime,
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            now: at(T1),
        })
        .await
        .unwrap()
    else {
        panic!("deadline delivery was not claimable");
    };
    deadline
        .store
        .persist_dispatch_result(PersistDispatchResultRequest {
            dispatch,
            runtime_instance_id: deadline_runtime,
            result: ChannelDispatchResult::RetryableFailure {
                failure: DeliveryFailure::provider(DeliveryFailureClass::ProviderRetryable, None)
                    .unwrap(),
                retry_after: Some(Duration::from_secs(60 * 60)),
            },
            local_retry_delay: Some(Duration::from_secs(1)),
            completed_at: at(T1),
        })
        .await
        .unwrap();
    let mut connection = deadline.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query("SELECT state, failure_class FROM outbound_deliveries")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "permanent_failure");
    assert_eq!(
        row.get::<String, _>("failure_class"),
        "delivery_deadline_exceeded"
    );
    drop(connection);
    deadline.store.verify_delivery_consistency().await.unwrap();
}

#[tokio::test]
async fn revoked_binding_disabled_account_and_corruption_fail_closed_before_network() {
    let revoked = fixture().await;
    revoked
        .service_with_delivery_profile()
        .classify(revoked.event("revoked-control", "revoked-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&revoked).await;
    let mut connection = revoked.guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE conversation_bindings SET lifecycle_state = 'revoked', revoked_at = ? \
         WHERE conversation_binding_id = ?",
    )
    .bind(T1)
    .bind(revoked.binding.conversation_binding_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert!(matches!(
        revoked
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at(T1),
            })
            .await
            .unwrap(),
        DeliveryClaim::StateAdvanced
    ));
    let mut connection = revoked.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT failure_class FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "binding_revoked"
    );
    assert_eq!(table_count(&revoked, "outbound_delivery_attempts").await, 0);
    drop(connection);
    revoked.store.verify_delivery_consistency().await.unwrap();

    let disabled = fixture().await;
    disabled
        .service_with_delivery_profile()
        .classify(disabled.event("disabled-control", "disabled-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&disabled).await;
    let identity_store = SqliteChannelIdentityStore::new(disabled.guard.runtime().clone());
    assert!(matches!(
        identity_store
            .disable_channel_account(disabled.account.channel_account_id, at(T1))
            .await
            .unwrap(),
        DisableChannelAccountOutcome::Disabled(_)
    ));
    assert!(matches!(
        disabled
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at(T1),
            })
            .await
            .unwrap(),
        DeliveryClaim::StateAdvanced
    ));
    let mut connection = disabled.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT failure_class FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "channel_account_disabled"
    );
    drop(connection);
    disabled.store.verify_delivery_consistency().await.unwrap();

    let corrupted = fixture().await;
    corrupted
        .service_with_delivery_profile()
        .classify(corrupted.event("corrupt-control", "corrupt-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&corrupted).await;
    let mut connection = corrupted.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE outbound_deliveries SET payload_text = 'tampered'")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        corrupted
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: at(T1),
            })
            .await
            .unwrap_err()
            .kind(),
        DeliveryStoreErrorKind::Inconsistent
    );
    assert!(corrupted.store.verify_delivery_consistency().await.is_err());
}

#[tokio::test]
async fn concurrent_claim_is_single_winner_and_topology_corruption_is_detected() {
    let fixture = fixture().await;
    fixture
        .service_with_delivery_profile()
        .classify(fixture.event("race-control", "race-message", "stop"))
        .await
        .unwrap();
    let runtime_id = create_runtime(&fixture).await;
    let first = fixture.store.claim_next_delivery(ClaimDeliveryRequest {
        runtime_instance_id: runtime_id,
        outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
        now: at(T1),
    });
    let second = fixture.store.claim_next_delivery(ClaimDeliveryRequest {
        runtime_instance_id: runtime_id,
        outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
        now: at(T1),
    });
    let (first, second) = tokio::join!(first, second);
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(
        claims
            .iter()
            .filter(|claim| matches!(claim, DeliveryClaim::Dispatch(_)))
            .count(),
        1
    );
    assert_eq!(
        claims
            .iter()
            .filter(|claim| matches!(claim, DeliveryClaim::NoneDue))
            .count(),
        1
    );
    assert_eq!(table_count(&fixture, "outbound_delivery_attempts").await, 1);

    let corrupt = self::fixture().await;
    corrupt
        .service_with_delivery_profile()
        .classify(corrupt.event("topology-control", "topology-message", "stop"))
        .await
        .unwrap();
    let mut connection = corrupt.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE outbound_deliveries SET part_count = 2")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(corrupt.store.verify_delivery_consistency().await.is_err());
}

#[test]
fn generic_ingress_sources_remain_provider_neutral_and_outbound_free() {
    let sources = [
        include_str!("../../application/channel_ingress.rs"),
        include_str!("../../application/control_message_policy.rs"),
        include_str!("../../ports/channel_ingress.rs"),
        include_str!("channel_ingress.rs"),
    ]
    .join("\n");
    for forbidden in [
        "Telegram",
        "update_id",
        "chat_id",
        "WAMID",
        "wa_id",
        "Graph API",
        "Bot API",
        "Slack event",
        "Discord gateway",
        "webhook",
        "polling offset",
        "outbound_deliveries",
        "delivery worker",
    ] {
        assert!(
            !sources.contains(forbidden),
            "found forbidden term: {forbidden}"
        );
    }
}
