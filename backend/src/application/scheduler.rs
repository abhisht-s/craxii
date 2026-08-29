use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::application::command_service::CommandPostCommit;
use crate::bootstrap::health::{FatalReasonCode, Health};
use crate::domain::{
    ConversationId, CurrentWorkAttempt, JournalEventId, JournalOffset, RuntimeInstanceId,
    UtcTimestamp, WorkId,
};
use crate::ports::clock::Clock;
use crate::ports::state_store::{
    ClaimNextWorkRequest, ClaimedWork, FinishCancellationRequest, InterruptOwnedWorkRequest,
    SchedulerStateStore, StateStoreError,
};

pub const SCHEDULER_FALLBACK_SCAN: Duration = Duration::from_secs(1);

pub type WorkRunnerFuture = Pin<Box<dyn Future<Output = WorkRunnerExit> + Send + 'static>>;

pub trait WorkRunner: Send + Sync + 'static {
    fn start(
        &self,
        work: ClaimedWork,
        cancellation: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkRunnerStartError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkRunnerExit {
    CancellationConfirmed,
    Abnormal,
}

pub struct WorkCancellation {
    receiver: tokio::sync::watch::Receiver<bool>,
}

impl WorkCancellation {
    #[must_use]
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn requested(&mut self) {
        while !*self.receiver.borrow() {
            if self.receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Clone)]
pub struct SchedulerNotifier {
    notify: Arc<tokio::sync::Notify>,
}

impl SchedulerNotifier {
    pub fn wake(&self) {
        self.notify.notify_one();
    }
}

impl CommandPostCommit for SchedulerNotifier {
    fn message_committed(&self, _: WorkId, _: JournalOffset) {
        self.wake();
    }

    fn active_cancellation_committed(&self, _: WorkId, _: JournalOffset) {
        self.wake();
    }

    fn direct_cancellation_committed(&self, _: WorkId, _: JournalOffset) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistryEntrySnapshot {
    pub work_id: WorkId,
    pub runtime_instance_id: RuntimeInstanceId,
    pub cancellation_requested: bool,
}

struct RegistryEntry {
    runtime_instance_id: RuntimeInstanceId,
    cancellation: tokio::sync::watch::Sender<bool>,
    task_id: tokio::task::Id,
}

#[derive(Clone, Default)]
pub struct TaskRegistryView {
    entries: Arc<Mutex<HashMap<WorkId, RegistryEntrySnapshot>>>,
}

impl TaskRegistryView {
    #[must_use]
    pub fn snapshot(&self) -> Vec<RegistryEntrySnapshot> {
        match self.entries.lock() {
            Ok(entries) => entries.values().copied().collect(),
            Err(poisoned) => poisoned.into_inner().values().copied().collect(),
        }
    }
}

pub struct SchedulerHandle {
    notifier: SchedulerNotifier,
    control: tokio::sync::mpsc::UnboundedSender<SchedulerCommand>,
    join: tokio::task::JoinHandle<Result<(), SchedulerError>>,
    registry: TaskRegistryView,
    claiming: Arc<AtomicBool>,
    claim_gate: Arc<tokio::sync::Mutex<()>>,
}

enum SchedulerCommand {
    BeginShutdown,
    PrepareDeadline(tokio::sync::oneshot::Sender<()>),
    AbortDurablyClassified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerStart {
    pub runtime_instance_id: RuntimeInstanceId,
    pub conversation_id: ConversationId,
    pub allow_test_ready: bool,
}

impl SchedulerHandle {
    #[must_use]
    pub fn notifier(&self) -> SchedulerNotifier {
        self.notifier.clone()
    }

    #[must_use]
    pub fn registry(&self) -> TaskRegistryView {
        self.registry.clone()
    }

    pub fn stop_claiming_and_wait(&self) -> impl Future<Output = ()> + Send + 'static {
        self.claiming.store(false, Ordering::Release);
        self.notifier.wake();
        let claim_gate = Arc::clone(&self.claim_gate);
        async move {
            let quiesced = claim_gate.lock().await;
            drop(quiesced);
        }
    }

    pub fn begin_shutdown(&self) {
        let _ = self.control.send(SchedulerCommand::BeginShutdown);
        self.notifier.wake();
    }

    pub async fn prepare_deadline_and_wait(&self) -> Result<(), SchedulerError> {
        let (acknowledged, wait) = tokio::sync::oneshot::channel();
        self.control
            .send(SchedulerCommand::PrepareDeadline(acknowledged))
            .map_err(|_| SchedulerError::TaskJoin)?;
        self.notifier.wake();
        wait.await.map_err(|_| SchedulerError::TaskJoin)
    }

    pub async fn stop_and_join(self) -> Result<(), SchedulerError> {
        self.stop_claiming_and_wait().await;
        self.begin_shutdown();
        self.join.await.map_err(|_| SchedulerError::TaskJoin)??;
        Ok(())
    }

    pub async fn join_before(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<bool, SchedulerError> {
        tokio::select! {
            result = &mut self.join => {
                result.map_err(|_| SchedulerError::TaskJoin)??;
                Ok(true)
            }
            () = tokio::time::sleep_until(deadline) => Ok(false),
        }
    }

    pub async fn join_to_completion(&mut self) -> Result<(), SchedulerError> {
        (&mut self.join)
            .await
            .map_err(|_| SchedulerError::TaskJoin)??;
        Ok(())
    }

    pub async fn abort_runners_and_join(self) -> Result<(), SchedulerError> {
        self.control
            .send(SchedulerCommand::AbortDurablyClassified)
            .map_err(|_| SchedulerError::TaskJoin)?;
        self.notifier.wake();
        self.join.await.map_err(|_| SchedulerError::TaskJoin)??;
        Ok(())
    }
}

pub fn start_scheduler<S, R, C>(
    store: Arc<S>,
    runner: Arc<R>,
    clock: Arc<C>,
    health: Health,
    fatal: tokio::sync::watch::Sender<bool>,
    start: SchedulerStart,
) -> Result<SchedulerHandle, SchedulerError>
where
    S: SchedulerStateStore + 'static,
    R: WorkRunner,
    C: Clock + 'static,
{
    let notify = Arc::new(tokio::sync::Notify::new());
    let notifier = SchedulerNotifier {
        notify: Arc::clone(&notify),
    };
    let (control, commands) = tokio::sync::mpsc::unbounded_channel();
    let registry = TaskRegistryView::default();
    let claiming = Arc::new(AtomicBool::new(true));
    let claim_gate = Arc::new(tokio::sync::Mutex::new(()));
    let loop_registry = registry.clone();
    if start.allow_test_ready {
        health.mark_ready().map_err(|_| SchedulerError::Health)?;
    }
    let join = tokio::spawn(run_scheduler(
        store,
        runner,
        clock,
        health,
        start.runtime_instance_id,
        start.conversation_id,
        notify,
        commands,
        fatal,
        loop_registry,
        Arc::clone(&claiming),
        Arc::clone(&claim_gate),
    ));
    notifier.wake();
    Ok(SchedulerHandle {
        notifier,
        control,
        join,
        registry,
        claiming,
        claim_gate,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_scheduler<S, R, C>(
    store: Arc<S>,
    runner: Arc<R>,
    clock: Arc<C>,
    health: Health,
    runtime_instance_id: RuntimeInstanceId,
    conversation_id: ConversationId,
    notify: Arc<tokio::sync::Notify>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<SchedulerCommand>,
    fatal: tokio::sync::watch::Sender<bool>,
    registry_view: TaskRegistryView,
    claiming: Arc<AtomicBool>,
    claim_gate: Arc<tokio::sync::Mutex<()>>,
) -> Result<(), SchedulerError>
where
    S: SchedulerStateStore + 'static,
    R: WorkRunner,
    C: Clock + 'static,
{
    let mut scan = tokio::time::interval(SCHEDULER_FALLBACK_SCAN);
    scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tasks = tokio::task::JoinSet::<(WorkId, WorkRunnerExit)>::new();
    let mut registry = HashMap::<WorkId, RegistryEntry>::new();
    let mut task_to_work = HashMap::<tokio::task::Id, WorkId>::new();
    let mut shutting_down = false;
    let mut fatal_error = None;
    let mut deadline_frozen = false;
    let mut control_open = true;

    loop {
        if shutting_down && tasks.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            command = commands.recv(), if control_open => {
                match command {
                    Some(SchedulerCommand::BeginShutdown) => {
                        shutting_down = true;
                        for entry in registry.values() {
                            let _ = entry.cancellation.send(true);
                        }
                        update_view(&registry, &registry_view);
                        notify.notify_one();
                    }
                    Some(SchedulerCommand::PrepareDeadline(acknowledged)) => {
                        deadline_frozen = true;
                        let _ = acknowledged.send(());
                    }
                    Some(SchedulerCommand::AbortDurablyClassified) => {
                    let mut classified_tasks = task_to_work.keys().copied().collect::<HashSet<_>>();
                    tasks.abort_all();
                    while let Some(joined) = tasks.join_next_with_id().await {
                        observe_join(
                            Some(joined),
                            store.as_ref(),
                            clock.as_ref(),
                            runtime_instance_id,
                            &mut registry,
                            &mut task_to_work,
                            &registry_view,
                            JoinDisposition::AlreadyDurablyClassified(&mut classified_tasks),
                        ).await?;
                    }
                    if !classified_tasks.is_empty() || !registry.is_empty() || !task_to_work.is_empty() {
                        return Err(SchedulerError::Invariant);
                    }
                    break;
                    }
                    None => {
                        control_open = false;
                        claiming.store(false, Ordering::Release);
                        fatal_error.get_or_insert(SchedulerError::TaskJoin);
                    }
                }
            }
            joined = tasks.join_next_with_id(), if !deadline_frozen && !tasks.is_empty() => {
                if let Err(error) = observe_join(
                    joined,
                    store.as_ref(),
                    clock.as_ref(),
                    runtime_instance_id,
                    &mut registry,
                    &mut task_to_work,
                    &registry_view,
                    JoinDisposition::ReconcileDurably,
                ).await {
                    claiming.store(false, Ordering::Release);
                    let _ = health.mark_fatal(FatalReasonCode::Internal);
                    let _ = fatal.send(true);
                    fatal_error.get_or_insert(error);
                }
                notify.notify_one();
            }
            () = notify.notified(), if !deadline_frozen && fatal_error.is_none() => {
                if let Err(error) = scan_once(
                    store.as_ref(),
                    runner.as_ref(),
                    clock.as_ref(),
                    runtime_instance_id,
                    conversation_id,
                    &mut tasks,
                    &mut registry,
                    &mut task_to_work,
                    &registry_view,
                    claiming.as_ref(),
                    claim_gate.as_ref(),
                ).await {
                    claiming.store(false, Ordering::Release);
                    let _ = health.mark_fatal(FatalReasonCode::Internal);
                    let _ = fatal.send(true);
                    fatal_error = Some(error);
                }
            }
            _ = scan.tick(), if !deadline_frozen && fatal_error.is_none() => {
                if let Err(error) = scan_once(
                    store.as_ref(),
                    runner.as_ref(),
                    clock.as_ref(),
                    runtime_instance_id,
                    conversation_id,
                    &mut tasks,
                    &mut registry,
                    &mut task_to_work,
                    &registry_view,
                    claiming.as_ref(),
                    claim_gate.as_ref(),
                ).await {
                    claiming.store(false, Ordering::Release);
                    let _ = health.mark_fatal(FatalReasonCode::Internal);
                    let _ = fatal.send(true);
                    fatal_error = Some(error);
                }
            }
        }
    }
    fatal_error.map_or(Ok(()), Err)
}

#[allow(clippy::too_many_arguments)]
async fn scan_once<S, R, C>(
    store: &S,
    runner: &R,
    clock: &C,
    runtime_instance_id: RuntimeInstanceId,
    conversation_id: ConversationId,
    tasks: &mut tokio::task::JoinSet<(WorkId, WorkRunnerExit)>,
    registry: &mut HashMap<WorkId, RegistryEntry>,
    task_to_work: &mut HashMap<tokio::task::Id, WorkId>,
    registry_view: &TaskRegistryView,
    claiming: &AtomicBool,
    claim_gate: &tokio::sync::Mutex<()>,
) -> Result<(), SchedulerError>
where
    S: SchedulerStateStore,
    R: WorkRunner,
    C: Clock,
{
    for cancellation in store
        .list_current_runtime_cancel_requested(runtime_instance_id)
        .await?
    {
        if let Some(entry) = registry.get(&cancellation.work_id) {
            let _ = entry.cancellation.send(true);
            update_view(registry, registry_view);
        } else if cancellation.current_attempt == CurrentWorkAttempt::None {
            store
                .interrupt_abnormal_runner(InterruptOwnedWorkRequest {
                    work_id: cancellation.work_id,
                    runtime_id: runtime_instance_id,
                    interrupted_at: now(clock)?,
                    event_id: JournalEventId::generate(),
                })
                .await?;
        }
    }

    if registry.is_empty() {
        let claim_section = claim_gate.lock().await;
        if claiming.load(Ordering::Acquire)
            && let Some(claimed) = store
                .claim_next_work(ClaimNextWorkRequest {
                    conversation_id,
                    runtime_id: runtime_instance_id,
                    claimed_at: now(clock)?,
                    event_id: JournalEventId::generate(),
                })
                .await?
        {
            #[cfg(feature = "test-failpoints")]
            crate::test_failpoints::reach(
                crate::test_failpoints::PhysicalHook::AfterWorkClaimCommit,
            );
            let work_id = claimed.work.work_id();
            let (cancellation, receiver) = tokio::sync::watch::channel(false);
            let future = match runner.start(claimed, WorkCancellation { receiver }) {
                Ok(future) => future,
                Err(_) => {
                    store
                        .interrupt_abnormal_runner(InterruptOwnedWorkRequest {
                            work_id,
                            runtime_id: runtime_instance_id,
                            interrupted_at: now(clock)?,
                            event_id: JournalEventId::generate(),
                        })
                        .await?;
                    drop(claim_section);
                    return Ok(());
                }
            };
            let abort = tasks.spawn(async move { (work_id, future.await) });
            let task_id = abort.id();
            registry.insert(
                work_id,
                RegistryEntry {
                    runtime_instance_id,
                    cancellation,
                    task_id,
                },
            );
            task_to_work.insert(task_id, work_id);
            update_view(registry, registry_view);
        }
        drop(claim_section);
    }
    Ok(())
}

type JoinedRunner = Result<(tokio::task::Id, (WorkId, WorkRunnerExit)), tokio::task::JoinError>;

enum JoinDisposition<'a> {
    ReconcileDurably,
    AlreadyDurablyClassified(&'a mut HashSet<tokio::task::Id>),
}

#[allow(clippy::too_many_arguments)]
async fn observe_join<S, C>(
    joined: Option<JoinedRunner>,
    store: &S,
    clock: &C,
    runtime_instance_id: RuntimeInstanceId,
    registry: &mut HashMap<WorkId, RegistryEntry>,
    task_to_work: &mut HashMap<tokio::task::Id, WorkId>,
    registry_view: &TaskRegistryView,
    disposition: JoinDisposition<'_>,
) -> Result<(), SchedulerError>
where
    S: SchedulerStateStore,
    C: Clock,
{
    let (work_id, task_id, exit) = match joined {
        Some(Ok((task_id, (work_id, exit)))) => {
            if task_to_work.remove(&task_id) != Some(work_id) {
                return Err(SchedulerError::Invariant);
            }
            (work_id, task_id, Some(exit))
        }
        Some(Err(error)) => {
            let task_id = error.id();
            let work_id = task_to_work
                .remove(&task_id)
                .ok_or(SchedulerError::Invariant)?;
            (work_id, task_id, None)
        }
        None => return Ok(()),
    };
    let entry = registry.remove(&work_id).ok_or(SchedulerError::Invariant)?;
    if entry.task_id != task_id || entry.runtime_instance_id != runtime_instance_id {
        return Err(SchedulerError::Invariant);
    }
    update_view(registry, registry_view);
    if let JoinDisposition::AlreadyDurablyClassified(classified_tasks) = disposition {
        if !classified_tasks.remove(&task_id) {
            return Err(SchedulerError::Invariant);
        }
        return Ok(());
    }
    match exit {
        Some(WorkRunnerExit::CancellationConfirmed) => {
            store
                .finish_cancellation(FinishCancellationRequest {
                    work_id,
                    runtime_id: runtime_instance_id,
                    confirmed_at: now(clock)?,
                    event_id: JournalEventId::generate(),
                })
                .await?;
        }
        Some(WorkRunnerExit::Abnormal) | None => {
            store
                .interrupt_abnormal_runner(InterruptOwnedWorkRequest {
                    work_id,
                    runtime_id: runtime_instance_id,
                    interrupted_at: now(clock)?,
                    event_id: JournalEventId::generate(),
                })
                .await?;
        }
    }
    Ok(())
}

fn update_view(registry: &HashMap<WorkId, RegistryEntry>, view: &TaskRegistryView) {
    let values = registry
        .iter()
        .map(|(work_id, entry)| {
            (
                *work_id,
                RegistryEntrySnapshot {
                    work_id: *work_id,
                    runtime_instance_id: entry.runtime_instance_id,
                    cancellation_requested: *entry.cancellation.borrow(),
                },
            )
        })
        .collect();
    match view.entries.lock() {
        Ok(mut snapshot) => *snapshot = values,
        Err(poisoned) => *poisoned.into_inner() = values,
    }
}

fn now(clock: &dyn Clock) -> Result<UtcTimestamp, SchedulerError> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| SchedulerError::Clock)?)
        .map_err(|_| SchedulerError::Clock)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    StateStore,
    Clock,
    Health,
    TaskJoin,
    Invariant,
}

impl From<StateStoreError> for SchedulerError {
    fn from(_: StateStoreError) -> Self {
        Self::StateStore
    }
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StateStore => "scheduler state store failure",
            Self::Clock => "scheduler clock failure",
            Self::Health => "scheduler health transition failure",
            Self::TaskJoin => "scheduler task join failure",
            Self::Invariant => "scheduler invariant failure",
        })
    }
}

