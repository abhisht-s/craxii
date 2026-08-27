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

mod digest;
mod error;
mod ids;
mod sequence;
mod time;

pub use digest::{CanonicalByteCount, Sha256Digest};
pub use error::{DomainValidationError, DomainValidationKind};
pub use ids::{
    ArtifactId, ClientCommandId, ClientMessageId, ContextManifestId, ConversationId, CorrelationId,
    CraxiiId, DeviceId, DraftId, ExecutionId, JournalEventId, LogicalInvocationId, MessageId,
    ModelInvocationId, RuntimeInstanceId, ToolExecutionId, WorkId, WorkspaceId, WorkstationId,
};
pub use sequence::{
    AgentStepNo, AttemptNo, ConversationWorkOrdinal, JournalOffset, StreamSeq, ToolOrdinal,
};
pub use time::{MonotonicDuration, UtcTimestamp};
