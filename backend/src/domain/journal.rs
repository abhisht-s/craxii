//! Typed append-only journal contracts.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{
    ArtifactId, ArtifactRetention, ClientMessageId, ConversationId, ConversationKind,
    ConversationLifecycle, ConversationWorkOrdinal, CorrelationId, CraxiiId, DeviceId,
    DiagnosticPid, GitRevision, JournalEventId, JournalOffset, LinuxBootId, LogicalInvocationId,
    Message, MessageContent, MessageId, MessageInput, MessageRole, ModelInvocationId,
    ModelInvocationState, PackageVersion, ProjectionVersion, RuntimeInstanceId,
    RuntimeShutdownReason, SchemaVersion, Sha256Digest, StreamSeq, ToolExecutionId,
    ToolExecutionState, ToolResultClass, UtcTimestamp, WorkId, WorkInputActor, WorkInputOrdinal,
    WorkInputRelationship, WorkKind, WorkspaceId, WorkstationGeneration, WorkstationId,
};

/// The four aggregate families with durable journal streams in V0.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum JournalStreamFamily {
    Craxii,
    Conversation,
    Work,
    Runtime,
}

/// One canonical aggregate stream identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum JournalStreamId {
    Craxii(CraxiiId),
    Conversation(ConversationId),
    Work(WorkId),
    Runtime(RuntimeInstanceId),
}

impl JournalStreamId {
    #[must_use]
    pub const fn family(self) -> JournalStreamFamily {
        match self {
            Self::Craxii(_) => JournalStreamFamily::Craxii,
            Self::Conversation(_) => JournalStreamFamily::Conversation,
            Self::Work(_) => JournalStreamFamily::Work,
            Self::Runtime(_) => JournalStreamFamily::Runtime,
        }
    }
}

impl fmt::Display for JournalStreamId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Craxii(id) => write!(formatter, "craxii:{id}"),
            Self::Conversation(id) => write!(formatter, "conversation:{id}"),
            Self::Work(id) => write!(formatter, "work:{id}"),
            Self::Runtime(id) => write!(formatter, "runtime:{id}"),
        }
    }
}

impl FromStr for JournalStreamId {
    type Err = JournalContractError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim() != value || value.matches(':').count() != 1 {
            return Err(JournalContractError::InvalidStream);
        }
        let (kind, id) = value
            .split_once(':')
            .ok_or(JournalContractError::InvalidStream)?;
        match kind {
            "craxii" => CraxiiId::parse_canonical(id)
                .map(Self::Craxii)
                .map_err(|_| JournalContractError::InvalidStream),
            "conversation" => ConversationId::parse_canonical(id)
                .map(Self::Conversation)
                .map_err(|_| JournalContractError::InvalidStream),
            "work" => WorkId::parse_canonical(id)
                .map(Self::Work)
                .map_err(|_| JournalContractError::InvalidStream),
            "runtime" => RuntimeInstanceId::parse_canonical(id)
                .map(Self::Runtime)
                .map_err(|_| JournalContractError::InvalidStream),
            _ => Err(JournalContractError::InvalidStream),
        }
    }
}

/// The exact typed actor identity stored in the event envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalActor {
    User(Option<DeviceId>),
    Craxii(CraxiiId),
    Model(ModelInvocationId),
    Tool(ToolExecutionId),
    Runtime(RuntimeInstanceId),
    Client(DeviceId),
}

