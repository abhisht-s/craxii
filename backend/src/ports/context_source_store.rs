//! Narrow read-only causal context source boundary owned by Stage 16.

use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::domain::{
    AgentStepNo, ArtifactId, ArtifactStorageKey, CanonicalByteCount, ConversationId,
    ConversationWorkOrdinal, JournalEventId, JournalOffset, LogicalInvocationId, Message,
    ModelInvocationId, ModelInvocationState, ProviderModelReference, Sha256Digest, ToolExecutionId,
    ToolExecutionState, ToolName, ToolOrdinal, WorkId, WorkState, WorkspaceId, WorkstationId,
};
use crate::ports::state_store::{
    NormalizedModelOutput, PreparedContextManifest, PreparedContextSource,
};

pub type ContextSourceStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ContextSourceStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextSourceStoreErrorKind {
    Storage,
    MissingSource,
    CorruptSource,
    InvalidOwnership,
    DuplicateSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextSourceStoreError {
    kind: ContextSourceStoreErrorKind,
}

impl ContextSourceStoreError {
    #[must_use]
    pub const fn new(kind: ContextSourceStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> ContextSourceStoreErrorKind {
        self.kind
    }
}

impl Display for ContextSourceStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ContextSourceStoreErrorKind::Storage => "context source storage failure",
            ContextSourceStoreErrorKind::MissingSource => "context source is missing",
            ContextSourceStoreErrorKind::CorruptSource => "context source is corrupt",
            ContextSourceStoreErrorKind::InvalidOwnership => "context source ownership differs",
            ContextSourceStoreErrorKind::DuplicateSource => "context source is duplicated",
        })
    }
}

impl std::error::Error for ContextSourceStoreError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextEligibilityRequest {
    pub work_id: WorkId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextWorkSource {
    pub work_id: WorkId,
    pub conversation_id: ConversationId,
    pub ordinal: ConversationWorkOrdinal,
    pub workspace_id: WorkspaceId,
    pub state: WorkState,
    pub terminal_reason: Option<String>,
    pub terminal_journal_offset: Option<JournalOffset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextMessageSource {
    pub work_id: WorkId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub input_event_id: JournalEventId,
    pub journal_offset: JournalOffset,
    pub message: Message,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextAssistantMessageSource {
    pub work_id: WorkId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub journal_event_id: JournalEventId,
    pub journal_offset: JournalOffset,
    pub message: Message,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextModelOutputSource {
    pub model_invocation_id: ModelInvocationId,
    pub logical_invocation_id: LogicalInvocationId,
    pub work_id: WorkId,
    pub conversation_id: ConversationId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub agent_step_no: AgentStepNo,
    pub attempt_no: i64,
    pub provider_model: ProviderModelReference,
    pub normalized_output: NormalizedModelOutput,
    pub provider_opaque_artifacts: Vec<ContextArtifactDescriptor>,
    pub stop_reason: String,
    pub journal_offset: JournalOffset,
    pub has_committed_final_assistant: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextArtifactDescriptor {
    pub artifact_id: ArtifactId,
    pub storage_key: ArtifactStorageKey,
    pub sha256: Sha256Digest,
    pub captured_byte_count: CanonicalByteCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextStreamCounts {
    pub observed: CanonicalByteCount,
    pub captured: CanonicalByteCount,
    pub returned_inline: CanonicalByteCount,
    pub omitted: CanonicalByteCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextToolResultSource {
    pub tool_execution_id: ToolExecutionId,
    pub work_id: WorkId,
    pub conversation_id: ConversationId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub source_model_invocation_id: ModelInvocationId,
    pub agent_step_no: AgentStepNo,
    pub tool_ordinal: ToolOrdinal,
    pub provider_tool_call_id: String,
    pub tool_name: ToolName,
    pub state: ToolExecutionState,
    pub result: Option<Value>,
    pub stdout_counts: Option<ContextStreamCounts>,
    pub stderr_counts: Option<ContextStreamCounts>,
    pub stdout_artifact: Option<ContextArtifactDescriptor>,
    pub stderr_artifact: Option<ContextArtifactDescriptor>,
    pub truncated: bool,
    pub journal_offset: JournalOffset,
}

/// Terminal model/tool boundary evidence in durable causal order for continuation decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextContinuationBoundary {
    Model {
        model_invocation_id: ModelInvocationId,
        logical_invocation_id: LogicalInvocationId,
        work_id: WorkId,
        work_ordinal: ConversationWorkOrdinal,
        agent_step_no: AgentStepNo,
        attempt_no: i64,
        state: ModelInvocationState,
        journal_offset: JournalOffset,
    },
    Tool {
        tool_execution_id: ToolExecutionId,
        source_model_invocation_id: ModelInvocationId,
        work_id: WorkId,
        work_ordinal: ConversationWorkOrdinal,
        agent_step_no: AgentStepNo,
        tool_ordinal: ToolOrdinal,
        state: ToolExecutionState,
        journal_offset: JournalOffset,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextWorkstationSource {
    pub workstation_id: WorkstationId,
    pub semantic_json: Value,
    pub source_sha256: Sha256Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextWorkspaceSource {
    pub workspace_id: WorkspaceId,
    pub semantic_json: Value,
    pub source_sha256: Sha256Digest,
}

/// All eligibility facts observed through one SQLite read snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextEligibilitySnapshot {
    pub active_work: ContextWorkSource,
    pub active_trigger: ContextMessageSource,
    pub prior_works: Vec<ContextWorkSource>,
    pub prior_messages: Vec<ContextMessageSource>,
    pub prior_final_assistant_messages: Vec<ContextAssistantMessageSource>,
    pub completed_model_outputs: Vec<ContextModelOutputSource>,
    pub observed_tool_results: Vec<ContextToolResultSource>,
    pub continuation_boundaries: Vec<ContextContinuationBoundary>,
    pub workstation: ContextWorkstationSource,
    pub workspace: ContextWorkspaceSource,
    pub highest_prior_terminal_work_ordinal: Option<ConversationWorkOrdinal>,
    pub maximum_journal_offset: JournalOffset,
    pub exact_input_event_ids: Vec<JournalEventId>,
    pub active_output_record_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextReconstructionRequest {
    pub manifest: PreparedContextManifest,
    pub ordered_sources: Box<[PreparedContextSource]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextReloadedMessageSource {
    pub work_id: WorkId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub journal_event_id: JournalEventId,
    pub journal_offset: JournalOffset,
    pub message: Message,
}

/// Exact durable record loaded for one prepared manifest source position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextReloadedSource {
    InstructionVersion,
    ToolDefinition,
    Workstation(ContextWorkstationSource),
    Workspace(ContextWorkspaceSource),
    Message(ContextReloadedMessageSource),
    ModelOutput(ContextModelOutputSource),
    ToolResult(ContextToolResultSource),
    Work(ContextWorkSource),
    Artifact(ContextArtifactDescriptor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextReconstructionSnapshot {
    pub active_work: ContextWorkSource,
    pub ordered_sources: Vec<ContextReloadedSource>,
}

/// Read-only source loading. It exposes no mutation, SQLx, pool, row, or transaction type.
pub trait ContextSourceStore: Send + Sync {
    fn load_context_eligibility_snapshot(
        &self,
        request: ContextEligibilityRequest,
    ) -> ContextSourceStoreFuture<'_, ContextEligibilitySnapshot>;

    fn reload_context_sources(
        &self,
        request: ContextReconstructionRequest,
    ) -> ContextSourceStoreFuture<'_, ContextReconstructionSnapshot>;
}
