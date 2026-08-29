use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::application::scheduler::{SchedulerError, SchedulerHandle};
use crate::bootstrap::health::{FatalReasonCode, Health, HealthState};
use crate::domain::{
    CorrelationId, JournalEventId, RuntimeInstanceId, RuntimeRecoveryPerformedV1,
    RuntimeShutdownReason, RuntimeStartEvidence, RuntimeStoppingV1, UtcTimestamp,
};
use crate::ports::clock::Clock;
use crate::ports::state_store::{
    AppendRecoverySummaryRequest, BeginRuntimeStoppingRequest, ClassifyShutdownWorkRequest,
    CreateRuntimeRequest, EnumerateStaleRuntimesRequest, FinishRuntimeRequest,
    HeartbeatRuntimeRequest, RecoverStaleRuntimeRequest, RecoveryStateStore,
    RequestOwnedCancellationRequest, RuntimeStateStore, SchedulerStateStore, StateStoreError,
};

pub const HEARTBEAT_CADENCE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBootstrapReceipt {
    pub runtime_instance_id: RuntimeInstanceId,
    pub started_event_id: JournalEventId,
    pub correlation_id: CorrelationId,
    pub recovery: RuntimeRecoveryPerformedV1,
}

pub async fn bootstrap_runtime<S: RuntimeStateStore + RecoveryStateStore>(
    store: &S,
    evidence: RuntimeStartEvidence,
    orphan_artifacts_observed: u64,
    clock: &dyn Clock,
) -> Result<RuntimeBootstrapReceipt, RuntimeControlError> {
    let runtime_instance_id = evidence.runtime_instance_id();
    let started_event_id = JournalEventId::generate();
    let correlation_id = CorrelationId::generate();
    let binary_version = evidence.package_version().clone();
    let schema_version = evidence.schema_version();
    let recovery_started = clock.monotonic_now();
    store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence,
            event_id: started_event_id,
            correlation_id,
        })
        .await?;

    let result = async {
        let stale = store
            .enumerate_stale_runtimes(EnumerateStaleRuntimesRequest {
                current_runtime_id: runtime_instance_id,
            })
            .await?;
        let retained_queued_work = store.count_retained_queued_work().await?;
        let mut summary = RuntimeRecoveryPerformedV1 {
            runtime_instance_id,
            stale_runtimes_observed: stale.len() as u64,
            stale_runtimes_closed: 0,
            retained_queued_work,
            interrupted_work: 0,
            model_attempts_provider_outcome_unknown: 0,
            model_attempts_terminal_preserved: 0,
            tool_attempts_interrupted_before_dispatch: 0,
            tool_attempts_outcome_unknown: 0,
            tool_attempts_terminal_preserved: 0,
            drafts_abandoned: 0,
            orphan_artifacts_observed,
            cleanup_checks_performed: 0,
            cleanup_unconfirmed: 0,
            recovery_duration_ms: 0,
            binary_version,
            schema_version,
            recovered_at: now(clock)?,
        };
        for stale_runtime_id in stale {
            let receipt = store
                .recover_stale_runtime_ownership(RecoverStaleRuntimeRequest {
                    stale_runtime_id,
                    current_runtime_id: runtime_instance_id,
                    recovered_at: now(clock)?,
                })
                .await?;
            summary.stale_runtimes_closed += u64::from(receipt.stale_runtime_closed);
            summary.interrupted_work += receipt.interrupted_work;
            summary.model_attempts_provider_outcome_unknown +=
                receipt.model_attempts_provider_outcome_unknown;
            summary.model_attempts_terminal_preserved += receipt.model_attempts_terminal_preserved;
            summary.tool_attempts_interrupted_before_dispatch +=
                receipt.tool_attempts_interrupted_before_dispatch;
            summary.tool_attempts_outcome_unknown += receipt.tool_attempts_outcome_unknown;
            summary.tool_attempts_terminal_preserved += receipt.tool_attempts_terminal_preserved;
            summary.drafts_abandoned += receipt.drafts_abandoned;
            summary.cleanup_checks_performed += receipt.cleanup_checks_performed;
            summary.cleanup_unconfirmed += receipt.cleanup_unconfirmed;
        }
        summary.recovered_at = now(clock)?;
        summary.recovery_duration_ms = clock
            .monotonic_now()
            .checked_duration_since(recovery_started)
            .ok_or(RuntimeControlError::Clock)?
            .as_millis()
            .try_into()
            .map_err(|_| RuntimeControlError::Clock)?;
        store
            .append_recovery_summary(AppendRecoverySummaryRequest {
                summary: summary.clone(),
                event_id: JournalEventId::generate(),
                started_event_id,
                correlation_id,
            })
            .await?;
        Ok::<_, RuntimeControlError>(summary)
    }
    .await;

    match result {
        Ok(recovery) => Ok(RuntimeBootstrapReceipt {
            runtime_instance_id,
            started_event_id,
            correlation_id,
            recovery,
        }),
        Err(original) => {
            let _ = store
                .mark_runtime_startup_failure(FinishRuntimeRequest {
                    runtime_instance_id,
                    stopped_at: now(clock).unwrap_or_else(|_| {
                        UtcTimestamp::parse_canonical("1970-01-01T00:00:00.000000Z")
                            .expect("fixed timestamp")
                    }),
                })
                .await;
            Err(original)
        }
    }
}

