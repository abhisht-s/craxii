//! Dependency-neutral, intent-specific durable state boundary.
//!
//! Each mutating operation owns one future atomic SQLite transaction. The boundary deliberately
//! has no generic CRUD, SQL, query, row, connection, pool, transaction, database path, or callback
//! surface. Provider/network/workstation/process calls, filesystem content reads, artifact rename,
//! client delivery, and unrelated sleeps or waits do not belong inside these operations.

use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use crate::domain::{
    ClientCommandId, ClientMessageId, Conversation, ConversationId, CorrelationId, CraxiiId,
    CraxiiPrincipal, CurrentWorkAttempt, DeviceId, JournalEventId, JournalOffset, Message,
    MessageId, ModelAttemptReference, ModelInvocationId, ModelInvocationState, ProjectionVersion,
    RuntimeInstanceId, ToolExecutionId, ToolExecutionState, ToolLifecycleReference, UtcTimestamp,
    WorkId, WorkItem, WorkLifecycleSnapshot, WorkState, WorkspaceId, WorkspaceIdentity,
    WorkstationCapabilities, WorkstationGeneration, WorkstationId, WorkstationIdentity,
};

/// Boxed future used by the port without an async-trait or adapter dependency.
pub type StateStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, StateStoreError>> + Send + 'a>>;

/// Closed dependency-neutral failure classes returned by StateStore implementations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateStoreErrorKind {
    Storage,
    StateConflict,
    InternalInvariant,
}

/// A safe port error that retains no adapter failure or raw storage detail.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct StateStoreError {
    kind: StateStoreErrorKind,
}

impl StateStoreError {
    #[must_use]
    pub const fn new(kind: StateStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> StateStoreErrorKind {
        self.kind
    }
}

impl Display for StateStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self.kind {
            StateStoreErrorKind::Storage => "state store storage failure",
            StateStoreErrorKind::StateConflict => "state store state conflict",
            StateStoreErrorKind::InternalInvariant => "state store internal invariant failure",
        })
    }
}

impl Debug for StateStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl std::error::Error for StateStoreError {}

/// Inclusive committed journal offsets produced by one atomic intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommittedEventRange {
    pub first: JournalOffset,
    pub last: JournalOffset,
}

/// Dependency-neutral committed mutation facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitReceipt {
    pub committed_version: Option<ProjectionVersion>,
    pub events: Option<CommittedEventRange>,
}

/// Exact optimistic Work guard carried into every guarded persistence intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkExpectation {
    pub work_id: WorkId,
    pub state: WorkState,
    pub version: ProjectionVersion,
    pub runtime_owner: Option<RuntimeInstanceId>,
    pub current_attempt: CurrentWorkAttempt,
}

impl WorkExpectation {
    #[must_use]
    pub const fn for_snapshot(snapshot: &WorkLifecycleSnapshot) -> Self {
        Self {
            work_id: snapshot.work_id(),
            state: snapshot.state(),
            version: snapshot.projection_version(),
            runtime_owner: snapshot.runtime_owner(),
            current_attempt: snapshot.current_attempt(),
        }
    }
}

/// Exact expected state for one model attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelExpectation {
    pub model_invocation_id: ModelInvocationId,
    pub state: ModelInvocationState,
}

/// Exact expected state for one Tool attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolExpectation {
    pub tool_execution_id: ToolExecutionId,
    pub state: ToolExecutionState,
}

/// Narrow reference to the durable event an owning operation must append.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventIntent {
    pub event_id: JournalEventId,
    pub correlation_id: CorrelationId,
    pub causation_event_id: Option<JournalEventId>,
}

/// Stable identity references created or loaded by the Stage 7 bootstrap transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V0IdentityReference {
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub workstation_id: WorkstationId,
    pub workspace_id: WorkspaceId,
}

/// Inputs Stage 7 uses to atomically load or create V0 identity.
pub struct LoadOrBootstrapIdentityRequest {
    pub proposed: V0IdentityReference,
    pub initialized_event_id: JournalEventId,
    pub conversation_created_event_id: JournalEventId,
    pub correlation_id: CorrelationId,
    pub created_at: UtcTimestamp,
    pub observation: BootstrapObservation,
}

/// Truthful configuration/runtime facts captured before the bootstrap write transaction.
pub struct BootstrapObservation {
    pub initial_generation: WorkstationGeneration,
    pub architecture: String,
    pub os_release: String,
    pub default_shell: String,
    pub workspace_logical_name: String,
    pub workspace_logical_root: String,
    pub workspace_resolved_root: String,
    pub max_execution_timeout_ms: u64,
    pub max_stdout_bytes: u64,
    pub max_stderr_bytes: u64,
    pub administrative_enabled: bool,
}

