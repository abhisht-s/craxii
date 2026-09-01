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
    ArtifactId, ArtifactReference, AuthorityDecisionSnapshot, CancellationCommandReceipt,
    CanonicalByteCount, ClientCommandId, ClientMessageId, CommandHashEncodingVersion,
    CommandOutcome, CommandRequestHash, Conversation, ConversationId, CorrelationId, CraxiiId,
    CraxiiPrincipal, CurrentWorkAttempt, DeviceId, IdempotencyKey, JournalEventId, JournalOffset,
    LogicalInvocationId, LogicalPathReference, Message, MessageCommandReceipt, MessageContent,
    MessageId, ModelAttemptReference, ModelInvocationId, ModelInvocationState, NormalizedError,
    PrivilegeMode, ProjectionVersion, ProviderModelReference, RuntimeInstanceId,
    RuntimeRecoveryPerformedV1, RuntimeStartEvidence, RuntimeStoppingV1, Sha256Digest,
    ToolExecutionId, ToolExecutionState, ToolLifecycleReference, ToolName, ToolResultClass,
    ToolVersion, UtcTimestamp, WorkId, WorkItem, WorkLifecycleSnapshot, WorkState, WorkspaceId,
    WorkspaceIdentity, WorkstationCapabilities, WorkstationGeneration, WorkstationId,
    WorkstationIdentity,
};
use crate::ports::artifact_store::FinalizedArtifact;
use crate::ports::model_provider::{
    ModelUsageStatus, ProviderErrorKind, ProviderOutcomeCertainty, ProviderRetryEvidence,
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
    IdempotencyConflict,
    TargetNotFound,
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
            StateStoreErrorKind::IdempotencyConflict => "state store idempotency conflict",
            StateStoreErrorKind::TargetNotFound => "state store target not found",
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
    pub cancellation_reason: Option<crate::domain::WorkCancellationReason>,
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
            cancellation_reason: snapshot.cancellation_reason(),
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub retry_evidence: Option<ProviderRetryEvidence>,
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
    pub usage_status: ModelUsageStatus,
    pub provider_error_kind: Option<ProviderErrorKind>,
    pub provider_outcome_certainty: ProviderOutcomeCertainty,
    pub billing_ambiguity: bool,
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
    /// Exact canonical authority plus prepared-cwd evidence durably bound to this dispatch.
    pub dispatch_evidence_json: String,
    pub effective_privilege: PrivilegeMode,
    pub prepared_cwd: crate::ports::workstation_preparation::PreparedCwdEvidence,
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

/// Existing V3 `tool_executions.result_json` byte ceiling.
pub const MAX_TOOL_RESULT_JSON_BYTES: usize = 262_144;

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
    /// Generic canonical-evidence artifacts referenced from `result`, not stream columns.
    pub evidence_artifact_ids: Vec<ArtifactId>,
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
    pub execution_capabilities: ExecutionCapabilityObservation,
}

/// Host-probed Stage 13 execution facts carried into current capability refresh.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionCapabilityObservation {
    pub foreground_execute: bool,
    pub privilege_administrative: bool,
    pub process_group_cleanup: bool,
    pub cgroup_cleanup: bool,
}

impl ExecutionCapabilityObservation {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            foreground_execute: false,
            privilege_administrative: false,
            process_group_cleanup: false,
            cgroup_cleanup: false,
        }
    }
}

pub struct LoadOrBootstrapIdentityReceipt {
    pub identity: V0IdentityReference,
    pub created: bool,
    pub commit: CommitReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageCommandCandidates {
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub acceptance_event_id: JournalEventId,
    pub queued_event_id: JournalEventId,
}

/// Atomic user-message acceptance and conversational Work creation intent.
pub struct AcceptUserMessageRequest {
    pub client_message_id: ClientMessageId,
    pub device_id: DeviceId,
    pub idempotency_key: IdempotencyKey,
    pub request_hash: CommandRequestHash,
    pub hash_version: CommandHashEncodingVersion,
    pub conversation_id: ConversationId,
    pub content: MessageContent,
    pub accepted_at: UtcTimestamp,
    pub candidates: MessageCommandCandidates,
}

/// FIFO claim request; the adapter guards the selected row as queued at its current version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimNextWorkRequest {
    pub conversation_id: ConversationId,
    pub runtime_id: RuntimeInstanceId,
    pub claimed_at: UtcTimestamp,
    pub event_id: JournalEventId,
}