fn now(clock: &dyn Clock) -> Result<UtcTimestamp, RuntimeControlError> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| RuntimeControlError::Clock)?)
        .map_err(|_| RuntimeControlError::Clock)
}

pub struct HeartbeatTask {
    stop: tokio::sync::watch::Sender<bool>,
    join: tokio::task::JoinHandle<Result<(), RuntimeControlError>>,
}

impl HeartbeatTask {
    pub fn start<S, C>(
        store: Arc<S>,
        clock: Arc<C>,
        health: Health,
        runtime_instance_id: RuntimeInstanceId,
        fatal: tokio::sync::watch::Sender<bool>,
    ) -> Self
    where
        S: RuntimeStateStore + 'static,
        C: Clock + 'static,
    {
        Self::start_with_cadence(
            store,
            clock,
            health,
            runtime_instance_id,
            fatal,
            HEARTBEAT_CADENCE,
        )
    }

    pub(crate) fn start_with_cadence<S, C>(
        store: Arc<S>,
        clock: Arc<C>,
        health: Health,
        runtime_instance_id: RuntimeInstanceId,
        fatal: tokio::sync::watch::Sender<bool>,
        cadence: Duration,
    ) -> Self
    where
        S: RuntimeStateStore + 'static,
        C: Clock + 'static,
    {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(cadence);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                tokio::select! {
                    biased;
                    result = stopped.changed() => {
                        if result.is_err() || *stopped.borrow() {
                            return Ok(());
                        }
                    }
                    _ = interval.tick() => {
                        let observed_at = now(clock.as_ref())?;
                        if let Err(error) = store.heartbeat_runtime(HeartbeatRuntimeRequest {
                            runtime_instance_id,
                            observed_at,
                        }).await {
                            if *stopped.borrow() {
                                return Ok(());
                            }
                            let _ = health.mark_fatal(FatalReasonCode::Internal);
                            let _ = fatal.send(true);
                            return Err(error.into());
                        }
                    }
                }
            }
        });
        Self { stop, join }
    }

    pub async fn stop_and_join(self) -> Result<(), RuntimeControlError> {
        let _ = self.stop.send(true);
        self.join
            .await
            .map_err(|_| RuntimeControlError::TaskJoin)??;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownReceipt {
    pub shutdown_requested_at: UtcTimestamp,
    pub grace_deadline: UtcTimestamp,
    pub began: bool,
}

pub struct ShutdownController<S, C> {
    store: Arc<S>,
    clock: Arc<C>,
    health: Health,
    runtime_instance_id: RuntimeInstanceId,
    correlation_id: CorrelationId,
    grace_period_ms: u64,
    requested: std::sync::atomic::AtomicBool,
    state: tokio::sync::Mutex<Option<ShutdownState>>,
    failure: tokio::sync::Mutex<Option<RuntimeControlError>>,
    heartbeat: tokio::sync::Mutex<Option<HeartbeatTask>>,
    scheduler: tokio::sync::Mutex<Option<SchedulerHandle>>,
}

pub type ControlledShutdownFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ShutdownReceipt, RuntimeControlError>> + Send + 'a>>;

/// Object-safe access to the one Stage 10 controlled-shutdown latch.
///
/// Transport supervisors use this narrow view so an unexpected shared-server failure requests
/// the existing runtime shutdown instead of creating a second lifecycle authority.
pub trait ControlledShutdown: Send + Sync {
    fn request_controlled_shutdown(&self) -> ControlledShutdownFuture<'_>;

    /// Reports the actual Stage 10 shutdown latch, not transport-local drain state.
    fn shutdown_is_requested(&self) -> bool;
}

#[derive(Clone, Copy)]
struct ShutdownState {
    receipt: ShutdownReceipt,
    monotonic_deadline: tokio::time::Instant,
}

impl<S, C> ShutdownController<S, C>
where
    S: RuntimeStateStore + SchedulerStateStore + RecoveryStateStore + 'static,
    C: Clock + 'static,
{
    #[must_use]
    pub fn new(
        store: Arc<S>,
        clock: Arc<C>,
        health: Health,
        runtime_instance_id: RuntimeInstanceId,
        correlation_id: CorrelationId,
        grace_period_ms: u64,
        heartbeat: HeartbeatTask,
    ) -> Self {
        Self {
            store,
            clock,
            health,
            runtime_instance_id,
            correlation_id,
            grace_period_ms,
            requested: std::sync::atomic::AtomicBool::new(false),
            state: tokio::sync::Mutex::new(None),
            failure: tokio::sync::Mutex::new(None),
            heartbeat: tokio::sync::Mutex::new(Some(heartbeat)),
            scheduler: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn install_scheduler(
        &self,
        scheduler: SchedulerHandle,
    ) -> Result<(), RuntimeControlError> {
        if self.state.lock().await.is_some() {
            return Err(RuntimeControlError::InvalidShutdown);
        }
        let mut installed = self.scheduler.lock().await;
        if installed.is_some() {
            return Err(RuntimeControlError::InvalidShutdown);
        }
        *installed = Some(scheduler);
        Ok(())
    }

    /// Latches the one Stage 10 shutdown request before outward transports are told to exit.
    ///
    /// The later async `request` work still owns claim quiescence, the durable stopping event,
    /// heartbeat join, and Work cancellation. Splitting this nonblocking latch lets bootstrap stop
    /// listener acceptance without creating a transport-local expected-exit authority.
    pub fn latch_shutdown_request(&self) {
        self.requested
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub async fn request(&self) -> Result<ShutdownReceipt, RuntimeControlError> {
        self.latch_shutdown_request();
        let mut state = self.state.lock().await;
        if let Some(existing) = *state {
            return Ok(ShutdownReceipt {
                began: false,
                ..existing.receipt
            });
        }
        let shutdown_requested_at = now(self.clock.as_ref())?;
        let monotonic_deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.grace_period_ms);
        let milliseconds =
            i64::try_from(self.grace_period_ms).map_err(|_| RuntimeControlError::Clock)?;
        let wall_deadline = shutdown_requested_at
            .to_offset_datetime()
            .checked_add(time::Duration::milliseconds(milliseconds))
            .ok_or(RuntimeControlError::Clock)?;
        let grace_deadline = UtcTimestamp::from_offset_datetime(wall_deadline)
            .map_err(|_| RuntimeControlError::Clock)?;
        match self.health.snapshot().state() {
            HealthState::LiveUnready | HealthState::Ready => {
                if self.health.mark_draining().is_err() {
                    self.record_failure(RuntimeControlError::Health).await;
                }
            }
            HealthState::Draining | HealthState::Fatal => {}
        }
        if let Some(scheduler) = self.scheduler.lock().await.as_ref() {
            scheduler.stop_claiming_and_wait().await;
        }
        let active_task_count = self
            .scheduler
            .lock()
            .await
            .as_ref()
            .map_or(0, |scheduler| scheduler.registry().snapshot().len() as u64);
        let event = RuntimeStoppingV1 {
            runtime_instance_id: self.runtime_instance_id,
            shutdown_requested_at,
            shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
            grace_deadline,
            active_work_count: active_task_count,
            active_task_count,
        };
        let stopping = self
            .store
            .begin_runtime_stopping(BeginRuntimeStoppingRequest {
                event,
                event_id: JournalEventId::generate(),
                correlation_id: self.correlation_id,
            })
            .await;
        match stopping {
            Ok(_) => {
                #[cfg(feature = "test-failpoints")]
                crate::test_failpoints::reach(
                    crate::test_failpoints::PhysicalHook::DuringGracefulShutdown,
                );
            }
            Err(_) => self.record_failure(RuntimeControlError::StateStore).await,
        }
        let receipt = ShutdownReceipt {
            shutdown_requested_at,
            grace_deadline,
            began: true,
        };
        *state = Some(ShutdownState {
            receipt,
            monotonic_deadline,
        });
        drop(state);

        if let Some(heartbeat) = self.heartbeat.lock().await.take()
            && let Err(error) = heartbeat.stop_and_join().await
        {
            self.record_failure(error).await;
        }
        let cancellation = self
            .store
            .request_owned_work_cancellation(RequestOwnedCancellationRequest {
                runtime_id: self.runtime_instance_id,
                requested_at: shutdown_requested_at,
            })
            .await;
        match cancellation {
            Ok(_) => {
                if let Some(scheduler) = self.scheduler.lock().await.as_ref() {
                    scheduler.begin_shutdown();
                }
            }
            Err(_) => self.record_failure(RuntimeControlError::StateStore).await,
        }
        Ok(receipt)
    }

    pub async fn monotonic_deadline(&self) -> Result<tokio::time::Instant, RuntimeControlError> {
        self.state
            .lock()
            .await
            .map(|state| state.monotonic_deadline)
            .ok_or(RuntimeControlError::InvalidShutdown)
    }

    pub async fn finish(&self) -> Result<(), RuntimeControlError> {
        let shutdown = (*self.state.lock().await).ok_or(RuntimeControlError::InvalidShutdown)?;
        let mut classified_before_abort = false;
        let mut ownership_accounted = true;
        if let Some(mut scheduler) = self.scheduler.lock().await.take() {
            match scheduler.join_before(shutdown.monotonic_deadline).await {
                Ok(true) => {}
                Err(_) => self.record_failure(RuntimeControlError::TaskJoin).await,
                Ok(false) => {
                    if scheduler.prepare_deadline_and_wait().await.is_err() {
                        if scheduler.join_to_completion().await.is_err() {
                            self.record_failure(RuntimeControlError::TaskJoin).await;
                        }
                    } else {
                        match self
                            .store
                            .classify_unresolved_shutdown_work(ClassifyShutdownWorkRequest {
                                runtime_id: self.runtime_instance_id,
                                classified_at: now(self.clock.as_ref())?,
                            })
                            .await
                        {
                            Ok(_) => {
                                classified_before_abort = true;
                                if scheduler.abort_runners_and_join().await.is_err() {
                                    self.record_failure(RuntimeControlError::TaskJoin).await;
                                }
                            }
                            Err(_) => {
                                ownership_accounted = false;
                                self.record_failure(RuntimeControlError::StateStore).await;
                            }
                        }
                    }
                }
            }
        }
        if !classified_before_abort
            && self
                .store
                .classify_unresolved_shutdown_work(ClassifyShutdownWorkRequest {
                    runtime_id: self.runtime_instance_id,
                    classified_at: now(self.clock.as_ref())?,
                })
                .await
                .is_err()
        {
            ownership_accounted = false;
            self.record_failure(RuntimeControlError::StateStore).await;
        }
        if ownership_accounted
            && self
                .store
                .finish_runtime_graceful(FinishRuntimeRequest {
                    runtime_instance_id: self.runtime_instance_id,
                    stopped_at: now(self.clock.as_ref())?,
                })
                .await
                .is_err()
        {
            self.record_failure(RuntimeControlError::StateStore).await;
        }
        self.failure.lock().await.map_or(Ok(()), Err)
    }

    async fn record_failure(&self, error: RuntimeControlError) {
        let mut failure = self.failure.lock().await;
        failure.get_or_insert(error);
    }
}

impl<S, C> ControlledShutdown for ShutdownController<S, C>
where
    S: RuntimeStateStore + SchedulerStateStore + RecoveryStateStore + 'static,
    C: Clock + 'static,
{
    fn request_controlled_shutdown(&self) -> ControlledShutdownFuture<'_> {
        Box::pin(self.request())
    }

    fn shutdown_is_requested(&self) -> bool {
        self.requested.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeControlError {
    StateStore,
    Clock,
    Health,
    TaskJoin,
    InvalidShutdown,
}

impl From<StateStoreError> for RuntimeControlError {
    fn from(_: StateStoreError) -> Self {
        Self::StateStore
    }
}

impl From<SchedulerError> for RuntimeControlError {
    fn from(_: SchedulerError) -> Self {
        Self::TaskJoin
    }
}

impl std::fmt::Display for RuntimeControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StateStore => "runtime state store failure",
            Self::Clock => "runtime clock failure",
            Self::Health => "runtime health transition failure",
            Self::TaskJoin => "runtime task join failure",
            Self::InvalidShutdown => "runtime shutdown was not requested",
        })
    }
}