impl JournalActor {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Craxii(_) => "craxii",
            Self::Model(_) => "model",
            Self::Tool(_) => "tool",
            Self::Runtime(_) => "runtime",
            Self::Client(_) => "client",
        }
    }

    #[must_use]
    pub fn id(self) -> Option<String> {
        match self {
            Self::User(id) => id.map(|value| value.to_string()),
            Self::Craxii(id) => Some(id.to_string()),
            Self::Model(id) => Some(id.to_string()),
            Self::Tool(id) => Some(id.to_string()),
            Self::Runtime(id) => Some(id.to_string()),
            Self::Client(id) => Some(id.to_string()),
        }
    }

    pub fn parse(kind: &str, id: Option<&str>) -> Result<Self, JournalContractError> {
        match (kind, id) {
            ("user", None) => Ok(Self::User(None)),
            ("user", Some(id)) => DeviceId::parse_canonical(id)
                .map(|id| Self::User(Some(id)))
                .map_err(|_| JournalContractError::InvalidActor),
            ("craxii", Some(id)) => CraxiiId::parse_canonical(id)
                .map(Self::Craxii)
                .map_err(|_| JournalContractError::InvalidActor),
            ("model", Some(id)) => ModelInvocationId::parse_canonical(id)
                .map(Self::Model)
                .map_err(|_| JournalContractError::InvalidActor),
            ("tool", Some(id)) => ToolExecutionId::parse_canonical(id)
                .map(Self::Tool)
                .map_err(|_| JournalContractError::InvalidActor),
            ("runtime", Some(id)) => RuntimeInstanceId::parse_canonical(id)
                .map(Self::Runtime)
                .map_err(|_| JournalContractError::InvalidActor),
            ("client", Some(id)) => DeviceId::parse_canonical(id)
                .map(Self::Client)
                .map_err(|_| JournalContractError::InvalidActor),
            _ => Err(JournalContractError::InvalidActor),
        }
    }
}

/// The implementation stage responsible for first emitting a registered event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalStageOwner {
    Stage7,
    Stage8,
    Stage9,
    Stage10,
}

macro_rules! event_kinds {
    ($( $variant:ident => ($literal:literal, $stream:ident, $state:literal, $owner:ident, $public:literal) ),+ $(,)?) => {
        /// The complete V0 journal registry. Every initial payload version is one.
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum JournalEventKind { $( $variant ),+ }

        impl JournalEventKind {
            pub const ALL: [Self; 28] = [$(Self::$variant),+];
            #[must_use]
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $literal),+ } }
            #[must_use]
            pub const fn primary_stream(self) -> JournalStreamFamily { match self { $(Self::$variant => JournalStreamFamily::$stream),+ } }
            #[must_use]
            pub const fn state_bearing(self) -> bool { match self { $(Self::$variant => $state),+ } }
            #[must_use]
            pub const fn owner(self) -> JournalStageOwner { match self { $(Self::$variant => JournalStageOwner::$owner),+ } }
            #[must_use]
            pub const fn public_candidate(self) -> bool { match self { $(Self::$variant => $public),+ } }
            #[must_use]
            pub const fn current_version(self) -> i64 { 1 }
            pub fn parse(value: &str) -> Result<Self, JournalContractError> {
                match value { $($literal => Ok(Self::$variant),)+ _ => Err(JournalContractError::UnknownEventType) }
            }
            #[must_use]
            pub const fn emitted_in_stage_7(self) -> bool { matches!(self, Self::CraxiiInitialized | Self::ConversationCreated) }
        }

        impl fmt::Display for JournalEventKind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { formatter.write_str(self.as_str()) }
        }
    };
}

event_kinds! {
    CraxiiInitialized => ("craxii.initialized", Craxii, true, Stage7, false),
    ConversationCreated => ("conversation.created", Conversation, true, Stage7, true),
    MessageAccepted => ("message.accepted", Conversation, true, Stage9, true),
    WorkQueued => ("work.queued", Work, true, Stage9, true),
    WorkStarted => ("work.started", Work, true, Stage10, true),
    WorkWaitingOnModel => ("work.waiting_on_model", Work, true, Stage8, true),
    WorkWaitingOnTool => ("work.waiting_on_tool", Work, true, Stage8, true),
    WorkResumed => ("work.resumed", Work, true, Stage8, true),
    WorkCancelRequested => ("work.cancel_requested", Work, true, Stage9, true),
    WorkCancelled => ("work.cancelled", Work, true, Stage9, true),
    WorkCompleted => ("work.completed", Work, true, Stage8, true),
    WorkFailed => ("work.failed", Work, true, Stage8, true),
    WorkInterrupted => ("work.interrupted", Work, true, Stage10, true),
    ModelInvocationStarted => ("model.invocation_started", Work, true, Stage8, false),
    ModelInvocationStreaming => ("model.invocation_streaming", Work, true, Stage8, false),
    ModelInvocationCompleted => ("model.invocation_completed", Work, true, Stage8, false),
    ModelInvocationFailed => ("model.invocation_failed", Work, true, Stage8, false),
    ModelInvocationInterrupted => ("model.invocation_interrupted", Work, true, Stage8, false),
    ToolExecutionRequested => ("tool.execution_requested", Work, true, Stage8, false),
    ToolExecutionDispatching => ("tool.execution_dispatching", Work, true, Stage8, false),
    ToolExecutionCompleted => ("tool.execution_completed", Work, true, Stage8, false),
    ToolExecutionInterruptedBeforeDispatch => ("tool.execution_interrupted_before_dispatch", Work, true, Stage8, false),
    ToolExecutionOutcomeUnknown => ("tool.execution_outcome_unknown", Work, true, Stage8, false),
    AssistantMessageCommitted => ("assistant.message_committed", Conversation, true, Stage8, true),
    ArtifactRecorded => ("artifact.recorded", Work, false, Stage8, false),
    RuntimeStarted => ("runtime.started", Runtime, true, Stage10, false),
    RuntimeRecoveryPerformed => ("runtime.recovery_performed", Runtime, true, Stage10, false),
    RuntimeStopping => ("runtime.stopping", Runtime, false, Stage10, false),
}