pub struct ClaimedWork {
    pub work: WorkItem,
    pub lifecycle: WorkLifecycleSnapshot,
    pub commit: CommitReceipt,
}

pub struct RequestCancellationRequest {
    pub client_command_id: ClientCommandId,
    pub device_id: DeviceId,
    pub idempotency_key: IdempotencyKey,
    pub request_hash: CommandRequestHash,
    pub hash_version: CommandHashEncodingVersion,
    pub work_id: WorkId,
    pub requested_at: UtcTimestamp,
    pub event_id: JournalEventId,
}

pub struct FinishCancellationRequest {
    pub work_id: WorkId,
    pub runtime_id: RuntimeInstanceId,
    pub confirmed_at: UtcTimestamp,
    pub event_id: JournalEventId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancelRequestedWork {
    pub work_id: WorkId,
    pub current_attempt: CurrentWorkAttempt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptOwnedWorkRequest {
    pub work_id: WorkId,
    pub runtime_id: RuntimeInstanceId,
    pub interrupted_at: UtcTimestamp,
    pub event_id: JournalEventId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestOwnedCancellationRequest {
    pub runtime_id: RuntimeInstanceId,
    pub requested_at: UtcTimestamp,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadOwnedWorkRequest {
    pub work_id: WorkId,
    pub runtime_id: RuntimeInstanceId,
}

/// Current durable runtime-owned Work facts used at every agent-loop checkpoint.
pub struct OwnedWorkState {
    pub work: WorkItem,
    pub lifecycle: WorkLifecycleSnapshot,
    pub started_at: UtcTimestamp,
    pub latest_work_event_id: JournalEventId,
    pub model_attempt_count: u64,
    pub tool_call_count: u64,
}

/// Generic exact terminal Work mutation for failures/limits/interruptions without an assistant.
pub struct TerminalizeOwnedWorkRequest {
    pub expected_work: WorkExpectation,
    pub work_next: WorkLifecycleSnapshot,
    pub terminal_at: UtcTimestamp,
    pub event: EventIntent,
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

/// One bounded underlying journal page. Public filtering belongs to the application layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicJournalPage {
    pub candidates: Vec<crate::domain::JournalEvent>,
    pub scanned_through: JournalOffset,
    pub has_more: bool,
}

/// Dependency-neutral source facts read atomically for the public bootstrap projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientBootstrapCandidate {
    pub snapshot_cursor: JournalOffset,
    pub principal: CraxiiPrincipal,
    pub primary_conversation: Conversation,
    pub messages: Vec<ClientMessageCandidate>,
    pub work_items: Vec<ClientWorkCandidate>,
    pub tool_summaries: Vec<ClientToolCandidate>,
    pub source_message_json_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientMessageCandidate {
    pub message: Message,
    pub conversation_sequence: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientWorkCandidate {
    pub work_id: WorkId,
    pub conversation_id: ConversationId,
    pub conversation_work_ordinal: crate::domain::ConversationWorkOrdinal,
    pub state: WorkState,
    pub trigger_message_id: MessageId,
    pub created_at: UtcTimestamp,
    pub queued_at: UtcTimestamp,
    pub started_at: Option<UtcTimestamp>,
    pub cancel_requested_at: Option<UtcTimestamp>,
    pub terminal_at: Option<UtcTimestamp>,
    pub terminal_reason: Option<crate::domain::JournalWorkTerminalReason>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientToolCandidate {
    pub work_id: WorkId,
    pub work_ordinal: crate::domain::ConversationWorkOrdinal,
    pub agent_step_no: crate::domain::AgentStepNo,
    pub tool_ordinal: crate::domain::ToolOrdinal,
    pub tool_execution_id: ToolExecutionId,
    pub tool_name: ToolName,
    pub state: ToolExecutionState,
    pub result_class: Option<ToolResultClass>,
    pub requested_at: UtcTimestamp,
    pub started_at: Option<UtcTimestamp>,
    pub completed_at: Option<UtcTimestamp>,
    pub cleanup_confirmed: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverStaleRuntimeRequest {
    pub stale_runtime_id: RuntimeInstanceId,
    pub current_runtime_id: RuntimeInstanceId,
    pub recovered_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassifyShutdownWorkRequest {
    pub runtime_id: RuntimeInstanceId,
    pub classified_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryReceipt {
    pub stale_runtime_closed: bool,
    pub interrupted_work: u64,
    pub model_attempts_provider_outcome_unknown: u64,
    pub model_attempts_terminal_preserved: u64,
    pub tool_attempts_interrupted_before_dispatch: u64,
    pub tool_attempts_outcome_unknown: u64,
    pub tool_attempts_terminal_preserved: u64,
    pub drafts_abandoned: u64,
    pub cleanup_checks_performed: u64,
    pub cleanup_unconfirmed: u64,
    pub commit: CommitReceipt,
}

pub struct CreateRuntimeRequest {
    pub evidence: RuntimeStartEvidence,
    pub event_id: JournalEventId,
    pub correlation_id: CorrelationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateRuntimeReceipt {
    pub runtime_instance_id: RuntimeInstanceId,
    pub started_event_id: JournalEventId,
    pub commit: CommitReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartbeatRuntimeRequest {
    pub runtime_instance_id: RuntimeInstanceId,
    pub observed_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartbeatRuntimeReceipt {
    pub persisted_at: UtcTimestamp,
    pub advanced: bool,
}

pub struct BeginRuntimeStoppingRequest {
    pub event: RuntimeStoppingV1,
    pub event_id: JournalEventId,
    pub correlation_id: CorrelationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginRuntimeStoppingReceipt {
    pub began: bool,
    pub commit: CommitReceipt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinishRuntimeRequest {
    pub runtime_instance_id: RuntimeInstanceId,
    pub stopped_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnumerateStaleRuntimesRequest {
    pub current_runtime_id: RuntimeInstanceId,
}

pub struct AppendRecoverySummaryRequest {
    pub summary: RuntimeRecoveryPerformedV1,
    pub event_id: JournalEventId,
    pub started_event_id: JournalEventId,
    pub correlation_id: CorrelationId,
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
    ) -> StateStoreFuture<'_, CommandOutcome<MessageCommandReceipt>>;
    fn request_cancellation(
        &self,
        request: RequestCancellationRequest,
    ) -> StateStoreFuture<'_, CommandOutcome<CancellationCommandReceipt>>;
}

/// Stage 10 scheduler/work-transition capability.
pub trait SchedulerStateStore: Send + Sync {
    fn claim_next_work(
        &self,
        request: ClaimNextWorkRequest,
    ) -> StateStoreFuture<'_, Option<ClaimedWork>>;
    fn list_current_runtime_cancel_requested(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> StateStoreFuture<'_, Vec<CancelRequestedWork>>;
    fn finish_cancellation(
        &self,
        request: FinishCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn interrupt_abnormal_runner(
        &self,
        request: InterruptOwnedWorkRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn request_owned_work_cancellation(
        &self,
        request: RequestOwnedCancellationRequest,
    ) -> StateStoreFuture<'_, Vec<WorkId>>;
}

/// Stage 10 RuntimeInstance lifecycle capability.
pub trait RuntimeStateStore: Send + Sync {
    fn create_runtime_and_started_event(
        &self,
        request: CreateRuntimeRequest,
    ) -> StateStoreFuture<'_, CreateRuntimeReceipt>;
    fn heartbeat_runtime(
        &self,
        request: HeartbeatRuntimeRequest,
    ) -> StateStoreFuture<'_, HeartbeatRuntimeReceipt>;
    fn begin_runtime_stopping(
        &self,
        request: BeginRuntimeStoppingRequest,
    ) -> StateStoreFuture<'_, BeginRuntimeStoppingReceipt>;
    fn finish_runtime_graceful(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn mark_runtime_startup_failure(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
    fn enumerate_stale_runtimes(
        &self,
        request: EnumerateStaleRuntimesRequest,
    ) -> StateStoreFuture<'_, Vec<RuntimeInstanceId>>;
    fn append_recovery_summary(
        &self,
        request: AppendRecoverySummaryRequest,
    ) -> StateStoreFuture<'_, CommitReceipt>;
}

/// Stage 8 model-attempt capability.
pub trait ModelStateStore: Send + Sync {
    fn load_owned_work(
        &self,
        request: LoadOwnedWorkRequest,
    ) -> StateStoreFuture<'_, OwnedWorkState>;
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
    fn terminalize_owned_work(
        &self,
        request: TerminalizeOwnedWorkRequest,
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
    fn current_journal_high_water(&self) -> StateStoreFuture<'_, JournalOffset>;
    fn load_client_bootstrap_snapshot(&self) -> StateStoreFuture<'_, ClientBootstrapCandidate>;
    fn list_public_journal_replay_candidates(
        &self,
        request: ListPublicJournalRequest,
    ) -> StateStoreFuture<'_, PublicJournalPage>;
}

/// Stage 10 stale-runtime recovery capability.
pub trait RecoveryStateStore: Send + Sync {
    fn count_retained_queued_work(&self) -> StateStoreFuture<'_, u64>;
    fn recover_stale_runtime_ownership(
        &self,
        request: RecoverStaleRuntimeRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt>;
    fn classify_unresolved_shutdown_work(
        &self,
        request: ClassifyShutdownWorkRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt>;
}

/// Future full assembly marker; staged adapters implement only capabilities they honestly own.
pub trait StateStore:
    BootstrapStateStore
    + CommandStateStore
    + SchedulerStateStore
    + RuntimeStateStore
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
        + RuntimeStateStore
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
        ListCancellation,
        InterruptWork,
        RequestOwnedCancellation,
        RequestCancellation,
        FinishCancellation,
        CreateRuntime,
        HeartbeatRuntime,
        BeginRuntimeStopping,
        FinishRuntime,
        MarkRuntimeStartupFailure,
        EnumerateStaleRuntimes,
        AppendRecoverySummary,
        BeginModel,
        MarkModelStreaming,
        FinishModel,
        RequestTool,
        CommitToolDispatch,
        FinishTool,
        CommitAssistant,
        LoadBootstrap,
        ListJournal,
        CountQueued,
        RecoverRuntime,
        ClassifyShutdownWork,
        VerifyConsistency,
    }

    impl Intent {
        const ALL: [Self; 28] = [
            Self::LoadOrBootstrapIdentity,
            Self::AcceptMessage,
            Self::ClaimWork,
            Self::ListCancellation,
            Self::InterruptWork,
            Self::RequestOwnedCancellation,
            Self::RequestCancellation,
            Self::FinishCancellation,
            Self::CreateRuntime,
            Self::HeartbeatRuntime,
            Self::BeginRuntimeStopping,
            Self::FinishRuntime,
            Self::MarkRuntimeStartupFailure,
            Self::EnumerateStaleRuntimes,
            Self::AppendRecoverySummary,
            Self::BeginModel,
            Self::MarkModelStreaming,
            Self::FinishModel,
            Self::RequestTool,
            Self::CommitToolDispatch,
            Self::FinishTool,
            Self::CommitAssistant,
            Self::LoadBootstrap,
            Self::ListJournal,
            Self::CountQueued,
            Self::RecoverRuntime,
            Self::ClassifyShutdownWork,
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
        ) -> StateStoreFuture<'_, CommandOutcome<MessageCommandReceipt>> {
            self.fail(Intent::AcceptMessage)
        }
        fn request_cancellation(
            &self,
            _: RequestCancellationRequest,
        ) -> StateStoreFuture<'_, CommandOutcome<CancellationCommandReceipt>> {
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
        fn list_current_runtime_cancel_requested(
            &self,
            _: RuntimeInstanceId,
        ) -> StateStoreFuture<'_, Vec<CancelRequestedWork>> {
            self.fail(Intent::ListCancellation)
        }
        fn finish_cancellation(
            &self,
            _: FinishCancellationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::FinishCancellation)
        }
        fn interrupt_abnormal_runner(
            &self,
            _: InterruptOwnedWorkRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::InterruptWork)
        }
        fn request_owned_work_cancellation(
            &self,
            _: RequestOwnedCancellationRequest,
        ) -> StateStoreFuture<'_, Vec<WorkId>> {
            self.fail(Intent::RequestOwnedCancellation)
        }
    }

    impl RuntimeStateStore for FakeStateStore {
        fn create_runtime_and_started_event(
            &self,
            _: CreateRuntimeRequest,
        ) -> StateStoreFuture<'_, CreateRuntimeReceipt> {
            self.fail(Intent::CreateRuntime)
        }
        fn heartbeat_runtime(
            &self,
            _: HeartbeatRuntimeRequest,
        ) -> StateStoreFuture<'_, HeartbeatRuntimeReceipt> {
            self.fail(Intent::HeartbeatRuntime)
        }
        fn begin_runtime_stopping(
            &self,
            _: BeginRuntimeStoppingRequest,
        ) -> StateStoreFuture<'_, BeginRuntimeStoppingReceipt> {
            self.fail(Intent::BeginRuntimeStopping)
        }
        fn finish_runtime_graceful(
            &self,
            _: FinishRuntimeRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::FinishRuntime)
        }
        fn mark_runtime_startup_failure(
            &self,
            _: FinishRuntimeRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::MarkRuntimeStartupFailure)
        }
        fn enumerate_stale_runtimes(
            &self,
            _: EnumerateStaleRuntimesRequest,
        ) -> StateStoreFuture<'_, Vec<RuntimeInstanceId>> {
            self.fail(Intent::EnumerateStaleRuntimes)
        }
        fn append_recovery_summary(
            &self,
            _: AppendRecoverySummaryRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            self.fail(Intent::AppendRecoverySummary)
        }
    }

    impl ModelStateStore for FakeStateStore {
        fn load_owned_work(
            &self,
            _: LoadOwnedWorkRequest,
        ) -> StateStoreFuture<'_, OwnedWorkState> {
            self.fail(Intent::BeginModel)
        }
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
        fn terminalize_owned_work(
            &self,
            _: TerminalizeOwnedWorkRequest,
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
        fn current_journal_high_water(&self) -> StateStoreFuture<'_, JournalOffset> {
            self.fail(Intent::ListJournal)
        }

        fn load_client_bootstrap_snapshot(&self) -> StateStoreFuture<'_, ClientBootstrapCandidate> {
            self.fail(Intent::LoadBootstrap)
        }

        fn list_public_journal_replay_candidates(
            &self,
            _: ListPublicJournalRequest,
        ) -> StateStoreFuture<'_, PublicJournalPage> {
            self.fail(Intent::ListJournal)
        }
    }

    impl RecoveryStateStore for FakeStateStore {
        fn count_retained_queued_work(&self) -> StateStoreFuture<'_, u64> {
            self.fail(Intent::CountQueued)
        }
        fn recover_stale_runtime_ownership(
            &self,
            _: RecoverStaleRuntimeRequest,
        ) -> StateStoreFuture<'_, RecoveryReceipt> {
            self.fail(Intent::RecoverRuntime)
        }
        fn classify_unresolved_shutdown_work(
            &self,
            _: ClassifyShutdownWorkRequest,
        ) -> StateStoreFuture<'_, RecoveryReceipt> {
            self.fail(Intent::ClassifyShutdownWork)
        }
    }

    fn require_state_store<T: StateStore>() {}

    #[test]
    fn fake_is_dependency_neutral_and_satisfies_the_complete_port() {
        require_state_store::<FakeStateStore>();
        let fake = FakeStateStore::new();
        assert!(fake.calls.lock().unwrap().is_empty());
        assert_eq!(Intent::ALL.len(), 28);
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
            "list_current_runtime_cancel_requested",
            "request_cancellation",
            "finish_cancellation",
            "interrupt_abnormal_runner",
            "request_owned_work_cancellation",
            "create_runtime_and_started_event",
            "heartbeat_runtime",
            "begin_runtime_stopping",
            "finish_runtime_graceful",
            "mark_runtime_startup_failure",
            "enumerate_stale_runtimes",
            "append_recovery_summary",
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
            "classify_unresolved_shutdown_work",
            "count_retained_queued_work",
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
