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
    ArtifactId, ArtifactReference, AuthorityDecisionSnapshot, CanonicalByteCount, ClientCommandId,
    ClientMessageId, Conversation, ConversationId, CorrelationId, CraxiiId, CraxiiPrincipal,
    CurrentWorkAttempt, DeviceId, JournalEventId, JournalOffset, LogicalInvocationId,
    LogicalPathReference, Message, MessageId, ModelAttemptReference, ModelInvocationId,
    ModelInvocationState, NormalizedError, PrivilegeMode, ProjectionVersion,
    ProviderModelReference, ResolvedPathEvidence, RuntimeInstanceId, Sha256Digest, ToolExecutionId,
    ToolExecutionState, ToolLifecycleReference, ToolName, ToolResultClass, ToolVersion,
    UtcTimestamp, WorkId, WorkItem, WorkLifecycleSnapshot, WorkState, WorkspaceId,
    WorkspaceIdentity, WorkstationCapabilities, WorkstationGeneration, WorkstationId,
    WorkstationIdentity,
};
use crate::ports::artifact_store::FinalizedArtifact;

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

/// A finalized physical object plus one immutable semantic metadata occurrence and event identity.
#[derive(Clone)]
pub struct PreparedArtifact {
    pub finalized: FinalizedArtifact,
    pub metadata: ArtifactReference,
    pub event: EventIntent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextSourceKind {
    SystemInstruction,
    DeveloperInstruction,
    WorkstationCapabilitySummary,
    WorkspaceIdentity,
    ToolDefinition,
    UserMessage,
    ActiveTrigger,
    AssistantMessage,
    CompletedModelOutput,
    ObservedToolResult,
    ArtifactContent,
    SyntheticFailure,
    SyntheticInterruption,
    SyntheticOutcomeUnknown,
    SyntheticDraftStatus,
    ProviderNativeContinuation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextSourceRecordKind {
    InstructionVersion,
    Workstation,
    Workspace,
    ToolDefinition,
    Message,
    ModelInvocation,
    ToolExecution,
    Work,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextSourceIdentity {
    Event(JournalEventId),
    Artifact(ArtifactId),
    Record {
        kind: ContextSourceRecordKind,
        id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextModelRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextTransformKind {
    Identity,
    InlineProjection,
    SyntheticStatus,
    ProviderContinuation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedContextSource {
    pub position: i64,
    pub kind: ContextSourceKind,
    pub identity: ContextSourceIdentity,
    pub model_role: Option<ContextModelRole>,
    pub item_class: Option<String>,
    pub source_content_sha256: Sha256Digest,
    pub rendered_byte_contribution: CanonicalByteCount,
    pub transform: ContextTransformKind,
    pub transformed: bool,
}

/// Exact immutable manifest facts produced later by Stage 16 and persisted by Stage 8.
pub struct PreparedContextManifest {
    pub context_manifest_id: crate::domain::ContextManifestId,
    pub work_id: WorkId,
    pub logical_invocation_id: LogicalInvocationId,
    pub provider_model: ProviderModelReference,
    pub assembler_version: String,
    pub context_policy_version: String,
    pub system_prompt_fingerprint: Sha256Digest,
    pub toolset_fingerprint: Sha256Digest,
    pub eligibility_conversation_id: ConversationId,
    pub active_work_ordinal: i64,
    pub highest_prior_terminal_work_ordinal: Option<i64>,
    pub input_event_ids: Vec<JournalEventId>,
    pub active_output_record_ids: Vec<String>,
    pub maximum_journal_offset: JournalOffset,
    pub canonical_byte_count: CanonicalByteCount,
    pub rendered_request_byte_count: CanonicalByteCount,
    pub estimated_input_tokens: u64,
    pub token_estimator_id: String,
    pub context_window_tokens: u64,
    pub reserved_output_tokens: u64,
    pub utilization_basis_points: u16,
    pub manifest_sha256: Sha256Digest,
    pub rendered_request_sha256: Sha256Digest,
    pub rendered_request_artifact_id: Option<ArtifactId>,
    pub omitted_source_count: u64,
    pub transformed_source_count: u64,
    pub sources: Vec<PreparedContextSource>,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSelectionReason {
    Explicit,
    ConfiguredDefault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequiredModelCapabilities {
    pub text_input: bool,
    pub text_output: bool,
    pub custom_tool_calling: bool,
    pub streaming: bool,
    pub ordered_output_items: bool,
    pub structured_output: bool,
    pub reasoning_continuation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderOptionValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOption {
    pub key: String,
    pub value: ProviderOptionValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NormalizedModelOutputItem {
    Text {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool_name: ToolName,
        arguments_json: String,
    },
    StructuredData {
        canonical_json: String,
    },
    Refusal {
        text: String,
    },
    ReasoningSummary {
        text: String,
    },
    ProviderOpaque {
        provider_id: crate::domain::ProviderId,
        item_type: String,
        sha256: Sha256Digest,
        artifact_id: ArtifactId,
    },
    UnknownProviderItem {
        item_type: String,
        sha256: Sha256Digest,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedModelOutput {
    pub items: Vec<NormalizedModelOutputItem>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

pub struct PreparedModelInvocation {
    pub attempt: ModelAttemptReference,
    pub selection_reason: ModelSelectionReason,
    pub required_capabilities: RequiredModelCapabilities,
    pub provider_options: Vec<ProviderOption>,
    pub request_sha256: Sha256Digest,
    pub request_artifact_id: Option<ArtifactId>,
    pub started_at: UtcTimestamp,
}

pub struct ModelStreamingObservation {
    pub first_byte_at: UtcTimestamp,
    pub first_output_at: Option<UtcTimestamp>,
    pub provider_request_id: Option<String>,
    pub provider_response_id: Option<String>,
    pub draft_exposed: bool,
}

pub struct ModelTerminalOutcome {
    pub state: ModelInvocationState,
    pub response_sha256: Option<Sha256Digest>,
    pub response_artifact_id: Option<ArtifactId>,
    pub normalized_output: Option<NormalizedModelOutput>,
    pub provider_request_id: Option<String>,
    pub provider_response_id: Option<String>,
    pub first_byte_at: Option<UtcTimestamp>,
    pub first_output_at: Option<UtcTimestamp>,
    pub completed_at: UtcTimestamp,
    pub usage: Option<ModelUsage>,
    pub stop_reason: Option<String>,
    pub tool_call_count: Option<u64>,
    pub draft_exposed: bool,
    pub normalized_error: Option<NormalizedError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolOutputPolicy {
    pub stdout_capture_limit: CanonicalByteCount,
    pub stderr_capture_limit: CanonicalByteCount,
    pub combined_inline_limit: CanonicalByteCount,
    pub per_stream_inline_limit: CanonicalByteCount,
}

pub struct PreparedToolExecution {
    pub lifecycle: ToolLifecycleReference,
    pub provider_tool_call_id: Option<String>,
    pub tool_name: ToolName,
    pub tool_version: ToolVersion,
    pub tool_schema_version: i64,
    pub arguments_json: String,
    pub arguments_sha256: Sha256Digest,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    /// Concrete effective logical CWD; omission is resolved to the workspace default by the caller.
    pub requested_cwd: LogicalPathReference,
    pub requested_privilege: PrivilegeMode,
    pub timeout_ms: u64,
    pub output_policy: ToolOutputPolicy,
    pub requested_at: UtcTimestamp,
}

impl PreparedToolExecution {
    /// Resolves an omitted user/tool CWD before constructing the persistence request.
    #[must_use]
    pub fn effective_requested_cwd(
        requested: Option<LogicalPathReference>,
        workspace_default: &LogicalPathReference,
    ) -> LogicalPathReference {
        requested.unwrap_or_else(|| workspace_default.clone())
    }
}

pub struct ToolDispatchIntent {
    pub authority: AuthorityDecisionSnapshot,
    pub effective_privilege: PrivilegeMode,
    pub resolved_cwd: ResolvedPathEvidence,
    pub timeout_ms: u64,
    pub output_policy: ToolOutputPolicy,
    pub dispatch_intent_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolStreamCounts {
    pub observed: CanonicalByteCount,
    pub captured: CanonicalByteCount,
    pub returned_inline: CanonicalByteCount,
    pub omitted: CanonicalByteCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResultEvidence {
    pub result_kind: ToolResultClass,
    pub summary: String,
    pub fields: Vec<(String, String)>,
}

pub struct ToolTerminalOutcome {
    pub state: ToolExecutionState,
    /// Optional deny evidence for a definite completed-before-dispatch result.
    pub predispatch_authority: Option<AuthorityDecisionSnapshot>,
    pub started_at: Option<UtcTimestamp>,
    pub completed_at: UtcTimestamp,
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub timed_out: Option<bool>,
    pub cancelled: Option<bool>,
    pub cleanup_confirmed: Option<bool>,
    pub result: Option<ToolResultEvidence>,
    pub stdout_artifact_id: Option<ArtifactId>,
    pub stderr_artifact_id: Option<ArtifactId>,
    pub stdout_counts: Option<ToolStreamCounts>,
    pub stderr_counts: Option<ToolStreamCounts>,
    pub truncated: bool,
    pub normalized_error: Option<NormalizedError>,
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
    pub manifest: PreparedContextManifest,
    pub invocation: PreparedModelInvocation,
    pub artifacts: Vec<PreparedArtifact>,
    pub work_next: WorkLifecycleSnapshot,
    pub invocation_event: EventIntent,
    pub work_event: EventIntent,
}

pub struct MarkModelStreamingRequest {
    pub expected_work: WorkExpectation,
    pub expected_model: ModelExpectation,
    pub observation: ModelStreamingObservation,
    pub event: EventIntent,
}

pub struct FinishModelInvocationRequest {
    pub expected_work: WorkExpectation,
    pub expected_model: ModelExpectation,
    pub outcome: ModelTerminalOutcome,
    pub artifacts: Vec<PreparedArtifact>,
    pub work_next: WorkLifecycleSnapshot,
    pub model_event: EventIntent,
    pub work_event: EventIntent,
}

pub struct RequestToolExecutionRequest {
    pub expected_work: WorkExpectation,
    pub tool: PreparedToolExecution,
    pub work_next: WorkLifecycleSnapshot,
    pub tool_event: EventIntent,
    pub work_event: EventIntent,
}

pub struct CommitToolDispatchIntentRequest {
    pub expected_work: WorkExpectation,
    pub expected_tool: ToolExpectation,
    pub dispatch: ToolDispatchIntent,
    pub event: EventIntent,
}

pub struct FinishToolExecutionRequest {
    pub expected_work: WorkExpectation,
    pub expected_tool: ToolExpectation,
    pub outcome: ToolTerminalOutcome,
    pub artifacts: Vec<PreparedArtifact>,
    pub work_next: WorkLifecycleSnapshot,
    pub tool_event: EventIntent,
    pub work_event: EventIntent,
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
    fn mark_model_streaming(
        &self,
        request: MarkModelStreamingRequest,
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

/// Stage 17 terminal assistant completion capability.
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
        MarkModelStreaming,
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
        const ALL: [Self; 17] = [
            Self::LoadOrBootstrapIdentity,
            Self::AcceptMessage,
            Self::ClaimWork,
            Self::TransitionWork,
            Self::RequestCancellation,
            Self::FinishCancellation,
            Self::BeginModel,
            Self::MarkModelStreaming,
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
        fn mark_model_streaming(
            &self,
            _: MarkModelStreamingRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::MarkModelStreaming)
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
        assert_eq!(Intent::ALL.len(), 17);
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
            "mark_model_streaming",
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

    #[test]
    fn omitted_tool_cwd_is_resolved_to_a_concrete_workspace_default_before_persistence() {
        let default = LogicalPathReference::absolute("/workspace").unwrap();
        assert_eq!(
            PreparedToolExecution::effective_requested_cwd(None, &default).canonical(),
            "/workspace"
        );
        let explicit = LogicalPathReference::workspace_relative("src").unwrap();
        assert_eq!(
            PreparedToolExecution::effective_requested_cwd(Some(explicit), &default).canonical(),
            "src"
        );
    }
}