impl std::error::Error for SchedulerError {}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::bootstrap::health::HealthState;
    use crate::domain::{
        ConversationWorkOrdinal, CorrelationId, CraxiiId, ProjectionVersion, WorkItem,
        WorkItemInputData, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput, WorkState,
        WorkspaceId,
    };
    use crate::ports::clock::TestClock;
    use crate::ports::state_store::{
        CommitReceipt, RequestOwnedCancellationRequest, StateStoreErrorKind, StateStoreFuture,
    };

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn test_clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            time::OffsetDateTime::from_unix_timestamp(1_777_000_000).unwrap(),
            Duration::ZERO,
        ))
    }

    fn claimed(runtime_instance_id: RuntimeInstanceId) -> ClaimedWork {
        let work_id = WorkId::generate();
        ClaimedWork {
            work: WorkItem::new(WorkItemInputData {
                work_id,
                craxii_id: CraxiiId::generate(),
                conversation_id: ConversationId::generate(),
                conversation_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                workspace_id: WorkspaceId::generate(),
                correlation_id: CorrelationId::generate(),
                created_at: "2026-08-28T03:00:00.000000Z".parse().unwrap(),
                queued_at: "2026-08-28T03:00:00.000000Z".parse().unwrap(),
            }),
            lifecycle: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id,
                state: WorkState::Running,
                projection_version: ProjectionVersion::try_new(2).unwrap(),
                runtime_owner: Some(runtime_instance_id),
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: None,
                terminal_reason: None,
            })
            .unwrap(),
            commit: CommitReceipt {
                committed_version: Some(ProjectionVersion::try_new(2).unwrap()),
                events: None,
            },
        }
    }

    #[derive(Default)]
    struct FakeSchedulerStore {
        claims: Mutex<VecDeque<ClaimedWork>>,
        cancellations: Mutex<Vec<crate::ports::state_store::CancelRequestedWork>>,
        finished: Mutex<Vec<WorkId>>,
        interrupted: Mutex<Vec<WorkId>>,
        fail_claim: AtomicBool,
        claim_count: AtomicUsize,
        claim_block: Option<Arc<ClaimBlock>>,
    }

    struct ClaimBlock {
        entered: tokio::sync::Barrier,
        release: tokio::sync::Semaphore,
    }

    impl FakeSchedulerStore {
        fn with_claim(claimed: ClaimedWork) -> Self {
            Self {
                claims: Mutex::new(VecDeque::from([claimed])),
                ..Self::default()
            }
        }

        fn with_blocked_claims(claims: VecDeque<ClaimedWork>, block: Arc<ClaimBlock>) -> Self {
            Self {
                claims: Mutex::new(claims),
                claim_block: Some(block),
                ..Self::default()
            }
        }
    }

    impl SchedulerStateStore for FakeSchedulerStore {
        fn claim_next_work(
            &self,
            _: ClaimNextWorkRequest,
        ) -> StateStoreFuture<'_, Option<ClaimedWork>> {
            Box::pin(async move {
                let claim_number = self.claim_count.fetch_add(1, Ordering::SeqCst) + 1;
                if claim_number == 1
                    && let Some(block) = &self.claim_block
                {
                    block.entered.wait().await;
                    let permit = block.release.acquire().await.map_err(|_| {
                        StateStoreError::new(StateStoreErrorKind::InternalInvariant)
                    })?;
                    permit.forget();
                }
                if self.fail_claim.load(Ordering::SeqCst) {
                    return Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant));
                }
                Ok(lock(&self.claims).pop_front())
            })
        }

        fn list_current_runtime_cancel_requested(
            &self,
            _: RuntimeInstanceId,
        ) -> StateStoreFuture<'_, Vec<crate::ports::state_store::CancelRequestedWork>> {
            Box::pin(async move { Ok(lock(&self.cancellations).clone()) })
        }

        fn finish_cancellation(
            &self,
            request: FinishCancellationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                lock(&self.finished).push(request.work_id);
                Ok(CommitReceipt {
                    committed_version: None,
                    events: None,
                })
            })
        }

        fn interrupt_abnormal_runner(
            &self,
            request: InterruptOwnedWorkRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                lock(&self.interrupted).push(request.work_id);
                Ok(CommitReceipt {
                    committed_version: None,
                    events: None,
                })
            })
        }

        fn request_owned_work_cancellation(
            &self,
            _: RequestOwnedCancellationRequest,
        ) -> StateStoreFuture<'_, Vec<WorkId>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct CancellationRunner {
        started: Arc<tokio::sync::Notify>,
    }

    impl WorkRunner for CancellationRunner {
        fn start(
            &self,
            _: ClaimedWork,
            mut cancellation: WorkCancellation,
        ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
            let started = Arc::clone(&self.started);
            Ok(Box::pin(async move {
                started.notify_one();
                cancellation.requested().await;
                WorkRunnerExit::CancellationConfirmed
            }))
        }
    }

    struct StartFailureRunner;

    impl WorkRunner for StartFailureRunner {
        fn start(
            &self,
            _: ClaimedWork,
            _: WorkCancellation,
        ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
            Err(WorkRunnerStartError)
        }
    }

    struct PanicRunner;

    impl WorkRunner for PanicRunner {
        fn start(
            &self,
            _: ClaimedWork,
            _: WorkCancellation,
        ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
            Ok(Box::pin(async move {
                panic!("scripted runner panic");
            }))
        }
    }

    struct DropObservedStubbornRunner {
        started: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }

    struct RunnerDropObservation(Arc<AtomicBool>);

    impl Drop for RunnerDropObservation {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl WorkRunner for DropObservedStubbornRunner {
        fn start(
            &self,
            _: ClaimedWork,
            _: WorkCancellation,
        ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
            let started = Arc::clone(&self.started);
            let dropped = Arc::clone(&self.dropped);
            Ok(Box::pin(async move {
                let _observation = RunnerDropObservation(dropped);
                started.notify_one();
                std::future::pending().await
            }))
        }
    }

    #[tokio::test]
    async fn manual_fallback_scan_owns_task_and_reconciles_lost_cancellation_signal() {
        let runtime_id = RuntimeInstanceId::generate();
        let claimed = claimed(runtime_id);
        let work_id = claimed.work.work_id();
        let store = FakeSchedulerStore::with_claim(claimed);
        let runner = CancellationRunner {
            started: Arc::new(tokio::sync::Notify::new()),
        };
        let mut tasks = tokio::task::JoinSet::new();
        let mut registry = HashMap::new();
        let mut task_to_work = HashMap::new();
        let view = TaskRegistryView::default();
        let claiming = AtomicBool::new(true);
        let claim_gate = tokio::sync::Mutex::new(());
        scan_once(
            &store,
            &runner,
            test_clock().as_ref(),
            runtime_id,
            ConversationId::generate(),
            &mut tasks,
            &mut registry,
            &mut task_to_work,
            &view,
            &claiming,
            &claim_gate,
        )
        .await
        .unwrap();
        runner.started.notified().await;
        assert_eq!(view.snapshot()[0].work_id, work_id);
        assert_eq!(registry.len(), 1);

        lock(&store.cancellations).push(crate::ports::state_store::CancelRequestedWork {
            work_id,
            current_attempt: CurrentWorkAttempt::None,
        });
        // Drive the periodic reconciliation path directly without sending a
        // notification; durable DB state is sufficient to wake the runner.
        scan_once(
            &store,
            &runner,
            test_clock().as_ref(),
            runtime_id,
            ConversationId::generate(),
            &mut tasks,
            &mut registry,
            &mut task_to_work,
            &view,
            &claiming,
            &claim_gate,
        )
        .await
        .unwrap();
        assert!(view.snapshot()[0].cancellation_requested);
        let joined = tasks.join_next_with_id().await;
        observe_join(
            joined,
            &store,
            test_clock().as_ref(),
            runtime_id,
            &mut registry,
            &mut task_to_work,
            &view,
            JoinDisposition::ReconcileDurably,
        )
        .await
        .unwrap();
        assert_eq!(*lock(&store.finished), vec![work_id]);
        assert!(view.snapshot().is_empty());
    }

    #[tokio::test]
    async fn post_claim_runner_start_failure_is_durably_interrupted_without_detach() {
        let runtime_id = RuntimeInstanceId::generate();
        let claimed = claimed(runtime_id);
        let work_id = claimed.work.work_id();
        let store = FakeSchedulerStore::with_claim(claimed);
        let mut tasks = tokio::task::JoinSet::new();
        let mut registry = HashMap::new();
        let mut task_to_work = HashMap::new();
        let view = TaskRegistryView::default();
        let claiming = AtomicBool::new(true);
        let claim_gate = tokio::sync::Mutex::new(());
        scan_once(
            &store,
            &StartFailureRunner,
            test_clock().as_ref(),
            runtime_id,
            ConversationId::generate(),
            &mut tasks,
            &mut registry,
            &mut task_to_work,
            &view,
            &claiming,
            &claim_gate,
        )
        .await
        .unwrap();
        assert_eq!(*lock(&store.interrupted), vec![work_id]);
        assert!(tasks.is_empty());
        assert!(view.snapshot().is_empty());
    }

    #[tokio::test]
    async fn runner_panic_is_joined_observed_and_interrupted() {
        let runtime_id = RuntimeInstanceId::generate();
        let claimed = claimed(runtime_id);
        let work_id = claimed.work.work_id();
        let store = FakeSchedulerStore::with_claim(claimed);
        let mut tasks = tokio::task::JoinSet::new();
        let mut registry = HashMap::new();
        let mut task_to_work = HashMap::new();
        let view = TaskRegistryView::default();
        let claiming = AtomicBool::new(true);
        let claim_gate = tokio::sync::Mutex::new(());
        scan_once(
            &store,
            &PanicRunner,
            test_clock().as_ref(),
            runtime_id,
            ConversationId::generate(),
            &mut tasks,
            &mut registry,
            &mut task_to_work,
            &view,
            &claiming,
            &claim_gate,
        )
        .await
        .unwrap();
        observe_join(
            tasks.join_next_with_id().await,
            &store,
            test_clock().as_ref(),
            runtime_id,
            &mut registry,
            &mut task_to_work,
            &view,
            JoinDisposition::ReconcileDurably,
        )
        .await
        .unwrap();
        assert_eq!(*lock(&store.interrupted), vec![work_id]);
        assert!(view.snapshot().is_empty());
    }

    #[tokio::test]
    async fn scheduler_handle_joins_every_runner_and_may_enable_only_test_readiness() {
        let runtime_id = RuntimeInstanceId::generate();
        let claimed = claimed(runtime_id);
        let work_id = claimed.work.work_id();
        let store = Arc::new(FakeSchedulerStore::with_claim(claimed));
        let started = Arc::new(tokio::sync::Notify::new());
        let runner = Arc::new(CancellationRunner {
            started: Arc::clone(&started),
        });
        let health = Health::new();
        let (fatal, _) = tokio::sync::watch::channel(false);
        let handle = start_scheduler(
            Arc::clone(&store),
            runner,
            test_clock(),
            health.clone(),
            fatal,
            SchedulerStart {
                runtime_instance_id: runtime_id,
                conversation_id: ConversationId::generate(),
                allow_test_ready: true,
            },
        )
        .unwrap();
        started.notified().await;
        assert_eq!(health.snapshot().state(), HealthState::Ready);
        assert_eq!(handle.registry().snapshot()[0].work_id, work_id);
        handle.stop_and_join().await.unwrap();
        assert_eq!(*lock(&store.finished), vec![work_id]);
    }

    #[tokio::test]
    async fn claim_quiescence_waits_through_registration_and_permanently_closes_admission() {
        let runtime_id = RuntimeInstanceId::generate();
        let first = claimed(runtime_id);
        let first_work_id = first.work.work_id();
        let second = claimed(runtime_id);
        let block = Arc::new(ClaimBlock {
            entered: tokio::sync::Barrier::new(2),
            release: tokio::sync::Semaphore::new(0),
        });
        let store = Arc::new(FakeSchedulerStore::with_blocked_claims(
            VecDeque::from([first, second]),
            Arc::clone(&block),
        ));
        let started = Arc::new(tokio::sync::Notify::new());
        let health = Health::new();
        let (fatal, _) = tokio::sync::watch::channel(false);
        let handle = start_scheduler(
            Arc::clone(&store),
            Arc::new(CancellationRunner {
                started: Arc::clone(&started),
            }),
            test_clock(),
            health,
            fatal,
            SchedulerStart {
                runtime_instance_id: runtime_id,
                conversation_id: ConversationId::generate(),
                allow_test_ready: false,
            },
        )
        .unwrap();

        block.entered.wait().await;
        let quiesced = handle.stop_claiming_and_wait();
        assert!(handle.claim_gate.try_lock().is_err());
        block.release.add_permits(1);
        quiesced.await;
        started.notified().await;
        assert_eq!(handle.registry().snapshot()[0].work_id, first_work_id);

        handle.stop_and_join().await.unwrap();
        assert_eq!(store.claim_count.load(Ordering::SeqCst), 1);
        assert_eq!(lock(&store.claims).len(), 1);
        assert_eq!(*lock(&store.finished), vec![first_work_id]);
    }

    #[tokio::test]
    async fn deadline_abort_is_owned_joined_and_observed_by_scheduler_parent() {
        let runtime_id = RuntimeInstanceId::generate();
        let work = claimed(runtime_id);
        let work_id = work.work.work_id();
        let store = Arc::new(FakeSchedulerStore::with_claim(work));
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let health = Health::new();
        let (fatal, _) = tokio::sync::watch::channel(false);
        let handle = start_scheduler(
            Arc::clone(&store),
            Arc::new(DropObservedStubbornRunner {
                started: Arc::clone(&started),
                dropped: Arc::clone(&dropped),
            }),
            test_clock(),
            health,
            fatal,
            SchedulerStart {
                runtime_instance_id: runtime_id,
                conversation_id: ConversationId::generate(),
                allow_test_ready: false,
            },
        )
        .unwrap();
        started.notified().await;
        let registry = handle.registry();
        assert_eq!(registry.snapshot()[0].work_id, work_id);

        handle.stop_claiming_and_wait().await;
        handle.begin_shutdown();
        handle.prepare_deadline_and_wait().await.unwrap();
        lock(&store.interrupted).push(work_id);
        handle.abort_runners_and_join().await.unwrap();

        assert!(dropped.load(Ordering::SeqCst));
        assert!(registry.snapshot().is_empty());
        assert_eq!(*lock(&store.interrupted), vec![work_id]);
    }

    #[tokio::test]
    async fn persistent_scheduler_consistency_failure_marks_fatal_and_stops() {
        let store = Arc::new(FakeSchedulerStore::default());
        store.fail_claim.store(true, Ordering::SeqCst);
        let health = Health::new();
        let (fatal, mut observed_fatal) = tokio::sync::watch::channel(false);
        let handle = start_scheduler(
            store,
            Arc::new(StartFailureRunner),
            test_clock(),
            health.clone(),
            fatal,
            SchedulerStart {
                runtime_instance_id: RuntimeInstanceId::generate(),
                conversation_id: ConversationId::generate(),
                allow_test_ready: false,
            },
        )
        .unwrap();
        observed_fatal.changed().await.unwrap();
        assert!(*observed_fatal.borrow());
        assert_eq!(health.snapshot().state(), HealthState::Fatal);
        assert_eq!(
            handle.stop_and_join().await,
            Err(SchedulerError::StateStore)
        );
    }
}