impl std::error::Error for RuntimeControlError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::health::HealthState;
    use crate::ports::clock::TestClock;
    use crate::ports::state_store::{
        BeginRuntimeStoppingReceipt, CreateRuntimeReceipt, CreateRuntimeRequest,
        EnumerateStaleRuntimesRequest, HeartbeatRuntimeReceipt, StateStoreErrorKind,
        StateStoreFuture,
    };

    struct FailingHeartbeatStore;

    fn failed<T>() -> StateStoreFuture<'static, T> {
        Box::pin(async { Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant)) })
    }

    impl RuntimeStateStore for FailingHeartbeatStore {
        fn create_runtime_and_started_event(
            &self,
            _: CreateRuntimeRequest,
        ) -> StateStoreFuture<'_, CreateRuntimeReceipt> {
            failed()
        }

        fn heartbeat_runtime(
            &self,
            _: HeartbeatRuntimeRequest,
        ) -> StateStoreFuture<'_, HeartbeatRuntimeReceipt> {
            failed()
        }

        fn begin_runtime_stopping(
            &self,
            _: BeginRuntimeStoppingRequest,
        ) -> StateStoreFuture<'_, BeginRuntimeStoppingReceipt> {
            failed()
        }

        fn finish_runtime_graceful(
            &self,
            _: FinishRuntimeRequest,
        ) -> StateStoreFuture<'_, crate::ports::state_store::CommitReceipt> {
            failed()
        }

        fn mark_runtime_startup_failure(
            &self,
            _: FinishRuntimeRequest,
        ) -> StateStoreFuture<'_, crate::ports::state_store::CommitReceipt> {
            failed()
        }

        fn enumerate_stale_runtimes(
            &self,
            _: EnumerateStaleRuntimesRequest,
        ) -> StateStoreFuture<'_, Vec<RuntimeInstanceId>> {
            failed()
        }

        fn append_recovery_summary(
            &self,
            _: AppendRecoverySummaryRequest,
        ) -> StateStoreFuture<'_, crate::ports::state_store::CommitReceipt> {
            failed()
        }
    }

    #[tokio::test]
    async fn persistent_heartbeat_storage_failure_is_fatal_and_joined() {
        let health = Health::new();
        let (fatal, mut observed) = tokio::sync::watch::channel(false);
        let clock = Arc::new(TestClock::new(
            time::OffsetDateTime::from_unix_timestamp(1_777_000_000).unwrap(),
            Duration::ZERO,
        ));
        let task = HeartbeatTask::start_with_cadence(
            Arc::new(FailingHeartbeatStore),
            clock,
            health.clone(),
            RuntimeInstanceId::generate(),
            fatal,
            Duration::from_millis(1),
        );
        tokio::time::timeout(Duration::from_secs(1), observed.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*observed.borrow());
        assert_eq!(health.snapshot().state(), HealthState::Fatal);
        assert_eq!(
            task.stop_and_join().await,
            Err(RuntimeControlError::StateStore)
        );
    }
}