/// Classification used before a payload is trusted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalVersionResolution {
    Supported(JournalEventKind),
    UnsupportedKnown(JournalEventKind),
    Unknown,
}

#[must_use]
pub fn resolve_event_version(event_type: &str, version: i64) -> JournalVersionResolution {
    match JournalEventKind::parse(event_type) {
        Ok(kind) if version == kind.current_version() => JournalVersionResolution::Supported(kind),
        Ok(kind) => JournalVersionResolution::UnsupportedKnown(kind),
        Err(_) => JournalVersionResolution::Unknown,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CraxiiInitializedV1 {
    pub craxii_id: CraxiiId,
    pub display_name: String,
    pub owner_label: String,
    pub architecture_revision: String,
    pub schema_revision: SchemaVersion,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub workstation_architecture: String,
    pub workstation_os_release: String,
    pub capabilities_sha256: Sha256Digest,
    pub workspace_id: WorkspaceId,
    pub workspace_logical_name: String,
    pub workspace_logical_root: String,
    pub primary_conversation_id: ConversationId,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationCreatedV1 {
    pub conversation_id: ConversationId,
    pub craxii_id: CraxiiId,
    pub kind: ConversationKind,
    pub lifecycle: ConversationLifecycle,
    pub next_work_ordinal: ConversationWorkOrdinal,
    pub state_version: ProjectionVersion,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageCommittedV1 {
    pub message_id: MessageId,
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub role: MessageRole,
    pub content: MessageContent,
    pub content_sha256: Sha256Digest,
    pub produced_by_work_id: Option<WorkId>,
    pub device_id: Option<DeviceId>,
    pub client_message_id: Option<ClientMessageId>,
    pub committed_at: UtcTimestamp,
}

impl MessageCommittedV1 {
    /// Reuses the canonical message constructor to validate provenance and content identity.
    pub fn validate_contract(&self) -> Result<(), JournalContractError> {
        let message = Message::try_new(MessageInput {
            message_id: self.message_id,
            craxii_id: self.craxii_id,
            conversation_id: self.conversation_id,
            role: self.role,
            content: self.content.clone(),
            produced_by_work_id: self.produced_by_work_id,
            device_id: self.device_id,
            client_message_id: self.client_message_id,
            committed_at: self.committed_at,
        })
        .map_err(|_| JournalContractError::InvalidPayload)?;
        if message.content_sha256() == self.content_sha256 {
            Ok(())
        } else {
            Err(JournalContractError::InvalidPayload)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalWorkTerminalReason {
    Answered,
    Refused,
    DefiniteNormalizedError,
    ProviderExhausted,
    InvalidModelOutput,
    LifecycleLimit,
    UserRequest,
    GracefulShutdown,
    RuntimeOwnershipLost,
    ProviderOutcomeUnknown,
    ToolInterruptedBeforeDispatch,
    ToolOutcomeUnknown,
    CleanupUnconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkInputFactV1 {
    pub input_event_id: JournalEventId,
    pub relationship: WorkInputRelationship,
    pub ordinal_within_work: WorkInputOrdinal,
    pub attached_at: UtcTimestamp,
    pub actor: WorkInputActor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkQueuedV1 {
    pub work_id: WorkId,
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub conversation_work_ordinal: ConversationWorkOrdinal,
    pub kind: WorkKind,
    pub priority: i64,
    pub workspace_id: WorkspaceId,
    pub correlation_id: CorrelationId,
    pub state_version: ProjectionVersion,
    pub created_at: UtcTimestamp,
    pub queued_at: UtcTimestamp,
    pub trigger: WorkInputFactV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalCurrentAttempt {
    None,
    Model(ModelInvocationId),
    Tool(ToolExecutionId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkTransitionV1 {
    pub work_id: WorkId,
    pub from_state: super::WorkState,
    pub to_state: super::WorkState,
    pub expected_state_version: ProjectionVersion,
    pub expected_runtime_owner: Option<RuntimeInstanceId>,
    pub expected_current_attempt: JournalCurrentAttempt,
    pub expected_cancellation_reason: Option<super::WorkCancellationReason>,
    pub state_version: ProjectionVersion,
    pub runtime_owner: Option<RuntimeInstanceId>,
    pub current_attempt: JournalCurrentAttempt,
    pub cancellation_reason: Option<super::WorkCancellationReason>,
    pub terminal_reason: Option<JournalWorkTerminalReason>,
    pub transitioned_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelInvocationEventV1 {
    pub work_id: WorkId,
    pub model_invocation_id: ModelInvocationId,
    pub logical_invocation_id: LogicalInvocationId,
    pub state: ModelInvocationState,
    pub observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecutionEventV1 {
    pub work_id: WorkId,
    pub tool_execution_id: ToolExecutionId,
    pub state: ToolExecutionState,
    pub outcome_classification: Option<ToolResultClass>,
    pub observed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRecordedV1 {
    pub work_id: WorkId,
    pub artifact_id: ArtifactId,
    pub sha256: Sha256Digest,
    pub canonical_length: u64,
    pub retention: ArtifactRetention,
    pub recorded_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStartedV1 {
    pub runtime_instance_id: RuntimeInstanceId,
    pub craxii_id: CraxiiId,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub linux_boot_id: LinuxBootId,
    pub process_id: DiagnosticPid,
    pub binary_version: PackageVersion,
    pub git_revision: GitRevision,
    pub schema_version: SchemaVersion,
    pub started_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRecoveryPerformedV1 {
    pub runtime_instance_id: RuntimeInstanceId,
    pub stale_runtimes_observed: u64,
    pub stale_runtimes_closed: u64,
    pub retained_queued_work: u64,
    pub interrupted_work: u64,
    pub model_attempts_provider_outcome_unknown: u64,
    pub model_attempts_terminal_preserved: u64,
    pub tool_attempts_interrupted_before_dispatch: u64,
    pub tool_attempts_outcome_unknown: u64,
    pub tool_attempts_terminal_preserved: u64,
    pub drafts_abandoned: u64,
    pub orphan_artifacts_observed: u64,
    pub cleanup_checks_performed: u64,
    pub cleanup_unconfirmed: u64,
    pub recovery_duration_ms: u64,
    pub binary_version: PackageVersion,
    pub schema_version: SchemaVersion,
    pub recovered_at: UtcTimestamp,
}

impl RuntimeRecoveryPerformedV1 {
    #[must_use]
    pub const fn counts_are_persistable(&self) -> bool {
        self.stale_runtimes_observed <= i64::MAX as u64
            && self.stale_runtimes_closed <= i64::MAX as u64
            && self.retained_queued_work <= i64::MAX as u64
            && self.interrupted_work <= i64::MAX as u64
            && self.model_attempts_provider_outcome_unknown <= i64::MAX as u64
            && self.model_attempts_terminal_preserved <= i64::MAX as u64
            && self.tool_attempts_interrupted_before_dispatch <= i64::MAX as u64
            && self.tool_attempts_outcome_unknown <= i64::MAX as u64
            && self.tool_attempts_terminal_preserved <= i64::MAX as u64
            && self.drafts_abandoned <= i64::MAX as u64
            && self.orphan_artifacts_observed <= i64::MAX as u64
            && self.cleanup_checks_performed <= i64::MAX as u64
            && self.cleanup_unconfirmed <= i64::MAX as u64
            && self.recovery_duration_ms <= i64::MAX as u64
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStoppingV1 {
    pub runtime_instance_id: RuntimeInstanceId,
    pub shutdown_requested_at: UtcTimestamp,
    pub shutdown_reason: RuntimeShutdownReason,
    pub grace_deadline: UtcTimestamp,
    pub active_work_count: u64,
    pub active_task_count: u64,
}

/// Trusted typed payloads. The envelope remains the sole kind/version discriminator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalEventPayload {
    CraxiiInitialized(CraxiiInitializedV1),
    ConversationCreated(ConversationCreatedV1),
    MessageAccepted(MessageCommittedV1),
    WorkQueued(WorkQueuedV1),
    WorkStarted(WorkTransitionV1),
    WorkWaitingOnModel(WorkTransitionV1),
    WorkWaitingOnTool(WorkTransitionV1),
    WorkResumed(WorkTransitionV1),
    WorkCancelRequested(WorkTransitionV1),
    WorkCancelled(WorkTransitionV1),
    WorkCompleted(WorkTransitionV1),
    WorkFailed(WorkTransitionV1),
    WorkInterrupted(WorkTransitionV1),
    ModelInvocationStarted(ModelInvocationEventV1),
    ModelInvocationStreaming(ModelInvocationEventV1),
    ModelInvocationCompleted(ModelInvocationEventV1),
    ModelInvocationFailed(ModelInvocationEventV1),
    ModelInvocationInterrupted(ModelInvocationEventV1),
    ToolExecutionRequested(ToolExecutionEventV1),
    ToolExecutionDispatching(ToolExecutionEventV1),
    ToolExecutionCompleted(ToolExecutionEventV1),
    ToolExecutionInterruptedBeforeDispatch(ToolExecutionEventV1),
    ToolExecutionOutcomeUnknown(ToolExecutionEventV1),
    AssistantMessageCommitted(MessageCommittedV1),
    ArtifactRecorded(ArtifactRecordedV1),
    RuntimeStarted(RuntimeStartedV1),
    RuntimeRecoveryPerformed(RuntimeRecoveryPerformedV1),
    RuntimeStopping(RuntimeStoppingV1),
}

impl JournalEventPayload {
    #[must_use]
    pub const fn kind(&self) -> JournalEventKind {
        match self {
            Self::CraxiiInitialized(_) => JournalEventKind::CraxiiInitialized,
            Self::ConversationCreated(_) => JournalEventKind::ConversationCreated,
            Self::MessageAccepted(_) => JournalEventKind::MessageAccepted,
            Self::WorkQueued(_) => JournalEventKind::WorkQueued,
            Self::WorkStarted(_) => JournalEventKind::WorkStarted,
            Self::WorkWaitingOnModel(_) => JournalEventKind::WorkWaitingOnModel,
            Self::WorkWaitingOnTool(_) => JournalEventKind::WorkWaitingOnTool,
            Self::WorkResumed(_) => JournalEventKind::WorkResumed,
            Self::WorkCancelRequested(_) => JournalEventKind::WorkCancelRequested,
            Self::WorkCancelled(_) => JournalEventKind::WorkCancelled,
            Self::WorkCompleted(_) => JournalEventKind::WorkCompleted,
            Self::WorkFailed(_) => JournalEventKind::WorkFailed,
            Self::WorkInterrupted(_) => JournalEventKind::WorkInterrupted,
            Self::ModelInvocationStarted(_) => JournalEventKind::ModelInvocationStarted,
            Self::ModelInvocationStreaming(_) => JournalEventKind::ModelInvocationStreaming,
            Self::ModelInvocationCompleted(_) => JournalEventKind::ModelInvocationCompleted,
            Self::ModelInvocationFailed(_) => JournalEventKind::ModelInvocationFailed,
            Self::ModelInvocationInterrupted(_) => JournalEventKind::ModelInvocationInterrupted,
            Self::ToolExecutionRequested(_) => JournalEventKind::ToolExecutionRequested,
            Self::ToolExecutionDispatching(_) => JournalEventKind::ToolExecutionDispatching,
            Self::ToolExecutionCompleted(_) => JournalEventKind::ToolExecutionCompleted,
            Self::ToolExecutionInterruptedBeforeDispatch(_) => {
                JournalEventKind::ToolExecutionInterruptedBeforeDispatch
            }
            Self::ToolExecutionOutcomeUnknown(_) => JournalEventKind::ToolExecutionOutcomeUnknown,
            Self::AssistantMessageCommitted(_) => JournalEventKind::AssistantMessageCommitted,
            Self::ArtifactRecorded(_) => JournalEventKind::ArtifactRecorded,
            Self::RuntimeStarted(_) => JournalEventKind::RuntimeStarted,
            Self::RuntimeRecoveryPerformed(_) => JournalEventKind::RuntimeRecoveryPerformed,
            Self::RuntimeStopping(_) => JournalEventKind::RuntimeStopping,
        }
    }
}

/// A committed dependency-neutral journal envelope. Raw JSON never crosses this boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalEvent {
    pub journal_offset: JournalOffset,
    pub event_id: JournalEventId,
    pub craxii_id: CraxiiId,
    pub stream_id: JournalStreamId,
    pub stream_seq: StreamSeq,
    pub event_version: i64,
    pub conversation_id: Option<ConversationId>,
    pub work_id: Option<WorkId>,
    pub causation_event_id: Option<JournalEventId>,
    pub correlation_id: CorrelationId,
    pub actor: JournalActor,
    pub runtime_instance_id: Option<RuntimeInstanceId>,
    pub payload: JournalEventPayload,
    pub payload_sha256: Sha256Digest,
    pub recorded_at: UtcTimestamp,
    pub occurred_at: Option<UtcTimestamp>,
}

impl JournalEvent {
    #[must_use]
    pub const fn kind(&self) -> JournalEventKind {
        self.payload.kind()
    }
}

/// Safe journal contract failures with no rejected payload material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalContractError {
    InvalidStream,
    InvalidActor,
    UnknownEventType,
    UnsupportedEventVersion,
    InvalidEnvelope,
    InvalidPayload,
    InvalidOrder,
    InvalidCausation,
    InconsistentProjection,
}

impl fmt::Display for JournalContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStream => "invalid journal stream",
            Self::InvalidActor => "invalid journal actor",
            Self::UnknownEventType => "unknown journal event type",
            Self::UnsupportedEventVersion => "unsupported journal event version",
            Self::InvalidEnvelope => "invalid journal envelope",
            Self::InvalidPayload => "invalid journal payload",
            Self::InvalidOrder => "invalid journal order",
            Self::InvalidCausation => "invalid journal causation",
            Self::InconsistentProjection => "inconsistent journal projection",
        })
    }
}

impl std::error::Error for JournalContractError {}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    #[test]
    fn stream_forms_are_exact_and_roundtrip() {
        for value in [
            format!("craxii:{V7}"),
            format!("conversation:{V7}"),
            format!("work:{V7}"),
            format!("runtime:{V7}"),
        ] {
            let stream: JournalStreamId = value.parse().unwrap();
            assert_eq!(stream.to_string(), value);
        }
        for invalid in [
            V7.to_owned(),
            format!("Craxii:{V7}"),
            format!("work:{V7}:extra"),
            format!("work: {}", V7),
            format!("model:{V7}"),
        ] {
            assert!(invalid.parse::<JournalStreamId>().is_err());
        }
    }

    #[test]
    fn registry_is_exact_complete_and_versioned() {
        assert_eq!(JournalEventKind::ALL.len(), 28);
        let unique = JournalEventKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 28);
        assert!(
            JournalEventKind::ALL
                .iter()
                .all(|kind| kind.current_version() == 1)
        );
        assert_eq!(
            JournalEventKind::ALL
                .iter()
                .filter(|kind| kind.emitted_in_stage_7())
                .map(|kind| kind.as_str())
                .collect::<Vec<_>>(),
            ["craxii.initialized", "conversation.created"]
        );
        assert_eq!(
            resolve_event_version("work.started", 2),
            JournalVersionResolution::UnsupportedKnown(JournalEventKind::WorkStarted)
        );
        assert_eq!(
            resolve_event_version("future.unknown", 1),
            JournalVersionResolution::Unknown
        );
    }
}