pub struct LoadOrBootstrapIdentityReceipt {
    pub identity: V0IdentityReference,
    pub created: bool,
    pub commit: CommitReceipt,
}

/// Atomic user-message acceptance and conversational Work creation.
pub struct AcceptUserMessageRequest {
    pub command_id: ClientCommandId,
    pub client_message_id: ClientMessageId,
    pub device_id: DeviceId,
    pub expected_conversation_version: ProjectionVersion,
    pub message: Message,
    pub work: WorkItem,
    pub acceptance_event: EventIntent,
    pub queued_event: EventIntent,
}

pub struct AcceptUserMessageReceipt {
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub commit: CommitReceipt,
}

/// FIFO claim request; the adapter guards the selected row as queued at its current version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimNextWorkRequest {
    pub conversation_id: ConversationId,
    pub runtime_id: RuntimeInstanceId,
    pub expected_candidate_state: WorkState,
}

pub struct ClaimedWork {
    pub work: WorkItem,
    pub lifecycle: WorkLifecycleSnapshot,
    pub commit: CommitReceipt,
}

/// Generic Work transition is still one named semantic intent, never generic row update.
pub struct TransitionWorkRequest {
    pub expected: WorkExpectation,
    pub next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct RequestCancellationRequest {
    pub command_id: ClientCommandId,
    pub expected: WorkExpectation,
    pub next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct FinishCancellationRequest {
    pub expected: WorkExpectation,
    pub next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct BeginModelInvocationRequest {
    pub expected_work: WorkExpectation,
    pub attempt: ModelAttemptReference,
    pub work_next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct FinishModelInvocationRequest {
    pub expected_work: WorkExpectation,
    pub expected_model: ModelExpectation,
    pub model_next: ModelInvocationState,
    pub work_next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct RequestToolExecutionRequest {
    pub expected_work: WorkExpectation,
    pub tool: ToolLifecycleReference,
    pub work_next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct CommitToolDispatchIntentRequest {
    pub expected_work: WorkExpectation,
    pub expected_tool: ToolExpectation,
    pub event: EventIntent,
}

pub struct FinishToolExecutionRequest {
    pub expected_work: WorkExpectation,
    pub expected_tool: ToolExpectation,
    pub tool_next: ToolExecutionState,
    pub work_next: WorkLifecycleSnapshot,
    pub event: EventIntent,
}

pub struct CommitAssistantCompletionRequest {
    pub expected_work: WorkExpectation,
    pub expected_model: ModelExpectation,
    pub assistant_message: Message,
    pub assistant_event: EventIntent,
    pub completion_event: EventIntent,
    pub work_next: WorkLifecycleSnapshot,
}

/// Stable verified V0 bootstrap state available to the application shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapSnapshot {
    pub principal: CraxiiPrincipal,
    pub workstation: WorkstationIdentity,
    pub workstation_capabilities: WorkstationCapabilities,
    pub workspace: WorkspaceIdentity,
    pub primary_conversation: Conversation,
    pub identity: V0IdentityReference,
    pub journal_head: JournalOffset,
    pub consistency: ApplicationConsistencyReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListPublicJournalRequest {
    pub after: Option<JournalOffset>,
    pub through: JournalOffset,
    pub limit: u32,
}

/// Payload decoding remains a later journal-owned contract; this preserves replay identity/order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicJournalCandidate {
    pub event_id: JournalEventId,
    pub offset: JournalOffset,
    pub correlation_id: CorrelationId,
    pub causation_event_id: Option<JournalEventId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverStaleRuntimeRequest {
    pub stale_runtime_id: RuntimeInstanceId,
    pub current_runtime_id: RuntimeInstanceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryReceipt {
    pub retained_queued: u64,
    pub terminal_unchanged: u64,
    pub interrupted: u64,
    pub commit: CommitReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationConsistencyReceipt {
    pub checked_invariants: u64,
    pub journal_head: Option<JournalOffset>,
}

/// Stage 7 identity/bootstrap and read-only consistency capability.
pub trait BootstrapStateStore: Send + Sync {
    fn load_or_bootstrap_v0_identity(
        &self,
        request: LoadOrBootstrapIdentityRequest,
    ) -> StateStoreFuture<'_, LoadOrBootstrapIdentityReceipt>;
    fn load_bootstrap_snapshot(&self) -> StateStoreFuture<'_, BootstrapSnapshot>;
    fn verify_application_consistency(&self)
    -> StateStoreFuture<'_, ApplicationConsistencyReceipt>;
}

/// Stage 9 client command capability.
pub trait CommandStateStore: Send + Sync {
    fn accept_user_message_and_create_work(
        &self,
        request: AcceptUserMessageRequest,
    ) -> StateStoreFuture<'_, AcceptUserMessageReceipt>;
    fn request_cancellation(
        &self,
        request: RequestCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 10 scheduler/work-transition capability.
pub trait SchedulerStateStore: Send + Sync {
    fn claim_next_work(
        &self,
        request: ClaimNextWorkRequest,
    ) -> StateStoreFuture<'_, Option<ClaimedWork>>;
    fn transition_work_and_append_event(
        &self,
        request: TransitionWorkRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn finish_cancellation(
        &self,
        request: FinishCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 8 model-attempt capability.
pub trait ModelStateStore: Send + Sync {
    fn begin_model_invocation(
        &self,
        request: BeginModelInvocationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn finish_model_invocation(
        &self,
        request: FinishModelInvocationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 8 Tool-attempt capability.
pub trait ToolStateStore: Send + Sync {
    fn request_tool_execution(
        &self,
        request: RequestToolExecutionRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn commit_tool_dispatch_intent(
        &self,
        request: CommitToolDispatchIntentRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn finish_tool_execution(
        &self,
        request: FinishToolExecutionRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 8 terminal assistant completion capability.
pub trait CompletionStateStore: Send + Sync {
    fn commit_assistant_completion(
        &self,
        request: CommitAssistantCompletionRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 11 internal-to-public replay candidate capability.
pub trait ReplayStateStore: Send + Sync {
    fn list_public_journal_replay_candidates(
        &self,
        request: ListPublicJournalRequest,
    ) -> StateStoreFuture<'_, Vec<PublicJournalCandidate>>;
}

/// Stage 10 stale-runtime recovery capability.
pub trait RecoveryStateStore: Send + Sync {
    fn recover_stale_runtime_ownership(
        &self,
        request: RecoverStaleRuntimeRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt>;
}

/// Future full assembly marker; staged adapters implement only capabilities they honestly own.
pub trait StateStore:
    BootstrapStateStore
    + CommandStateStore
    + SchedulerStateStore
    + ModelStateStore
    + ToolStateStore
    + CompletionStateStore
    + ReplayStateStore
    + RecoveryStateStore
{
}

impl<T> StateStore for T where
    T: BootstrapStateStore
        + CommandStateStore
        + SchedulerStateStore
        + ModelStateStore
        + ToolStateStore
        + CompletionStateStore
        + ReplayStateStore
        + RecoveryStateStore
{
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Intent {
        LoadOrBootstrapIdentity,
        AcceptMessage,
        ClaimWork,
        TransitionWork,
        RequestCancellation,
        FinishCancellation,
        BeginModel,
        FinishModel,
        RequestTool,
        CommitToolDispatch,
        FinishTool,
        CommitAssistant,
        LoadBootstrap,
        ListJournal,
        RecoverRuntime,
        VerifyConsistency,
    }

    impl Intent {
        const ALL: [Self; 16] = [
            Self::LoadOrBootstrapIdentity,
            Self::AcceptMessage,
            Self::ClaimWork,
            Self::TransitionWork,
            Self::RequestCancellation,
            Self::FinishCancellation,
            Self::BeginModel,
            Self::FinishModel,
            Self::RequestTool,
            Self::CommitToolDispatch,
            Self::FinishTool,
            Self::CommitAssistant,
            Self::LoadBootstrap,
            Self::ListJournal,
            Self::RecoverRuntime,
            Self::VerifyConsistency,
        ];
    }

    /// Contract-only fake: it proves trait usability and is not an in-memory persistence model.
    struct FakeStateStore {
        calls: Mutex<Vec<Intent>>,
    }

    impl FakeStateStore {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        fn fail<T>(&self, intent: Intent) -> StateStoreFuture<'_, T> {
            self.calls.lock().unwrap().push(intent);
            Box::pin(async { Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant)) })
        }
    }

    impl BootstrapStateStore for FakeStateStore {
        fn load_or_bootstrap_v0_identity(
            &self,
            _: LoadOrBootstrapIdentityRequest,
        ) -> StateStoreFuture<'_, LoadOrBootstrapIdentityReceipt> {
            self.fail(Intent::LoadOrBootstrapIdentity)
        }
        fn load_bootstrap_snapshot(&self) -> StateStoreFuture<'_, BootstrapSnapshot> {
            self.fail(Intent::LoadBootstrap)
        }
        fn verify_application_consistency(
            &self,
        ) -> StateStoreFuture<'_, ApplicationConsistencyReceipt> {
            self.fail(Intent::VerifyConsistency)
        }
    }

    impl CommandStateStore for FakeStateStore {
        fn accept_user_message_and_create_work(
            &self,
            _: AcceptUserMessageRequest,
        ) -> StateStoreFuture<'_, AcceptUserMessageReceipt> {
            self.fail(Intent::AcceptMessage)
        }
        fn request_cancellation(
            &self,
            _: RequestCancellationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::RequestCancellation)
        }
    }

    impl SchedulerStateStore for FakeStateStore {
        fn claim_next_work(
            &self,
            _: ClaimNextWorkRequest,
        ) -> StateStoreFuture<'_, Option<ClaimedWork>> {
            self.fail(Intent::ClaimWork)
        }
        fn transition_work_and_append_event(
            &self,
            _: TransitionWorkRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::TransitionWork)
        }
        fn finish_cancellation(
            &self,
            _: FinishCancellationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::FinishCancellation)
        }
    }

    impl ModelStateStore for FakeStateStore {
        fn begin_model_invocation(
            &self,
            _: BeginModelInvocationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::BeginModel)
        }
        fn finish_model_invocation(
            &self,
            _: FinishModelInvocationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::FinishModel)
        }
    }

    impl ToolStateStore for FakeStateStore {
        fn request_tool_execution(
            &self,
            _: RequestToolExecutionRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::RequestTool)
        }
        fn commit_tool_dispatch_intent(
            &self,
            _: CommitToolDispatchIntentRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::CommitToolDispatch)
        }
        fn finish_tool_execution(
            &self,
            _: FinishToolExecutionRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::FinishTool)
        }
    }

    impl CompletionStateStore for FakeStateStore {
        fn commit_assistant_completion(
            &self,
            _: CommitAssistantCompletionRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::CommitAssistant)
        }
    }

    impl ReplayStateStore for FakeStateStore {
        fn list_public_journal_replay_candidates(
            &self,
            _: ListPublicJournalRequest,
        ) -> StateStoreFuture<'_, Vec<PublicJournalCandidate>> {
            self.fail(Intent::ListJournal)
        }
    }

    impl RecoveryStateStore for FakeStateStore {
        fn recover_stale_runtime_ownership(
            &self,
            _: RecoverStaleRuntimeRequest,
        ) -> StateStoreFuture<'_, RecoveryReceipt> {
            self.fail(Intent::RecoverRuntime)
        }
    }

    fn require_state_store<T: StateStore>() {}

    #[test]
    fn fake_is_dependency_neutral_and_satisfies_the_complete_port() {
        require_state_store::<FakeStateStore>();
        let fake = FakeStateStore::new();
        assert!(fake.calls.lock().unwrap().is_empty());
        assert_eq!(Intent::ALL.len(), 16);
    }

    #[test]
    fn expected_state_version_runtime_and_attempt_flow_together() {
        let work_id = WorkId::generate();
        let snapshot = WorkLifecycleSnapshot::initial(work_id);
        let expected = WorkExpectation::for_snapshot(&snapshot);
        assert_eq!(expected.work_id, work_id);
        assert_eq!(expected.state, WorkState::Queued);
        assert_eq!(expected.version.get(), 1);
        assert_eq!(expected.runtime_owner, None);
        assert_eq!(expected.current_attempt, CurrentWorkAttempt::None);
    }

    #[test]
    fn receipt_represents_committed_version_and_event_offset_range() {
        let receipt = CommitReceipt {
            committed_version: Some(ProjectionVersion::try_new(2).unwrap()),
            events: Some(CommittedEventRange {
                first: JournalOffset::try_new(10).unwrap(),
                last: JournalOffset::try_new(12).unwrap(),
            }),
        };
        assert_eq!(receipt.committed_version.unwrap().get(), 2);
        assert_eq!(receipt.events.unwrap().first.get(), 10);
        assert_eq!(receipt.events.unwrap().last.get(), 12);
    }

    #[test]
    fn one_method_name_exists_for_each_distinct_durable_intent() {
        let names = [
            "load_or_bootstrap_v0_identity",
            "accept_user_message_and_create_work",
            "claim_next_work",
            "transition_work_and_append_event",
            "request_cancellation",
            "finish_cancellation",
            "begin_model_invocation",
            "finish_model_invocation",
            "request_tool_execution",
            "commit_tool_dispatch_intent",
            "finish_tool_execution",
            "commit_assistant_completion",
            "load_bootstrap_snapshot",
            "list_public_journal_replay_candidates",
            "recover_stale_runtime_ownership",
            "verify_application_consistency",
        ];
        let unique = names.into_iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), Intent::ALL.len());
    }
}
