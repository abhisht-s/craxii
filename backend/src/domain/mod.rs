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

mod content;
mod digest;
mod entities;
mod error;
mod evidence;
mod ids;
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
pub use error::{DomainValidationError, DomainValidationKind};
pub use evidence::{
    ArtifactCompression, ArtifactEncoding, ArtifactLogicalName, ArtifactMimeType, ArtifactProducer,
    ArtifactReference, ArtifactReferenceInput, ArtifactRetention, ArtifactStorageBackend,
    AuthorityDecision, AuthorityDecisionSnapshot, AuthorityPolicyVersion, AuthorityReasonCode,
    DiagnosticPid, GitRevision, LinuxBootId, ModelAttemptReference, ModelAttemptReferenceInput,
    ModelCapabilitySnapshot, ModelCapabilitySnapshotInput, ModelTargetId, OpaqueStorageKey,
    PackageVersion, PrivilegeMode, ProviderId, ProviderModelId, ProviderModelReference,
    ResolvedPathEvidence, RuntimeStartEvidence, RuntimeStartEvidenceInput,
    TargetConfigurationVersion, TokenCount, ToolAttemptReference, ToolAttemptReferenceInput,
    ToolName, ToolVersion,
};
pub use ids::{
    ArtifactId, ClientCommandId, ClientMessageId, ContextManifestId, ConversationId, CorrelationId,
    CraxiiId, DeviceId, DraftId, ExecutionId, JournalEventId, LogicalInvocationId, MessageId,
    ModelInvocationId, RuntimeInstanceId, ToolExecutionId, WorkId, WorkspaceId, WorkstationId,
};
pub use path::{LogicalPathKind, LogicalPathReference, MAX_LOGICAL_PATH_BYTES};
pub use sequence::{
    AgentStepNo, AttemptNo, ConversationWorkOrdinal, JournalOffset, StreamSeq, ToolOrdinal,
};
pub use time::{MonotonicDuration, UtcTimestamp};
