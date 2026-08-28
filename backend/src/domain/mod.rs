//! Canonical domain values independent of storage, transport, and providers.
//!
//! UUID identity types are intentionally not interchangeable:
//!
//! ```compile_fail
//! use craxii_server::domain::{ConversationId, MessageId};
//!
//! fn load_conversation(_: ConversationId) {}
//!
//! let message: MessageId = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap();
//! load_conversation(message);
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::{MessageId, WorkId};
//!
//! fn load_message(_: MessageId) {}
//!
//! let work: WorkId = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap();
//! load_message(work);
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::{ClientMessageId, MessageId};
//!
//! fn load_message(_: MessageId) {}
//!
//! let client: ClientMessageId =
//!     "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap();
//! load_message(client);
//! ```
//!
//! UUIDv7 time and lexical order are deliberately unavailable as semantic order:
//!
//! ```compile_fail
//! use craxii_server::domain::JournalEventId;
//!
//! let first: JournalEventId = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap();
//! let second: JournalEventId = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0e".parse().unwrap();
//! let _semantic_order = first < second;
//! ```
//!
//! Process-local monotonic durations have no serialization contract:
//!
//! ```compile_fail
//! use craxii_server::domain::MonotonicDuration;
//!
//! let duration = MonotonicDuration::from_millis(5);
//! let _persisted = serde_json::to_string(&duration).unwrap();
//! ```
//!
//! Stage 3.2 evidence types remain semantically distinct:
//!
//! ```compile_fail
//! use craxii_server::domain::{ConversationWorkOrdinal, RuntimeInstanceId, WorkstationGeneration};
//!
//! fn generation(_: WorkstationGeneration) {}
//! let runtime = RuntimeInstanceId::generate();
//! let ordinal = ConversationWorkOrdinal::try_new(1).unwrap();
//! generation(runtime);
//! generation(ordinal);
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::{LogicalPathReference, ResolvedPathEvidence};
//!
//! fn requested(_: LogicalPathReference) {}
//! fn misuse(evidence: ResolvedPathEvidence) { requested(evidence); }
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::{ArtifactId, Sha256Digest};
//!
//! fn artifact(_: ArtifactId) {}
//! artifact(Sha256Digest::hash_bytes(b"artifact"));
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::{ExecutionId, ToolExecutionId};
//!
//! fn dispatch(_: ExecutionId) {}
//! let tool_attempt = ToolExecutionId::generate();
//! dispatch(tool_attempt);
//! ```
//!
//! Stable normalized codes and safe messages cannot be created from arbitrary text:
//!
//! ```compile_fail
//! use craxii_server::domain::ErrorCode;
//!
//! let _: ErrorCode = String::from("adapter_supplied_code").into();
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::SafeMessage;
//!
//! let _: SafeMessage = String::from("raw provider failure").into();
//! ```
//!
//! Internal trace detail has no formatting, Serde, or raw-access surface:
//!
//! ```compile_fail
//! use craxii_server::domain::InternalDetail;
//!
//! fn require_display<T: std::fmt::Display>() {}
//! require_display::<InternalDetail>();
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::InternalDetail;
//!
//! fn require_debug<T: std::fmt::Debug>() {}
//! require_debug::<InternalDetail>();
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::InternalDetail;
//!
//! fn require_serialize<T: serde::Serialize>() {}
//! require_serialize::<InternalDetail>();
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::InternalDetail;
//!
//! fn require_deserialize<T: for<'de> serde::Deserialize<'de>>() {}
//! require_deserialize::<InternalDetail>();
//! ```
//!
//! ```compile_fail
//! use craxii_server::domain::InternalDetail;
//!
//! fn expose_raw(detail: &InternalDetail) -> &str { detail.as_str() }
//! ```
//!
//! Local validation has no context-free normalized conversion:
//!
//! ```compile_fail
//! use craxii_server::domain::{MessageId, NormalizedError};
//!
//! let local = "rejected".parse::<MessageId>().unwrap_err();
//! let _: NormalizedError = local.into();
//! ```
//!
//! Raw adapter errors cannot substitute for normalized domain errors:
//!
//! ```compile_fail
//! use craxii_server::domain::NormalizedError;
//!
//! fn handle(_: NormalizedError) {}
//! handle(std::fmt::Error);
//! ```
//!
//! Work, model, and tool lifecycle states cannot substitute for each other:
//!
//! ```compile_fail
//! use craxii_server::domain::{ModelInvocationState, WorkState};
//!
//! fn persist_work_state(_: WorkState) {}
//! persist_work_state(ModelInvocationState::Requesting);
//! ```

mod content;
mod digest;
mod entities;
mod error;
mod evidence;
mod ids;
mod journal;
mod lifecycle;
mod path;
mod sequence;
mod time;

pub use content::{ContentBlock, ContentVersion, MAX_CONTENT_TEXT_BYTES, MessageContent};
pub use digest::{CanonicalByteCount, Sha256Digest};
pub use entities::{
    Conversation, ConversationKind, ConversationLifecycle, CraxiiLifecycle, CraxiiPrincipal,
    CraxiiPrincipalInput, HostingProvider, Message, MessageInput, MessageRole, ProjectionVersion,
    SchemaVersion, WorkInputActor, WorkInputOrdinal, WorkInputRelationship, WorkItem,
    WorkItemInput, WorkItemInputData, WorkKind, WorkspaceCapabilityRef, WorkspaceIdentity,
    WorkspaceIdentityInput, WorkspaceLifecycle, WorkstationCapabilities,
    WorkstationCapabilitiesInput, WorkstationCapabilitiesVersion, WorkstationCapabilityFlags,
    WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits, WorkstationGeneration,
    WorkstationIdentity, WorkstationIdentityInput, WorkstationKind,
};
pub use error::{
    Certainty, DomainValidationError, DomainValidationKind, ErrorCategory, ErrorCode,
    InternalDetail, NormalizedError, Retryability, SafeMessage, SourceStatus,
};
pub use evidence::{
    ArtifactCompression, ArtifactEncoding, ArtifactLogicalName, ArtifactMimeType, ArtifactProducer,
    ArtifactReference, ArtifactReferenceInput, ArtifactRetention, ArtifactStorageBackend,
    ArtifactStorageKey, AuthorityDecision, AuthorityDecisionSnapshot, AuthorityPolicyVersion,
    AuthorityReasonCode, DiagnosticPid, GitRevision, LinuxBootId, ModelAttemptReference,
    ModelAttemptReferenceInput, ModelCapabilitySnapshot, ModelCapabilitySnapshotInput,
    ModelTargetId, PackageVersion, PrivilegeMode, ProviderId, ProviderModelId,
    ProviderModelReference, ResolvedPathEvidence, RuntimeStartEvidence, RuntimeStartEvidenceInput,
    TargetConfigurationVersion, TokenCount, ToolAttemptReference, ToolAttemptReferenceInput,
    ToolName, ToolVersion,
};
pub use ids::{
    ArtifactId, ClientCommandId, ClientMessageId, ContextManifestId, ConversationId, CorrelationId,
    CraxiiId, DeviceId, DraftId, ExecutionId, JournalEventId, LogicalInvocationId, MessageId,
    ModelInvocationId, RuntimeInstanceId, ToolExecutionId, WorkId, WorkspaceId, WorkstationId,
};
pub use journal::{
    ArtifactRecordedV1, ConversationCreatedV1, CraxiiInitializedV1, JournalActor,
    JournalContractError, JournalCurrentAttempt, JournalEvent, JournalEventKind,
    JournalEventPayload, JournalRuntimeState, JournalStageOwner, JournalStreamFamily,
    JournalStreamId, JournalVersionResolution, JournalWorkTerminalReason, MessageCommittedV1,
    ModelInvocationEventV1, RuntimeEventV1, ToolExecutionEventV1, WorkInputFactV1, WorkQueuedV1,
    WorkTransitionV1, resolve_event_version,
};
pub use lifecycle::{
    CancellationCheckpoint, CancellationChildOutcome, CancellationDecision, CleanupStatus,
    CurrentWorkAttempt, ExecutionControlEvent, ExecutionControlLatch,
    ExecutionControlLatchDecision, FailpointPhysicalInterpretation, FailpointRecoveryExpectation,
    FailpointSemanticExpectation, FinalAnswerEffect, FinalAnswerRequiredEffects,
    FirstProviderDeltaDecision, GracefulShutdownDecision, LifecycleConflictKind,
    LifecycleFailpoint, LifecycleInvariantKind, LifecycleLimit, LifecycleTransitionError,
    LimitOutcome, ModelCancellationDecision, ModelCancellationEvidence, ModelInvocationLifecycle,
    ModelInvocationState, ModelLifecycleEffect, ModelOutputFacts, ModelOutputFailure,
    ModelResponseStatus, ModelTerminalFacts, ModelTransitionDecision, ModelTransitionRequest,
    RecoveryAttempt, RecoveryClassification, RecoveryDecision, RecoveryInput,
    SyntheticStatusRequirement, TerminalDecision, ToolCancellationContext,
    ToolCancellationDecision, ToolExecutionLifecycle, ToolExecutionState, ToolLifecycleEffect,
    ToolLifecycleReference, ToolResultClass, ToolTransitionDecision, ToolTransitionRequest,
    WorkCancellationReason, WorkCompletionEvidence, WorkCompletionReason, WorkEventKind,
    WorkFailureReason, WorkInterruptionReason, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput,
    WorkProgressionAction, WorkRequiredEffect, WorkState, WorkTerminalReason,
    WorkTransitionDecision, WorkTransitionGuard, WorkTransitionRequest, classify_recovery,
    decide_cancellation, decide_final_answer, decide_first_provider_delta,
    decide_graceful_shutdown, decide_latched_tool_terminal, decide_limit_outcome,
    decide_model_cancellation, decide_model_terminal, decide_model_transition,
    decide_next_model_attempt, decide_tool_cancellation, decide_tool_transition,
    decide_work_transition, ensure_work_progression_allowed, failpoint_semantic_expectation,
    is_legal_model_pair, is_legal_tool_pair, is_legal_work_pair,
};
pub use path::{LogicalPathKind, LogicalPathReference, MAX_LOGICAL_PATH_BYTES};
pub use sequence::{
    AgentStepNo, AttemptNo, ConversationWorkOrdinal, JournalOffset, StreamSeq, ToolOrdinal,
};
pub use time::{MonotonicDuration, UtcTimestamp};
