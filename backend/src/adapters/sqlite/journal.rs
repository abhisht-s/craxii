use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::Row;

use crate::domain::{
    ArtifactId, ArtifactRecordedV1, ArtifactRetention, ClientMessageId, ContentBlock,
    ConversationCreatedV1, ConversationId, ConversationKind, ConversationLifecycle,
    ConversationWorkOrdinal, CorrelationId, CraxiiId, CraxiiInitializedV1, DeviceId, DiagnosticPid,
    GitRevision, JournalActor, JournalContractError, JournalCurrentAttempt, JournalEvent,
    JournalEventId, JournalEventKind, JournalEventPayload, JournalOffset, JournalStreamId,
    LinuxBootId, LogicalInvocationId, MessageCommittedV1, MessageContent, MessageId, MessageRole,
    ModelInvocationEventV1, ModelInvocationId, ModelInvocationState, PackageVersion,
    ProjectionVersion, RuntimeInstanceId, RuntimeRecoveryPerformedV1, RuntimeShutdownReason,
    RuntimeStartedV1, RuntimeStoppingV1, SchemaVersion, Sha256Digest, StreamSeq,
    ToolExecutionEventV1, ToolExecutionId, ToolExecutionState, ToolResultClass, UtcTimestamp,
    WorkCancellationReason, WorkId, WorkInputActor, WorkInputFactV1, WorkInputOrdinal,
    WorkInputRelationship, WorkKind, WorkQueuedV1, WorkState, WorkTransitionV1, WorkspaceId,
    WorkstationGeneration, WorkstationId, resolve_event_version,
};

use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::transaction::WriteTransaction;

const MAX_PAYLOAD_BYTES: usize = 262_144;

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCraxiiInitializedV1 {
    craxii_id: CraxiiId,
    display_name: String,
    owner_label: String,
    architecture_revision: String,
    schema_revision: SchemaVersion,
    workstation_id: WorkstationId,
    workstation_generation: WorkstationGeneration,
    workstation_architecture: String,
    workstation_os_release: String,
    capabilities_sha256: Sha256Digest,
    workspace_id: WorkspaceId,
    workspace_logical_name: String,
    workspace_logical_root: String,
    primary_conversation_id: ConversationId,
    created_at: UtcTimestamp,
}

impl From<&CraxiiInitializedV1> for StoredCraxiiInitializedV1 {
    fn from(value: &CraxiiInitializedV1) -> Self {
        Self {
            craxii_id: value.craxii_id,
            display_name: value.display_name.clone(),
            owner_label: value.owner_label.clone(),
            architecture_revision: value.architecture_revision.clone(),
            schema_revision: value.schema_revision,
            workstation_id: value.workstation_id,
            workstation_generation: value.workstation_generation,
            workstation_architecture: value.workstation_architecture.clone(),
            workstation_os_release: value.workstation_os_release.clone(),
            capabilities_sha256: value.capabilities_sha256,
            workspace_id: value.workspace_id,
            workspace_logical_name: value.workspace_logical_name.clone(),
            workspace_logical_root: value.workspace_logical_root.clone(),
            primary_conversation_id: value.primary_conversation_id,
            created_at: value.created_at,
        }
    }
}

impl From<StoredCraxiiInitializedV1> for CraxiiInitializedV1 {
    fn from(value: StoredCraxiiInitializedV1) -> Self {
        Self {
            craxii_id: value.craxii_id,
            display_name: value.display_name,
            owner_label: value.owner_label,
            architecture_revision: value.architecture_revision,
            schema_revision: value.schema_revision,
            workstation_id: value.workstation_id,
            workstation_generation: value.workstation_generation,
            workstation_architecture: value.workstation_architecture,
            workstation_os_release: value.workstation_os_release,
            capabilities_sha256: value.capabilities_sha256,
            workspace_id: value.workspace_id,
            workspace_logical_name: value.workspace_logical_name,
            workspace_logical_root: value.workspace_logical_root,
            primary_conversation_id: value.primary_conversation_id,
            created_at: value.created_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConversationCreatedV1 {
    conversation_id: ConversationId,
    craxii_id: CraxiiId,
    kind: ConversationKind,
    lifecycle: ConversationLifecycle,
    next_work_ordinal: ConversationWorkOrdinal,
    state_version: ProjectionVersion,
    created_at: UtcTimestamp,
}

impl From<&ConversationCreatedV1> for StoredConversationCreatedV1 {
    fn from(value: &ConversationCreatedV1) -> Self {
        Self {
            conversation_id: value.conversation_id,
            craxii_id: value.craxii_id,
            kind: value.kind,
            lifecycle: value.lifecycle,
            next_work_ordinal: value.next_work_ordinal,
            state_version: value.state_version,
            created_at: value.created_at,
        }
    }
}

impl From<StoredConversationCreatedV1> for ConversationCreatedV1 {
    fn from(value: StoredConversationCreatedV1) -> Self {
        Self {
            conversation_id: value.conversation_id,
            craxii_id: value.craxii_id,
            kind: value.kind,
            lifecycle: value.lifecycle,
            next_work_ordinal: value.next_work_ordinal,
            state_version: value.state_version,
            created_at: value.created_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessageContentV1 {
    version: u8,
    blocks: Vec<StoredTextBlockV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum StoredTextBlockV1 {
    #[serde(rename = "text")]
    Text { text: String },
}

impl StoredMessageContentV1 {
    fn from_domain(value: &MessageContent) -> Self {
        Self {
            version: 1,
            blocks: value
                .blocks()
                .iter()
                .map(|block| StoredTextBlockV1::Text {
                    text: block.as_text().to_owned(),
                })
                .collect(),
        }
    }

    fn into_domain(self) -> Result<MessageContent, SqliteAdapterError> {
        if self.version != 1 {
            return Err(inconsistent());
        }
        let blocks = self
            .blocks
            .into_iter()
            .map(|block| match block {
                StoredTextBlockV1::Text { text } => {
                    ContentBlock::text(text).map_err(|_| inconsistent())
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        MessageContent::try_new(blocks).map_err(|_| inconsistent())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessageCommittedV1 {
    message_id: MessageId,
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    role: MessageRole,
    content: StoredMessageContentV1,
    content_sha256: Sha256Digest,
    produced_by_work_id: Option<WorkId>,
    device_id: Option<DeviceId>,
    client_message_id: Option<ClientMessageId>,
    committed_at: UtcTimestamp,
}

impl From<&MessageCommittedV1> for StoredMessageCommittedV1 {
    fn from(value: &MessageCommittedV1) -> Self {
        Self {
            message_id: value.message_id,
            craxii_id: value.craxii_id,
            conversation_id: value.conversation_id,
            role: value.role,
            content: StoredMessageContentV1::from_domain(&value.content),
            content_sha256: value.content_sha256,
            produced_by_work_id: value.produced_by_work_id,
            device_id: value.device_id,
            client_message_id: value.client_message_id,
            committed_at: value.committed_at,
        }
    }
}

impl TryFrom<StoredMessageCommittedV1> for MessageCommittedV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredMessageCommittedV1) -> Result<Self, Self::Error> {
        let content = value.content.into_domain()?;
        if content.content_sha256() != value.content_sha256 {
            return Err(inconsistent());
        }
        let provenance_valid = match value.role {
            MessageRole::User => {
                value.produced_by_work_id.is_none()
                    && value.device_id.is_some()
                    && value.client_message_id.is_some()
            }
            MessageRole::Assistant => {
                value.produced_by_work_id.is_some()
                    && value.device_id.is_none()
                    && value.client_message_id.is_none()
            }
            MessageRole::System => {
                value.produced_by_work_id.is_none()
                    && value.device_id.is_none()
                    && value.client_message_id.is_none()
            }
        };
        if !provenance_valid {
            return Err(inconsistent());
        }
        Ok(Self {
            message_id: value.message_id,
            craxii_id: value.craxii_id,
            conversation_id: value.conversation_id,
            role: value.role,
            content,
            content_sha256: value.content_sha256,
            produced_by_work_id: value.produced_by_work_id,
            device_id: value.device_id,
            client_message_id: value.client_message_id,
            committed_at: value.committed_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkInputFactV1 {
    input_event_id: JournalEventId,
    relationship: WorkInputRelationship,
    ordinal_within_work: WorkInputOrdinal,
    attached_at: UtcTimestamp,
    actor: WorkInputActor,
}

impl From<&WorkInputFactV1> for StoredWorkInputFactV1 {
    fn from(value: &WorkInputFactV1) -> Self {
        Self {
            input_event_id: value.input_event_id,
            relationship: value.relationship,
            ordinal_within_work: value.ordinal_within_work,
            attached_at: value.attached_at,
            actor: value.actor,
        }
    }
}

impl From<StoredWorkInputFactV1> for WorkInputFactV1 {
    fn from(value: StoredWorkInputFactV1) -> Self {
        Self {
            input_event_id: value.input_event_id,
            relationship: value.relationship,
            ordinal_within_work: value.ordinal_within_work,
            attached_at: value.attached_at,
            actor: value.actor,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkQueuedV1 {
    work_id: WorkId,
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    conversation_work_ordinal: ConversationWorkOrdinal,
    kind: WorkKind,
    priority: i64,
    workspace_id: WorkspaceId,
    correlation_id: CorrelationId,
    state_version: ProjectionVersion,
    created_at: UtcTimestamp,
    queued_at: UtcTimestamp,
    trigger: StoredWorkInputFactV1,
}

impl From<&WorkQueuedV1> for StoredWorkQueuedV1 {
    fn from(value: &WorkQueuedV1) -> Self {
        Self {
            work_id: value.work_id,
            craxii_id: value.craxii_id,
            conversation_id: value.conversation_id,
            conversation_work_ordinal: value.conversation_work_ordinal,
            kind: value.kind,
            priority: value.priority,
            workspace_id: value.workspace_id,
            correlation_id: value.correlation_id,
            state_version: value.state_version,
            created_at: value.created_at,
            queued_at: value.queued_at,
            trigger: StoredWorkInputFactV1::from(&value.trigger),
        }
    }
}

impl TryFrom<StoredWorkQueuedV1> for WorkQueuedV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredWorkQueuedV1) -> Result<Self, Self::Error> {
        if value.priority != 0
            || value.state_version.get() != 1
            || value.trigger.relationship != WorkInputRelationship::Trigger
            || value.trigger.ordinal_within_work.get() != 1
            || value.trigger.actor != WorkInputActor::User
        {
            return Err(inconsistent());
        }
        Ok(Self {
            work_id: value.work_id,
            craxii_id: value.craxii_id,
            conversation_id: value.conversation_id,
            conversation_work_ordinal: value.conversation_work_ordinal,
            kind: value.kind,
            priority: value.priority,
            workspace_id: value.workspace_id,
            correlation_id: value.correlation_id,
            state_version: value.state_version,
            created_at: value.created_at,
            queued_at: value.queued_at,
            trigger: value.trigger.into(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
enum StoredCurrentAttemptV1 {
    None,
    Model(ModelInvocationId),
    Tool(ToolExecutionId),
}

impl From<JournalCurrentAttempt> for StoredCurrentAttemptV1 {
    fn from(value: JournalCurrentAttempt) -> Self {
        match value {
            JournalCurrentAttempt::None => Self::None,
            JournalCurrentAttempt::Model(id) => Self::Model(id),
            JournalCurrentAttempt::Tool(id) => Self::Tool(id),
        }
    }
}

impl From<StoredCurrentAttemptV1> for JournalCurrentAttempt {
    fn from(value: StoredCurrentAttemptV1) -> Self {
        match value {
            StoredCurrentAttemptV1::None => Self::None,
            StoredCurrentAttemptV1::Model(id) => Self::Model(id),
            StoredCurrentAttemptV1::Tool(id) => Self::Tool(id),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkTransitionV1 {
    work_id: WorkId,
    from_state: WorkState,
    to_state: WorkState,
    expected_state_version: ProjectionVersion,
    expected_runtime_owner: Option<RuntimeInstanceId>,
    expected_current_attempt: StoredCurrentAttemptV1,
    expected_cancellation_reason: Option<WorkCancellationReason>,
    state_version: ProjectionVersion,
    runtime_owner: Option<RuntimeInstanceId>,
    current_attempt: StoredCurrentAttemptV1,
    cancellation_reason: Option<WorkCancellationReason>,
    terminal_reason: Option<crate::domain::JournalWorkTerminalReason>,
    transitioned_at: UtcTimestamp,
}

impl From<&WorkTransitionV1> for StoredWorkTransitionV1 {
    fn from(value: &WorkTransitionV1) -> Self {
        Self {
            work_id: value.work_id,
            from_state: value.from_state,
            to_state: value.to_state,
            expected_state_version: value.expected_state_version,
            expected_runtime_owner: value.expected_runtime_owner,
            expected_current_attempt: value.expected_current_attempt.into(),
            expected_cancellation_reason: value.expected_cancellation_reason,
            state_version: value.state_version,
            runtime_owner: value.runtime_owner,
            current_attempt: value.current_attempt.into(),
            cancellation_reason: value.cancellation_reason,
            terminal_reason: value.terminal_reason,
            transitioned_at: value.transitioned_at,
        }
    }
}

impl TryFrom<StoredWorkTransitionV1> for WorkTransitionV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredWorkTransitionV1) -> Result<Self, Self::Error> {
        if value.state_version.get()
            != value
                .expected_state_version
                .checked_increment()
                .map_err(|_| inconsistent())?
                .get()
            || !crate::domain::is_legal_work_pair(value.from_state, value.to_state)
        {
            return Err(inconsistent());
        }
        Ok(Self {
            work_id: value.work_id,
            from_state: value.from_state,
            to_state: value.to_state,
            expected_state_version: value.expected_state_version,
            expected_runtime_owner: value.expected_runtime_owner,
            expected_current_attempt: value.expected_current_attempt.into(),
            expected_cancellation_reason: value.expected_cancellation_reason,
            state_version: value.state_version,
            runtime_owner: value.runtime_owner,
            current_attempt: value.current_attempt.into(),
            cancellation_reason: value.cancellation_reason,
            terminal_reason: value.terminal_reason,
            transitioned_at: value.transitioned_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredModelInvocationEventV1 {
    work_id: WorkId,
    model_invocation_id: ModelInvocationId,
    logical_invocation_id: LogicalInvocationId,
    state: ModelInvocationState,
    observed_at: UtcTimestamp,
}

impl From<&ModelInvocationEventV1> for StoredModelInvocationEventV1 {
    fn from(value: &ModelInvocationEventV1) -> Self {
        Self {
            work_id: value.work_id,
            model_invocation_id: value.model_invocation_id,
            logical_invocation_id: value.logical_invocation_id,
            state: value.state,
            observed_at: value.observed_at,
        }
    }
}

impl From<StoredModelInvocationEventV1> for ModelInvocationEventV1 {
    fn from(value: StoredModelInvocationEventV1) -> Self {
        Self {
            work_id: value.work_id,
            model_invocation_id: value.model_invocation_id,
            logical_invocation_id: value.logical_invocation_id,
            state: value.state,
            observed_at: value.observed_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToolExecutionEventV1 {
    work_id: WorkId,
    tool_execution_id: ToolExecutionId,
    state: ToolExecutionState,
    outcome_classification: Option<ToolResultClass>,
    observed_at: UtcTimestamp,
}

impl From<&ToolExecutionEventV1> for StoredToolExecutionEventV1 {
    fn from(value: &ToolExecutionEventV1) -> Self {
        Self {
            work_id: value.work_id,
            tool_execution_id: value.tool_execution_id,
            state: value.state,
            outcome_classification: value.outcome_classification,
            observed_at: value.observed_at,
        }
    }
}

impl From<StoredToolExecutionEventV1> for ToolExecutionEventV1 {
    fn from(value: StoredToolExecutionEventV1) -> Self {
        Self {
            work_id: value.work_id,
            tool_execution_id: value.tool_execution_id,
            state: value.state,
            outcome_classification: value.outcome_classification,
            observed_at: value.observed_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArtifactRecordedV1 {
    work_id: WorkId,
    artifact_id: ArtifactId,
    sha256: Sha256Digest,
    canonical_length: u64,
    retention: ArtifactRetention,
    recorded_at: UtcTimestamp,
}

impl From<&ArtifactRecordedV1> for StoredArtifactRecordedV1 {
    fn from(value: &ArtifactRecordedV1) -> Self {
        Self {
            work_id: value.work_id,
            artifact_id: value.artifact_id,
            sha256: value.sha256,
            canonical_length: value.canonical_length,
            retention: value.retention,
            recorded_at: value.recorded_at,
        }
    }
}

impl From<StoredArtifactRecordedV1> for ArtifactRecordedV1 {
    fn from(value: StoredArtifactRecordedV1) -> Self {
        Self {
            work_id: value.work_id,
            artifact_id: value.artifact_id,
            sha256: value.sha256,
            canonical_length: value.canonical_length,
            retention: value.retention,
            recorded_at: value.recorded_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRuntimeStartedV1 {
    runtime_instance_id: RuntimeInstanceId,
    craxii_id: CraxiiId,
    workstation_id: WorkstationId,
    workstation_generation: WorkstationGeneration,
    linux_boot_id: String,
    process_id: DiagnosticPid,
    binary_version: String,
    git_revision: String,
    schema_version: SchemaVersion,
    started_at: UtcTimestamp,
}

impl From<&RuntimeStartedV1> for StoredRuntimeStartedV1 {
    fn from(value: &RuntimeStartedV1) -> Self {
        Self {
            runtime_instance_id: value.runtime_instance_id,
            craxii_id: value.craxii_id,
            workstation_id: value.workstation_id,
            workstation_generation: value.workstation_generation,
            linux_boot_id: value.linux_boot_id.as_str().to_owned(),
            process_id: value.process_id,
            binary_version: value.binary_version.as_str().to_owned(),
            git_revision: value.git_revision.as_str().to_owned(),
            schema_version: value.schema_version,
            started_at: value.started_at,
        }
    }
}

impl TryFrom<StoredRuntimeStartedV1> for RuntimeStartedV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredRuntimeStartedV1) -> Result<Self, Self::Error> {
        Ok(Self {
            runtime_instance_id: value.runtime_instance_id,
            craxii_id: value.craxii_id,
            workstation_id: value.workstation_id,
            workstation_generation: value.workstation_generation,
            linux_boot_id: LinuxBootId::try_new(value.linux_boot_id).map_err(|_| inconsistent())?,
            process_id: value.process_id,
            binary_version: PackageVersion::try_new(value.binary_version)
                .map_err(|_| inconsistent())?,
            git_revision: GitRevision::try_new(value.git_revision).map_err(|_| inconsistent())?,
            schema_version: value.schema_version,
            started_at: value.started_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRuntimeRecoveryPerformedV1 {
    runtime_instance_id: RuntimeInstanceId,
    stale_runtimes_observed: u64,
    stale_runtimes_closed: u64,
    retained_queued_work: u64,
    interrupted_work: u64,
    model_attempts_provider_outcome_unknown: u64,
    model_attempts_terminal_preserved: u64,
    tool_attempts_interrupted_before_dispatch: u64,
    tool_attempts_outcome_unknown: u64,
    tool_attempts_terminal_preserved: u64,
    drafts_abandoned: u64,
    orphan_artifacts_observed: u64,
    cleanup_checks_performed: u64,
    cleanup_unconfirmed: u64,
    recovery_duration_ms: u64,
    binary_version: String,
    schema_version: SchemaVersion,
    recovered_at: UtcTimestamp,
}

impl From<&RuntimeRecoveryPerformedV1> for StoredRuntimeRecoveryPerformedV1 {
    fn from(value: &RuntimeRecoveryPerformedV1) -> Self {
        Self {
            runtime_instance_id: value.runtime_instance_id,
            stale_runtimes_observed: value.stale_runtimes_observed,
            stale_runtimes_closed: value.stale_runtimes_closed,
            retained_queued_work: value.retained_queued_work,
            interrupted_work: value.interrupted_work,
            model_attempts_provider_outcome_unknown: value.model_attempts_provider_outcome_unknown,
            model_attempts_terminal_preserved: value.model_attempts_terminal_preserved,
            tool_attempts_interrupted_before_dispatch: value
                .tool_attempts_interrupted_before_dispatch,
            tool_attempts_outcome_unknown: value.tool_attempts_outcome_unknown,
            tool_attempts_terminal_preserved: value.tool_attempts_terminal_preserved,
            drafts_abandoned: value.drafts_abandoned,
            orphan_artifacts_observed: value.orphan_artifacts_observed,
            cleanup_checks_performed: value.cleanup_checks_performed,
            cleanup_unconfirmed: value.cleanup_unconfirmed,
            recovery_duration_ms: value.recovery_duration_ms,
            binary_version: value.binary_version.as_str().to_owned(),
            schema_version: value.schema_version,
            recovered_at: value.recovered_at,
        }
    }
}

impl TryFrom<StoredRuntimeRecoveryPerformedV1> for RuntimeRecoveryPerformedV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredRuntimeRecoveryPerformedV1) -> Result<Self, Self::Error> {
        let decoded = Self {
            runtime_instance_id: value.runtime_instance_id,
            stale_runtimes_observed: value.stale_runtimes_observed,
            stale_runtimes_closed: value.stale_runtimes_closed,
            retained_queued_work: value.retained_queued_work,
            interrupted_work: value.interrupted_work,
            model_attempts_provider_outcome_unknown: value.model_attempts_provider_outcome_unknown,
            model_attempts_terminal_preserved: value.model_attempts_terminal_preserved,
            tool_attempts_interrupted_before_dispatch: value
                .tool_attempts_interrupted_before_dispatch,
            tool_attempts_outcome_unknown: value.tool_attempts_outcome_unknown,
            tool_attempts_terminal_preserved: value.tool_attempts_terminal_preserved,
            drafts_abandoned: value.drafts_abandoned,
            orphan_artifacts_observed: value.orphan_artifacts_observed,
            cleanup_checks_performed: value.cleanup_checks_performed,
            cleanup_unconfirmed: value.cleanup_unconfirmed,
            recovery_duration_ms: value.recovery_duration_ms,
            binary_version: PackageVersion::try_new(value.binary_version)
                .map_err(|_| inconsistent())?,
            schema_version: value.schema_version,
            recovered_at: value.recovered_at,
        };
        if !decoded.counts_are_persistable() {
            return Err(inconsistent());
        }
        Ok(decoded)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRuntimeStoppingV1 {
    runtime_instance_id: RuntimeInstanceId,
    shutdown_requested_at: UtcTimestamp,
    shutdown_reason: String,
    grace_deadline: UtcTimestamp,
    active_work_count: u64,
    active_task_count: u64,
}

impl From<&RuntimeStoppingV1> for StoredRuntimeStoppingV1 {
    fn from(value: &RuntimeStoppingV1) -> Self {
        Self {
            runtime_instance_id: value.runtime_instance_id,
            shutdown_requested_at: value.shutdown_requested_at,
            shutdown_reason: value.shutdown_reason.as_str().to_owned(),
            grace_deadline: value.grace_deadline,
            active_work_count: value.active_work_count,
            active_task_count: value.active_task_count,
        }
    }
}

impl TryFrom<StoredRuntimeStoppingV1> for RuntimeStoppingV1 {
    type Error = SqliteAdapterError;

    fn try_from(value: StoredRuntimeStoppingV1) -> Result<Self, Self::Error> {
        if value.shutdown_reason != RuntimeShutdownReason::GracefulShutdown.as_str()
            || value.grace_deadline < value.shutdown_requested_at
            || value.active_work_count > i64::MAX as u64
            || value.active_task_count > i64::MAX as u64
        {
            return Err(inconsistent());
        }
        Ok(Self {
            runtime_instance_id: value.runtime_instance_id,
            shutdown_requested_at: value.shutdown_requested_at,
            shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
            grace_deadline: value.grace_deadline,
            active_work_count: value.active_work_count,
            active_task_count: value.active_task_count,
        })
    }
}

enum StoredEventPayloadV1 {
    CraxiiInitialized(StoredCraxiiInitializedV1),
    ConversationCreated(StoredConversationCreatedV1),
    Message(StoredMessageCommittedV1),
    WorkQueued(StoredWorkQueuedV1),
    WorkTransition(StoredWorkTransitionV1),
    Model(StoredModelInvocationEventV1),
    Tool(StoredToolExecutionEventV1),
    Artifact(StoredArtifactRecordedV1),
    RuntimeStarted(StoredRuntimeStartedV1),
    RuntimeRecovery(StoredRuntimeRecoveryPerformedV1),
    RuntimeStopping(StoredRuntimeStoppingV1),
}

fn to_json<T: Serialize>(value: &T) -> Result<String, SqliteAdapterError> {
    let json = serde_json::to_string(value).map_err(|_| inconsistent())?;
    if json.len() > MAX_PAYLOAD_BYTES {
        return Err(inconsistent());
    }
    Ok(json)
}

fn from_json<T: DeserializeOwned>(json: &str) -> Result<T, SqliteAdapterError> {
    if json.len() > MAX_PAYLOAD_BYTES {
        return Err(inconsistent());
    }
    serde_json::from_str(json).map_err(|_| inconsistent())
}

pub(super) fn encode_event_payload(
    payload: &JournalEventPayload,
) -> Result<(String, Sha256Digest), SqliteAdapterError> {
    let stored = match payload {
        JournalEventPayload::CraxiiInitialized(value) => {
            StoredEventPayloadV1::CraxiiInitialized(value.into())
        }
        JournalEventPayload::ConversationCreated(value) => {
            StoredEventPayloadV1::ConversationCreated(value.into())
        }
        JournalEventPayload::MessageAccepted(value)
        | JournalEventPayload::AssistantMessageCommitted(value) => {
            StoredEventPayloadV1::Message(value.into())
        }
        JournalEventPayload::WorkQueued(value) => StoredEventPayloadV1::WorkQueued(value.into()),
        JournalEventPayload::WorkStarted(value)
        | JournalEventPayload::WorkWaitingOnModel(value)
        | JournalEventPayload::WorkWaitingOnTool(value)
        | JournalEventPayload::WorkResumed(value)
        | JournalEventPayload::WorkCancelRequested(value)
        | JournalEventPayload::WorkCancelled(value)
        | JournalEventPayload::WorkCompleted(value)
        | JournalEventPayload::WorkFailed(value)
        | JournalEventPayload::WorkInterrupted(value) => {
            StoredEventPayloadV1::WorkTransition(value.into())
        }
        JournalEventPayload::ModelInvocationStarted(value)
        | JournalEventPayload::ModelInvocationStreaming(value)
        | JournalEventPayload::ModelInvocationCompleted(value)
        | JournalEventPayload::ModelInvocationFailed(value)
        | JournalEventPayload::ModelInvocationInterrupted(value) => {
            StoredEventPayloadV1::Model(value.into())
        }
        JournalEventPayload::ToolExecutionRequested(value)
        | JournalEventPayload::ToolExecutionDispatching(value)
        | JournalEventPayload::ToolExecutionCompleted(value)
        | JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value)
        | JournalEventPayload::ToolExecutionOutcomeUnknown(value) => {
            StoredEventPayloadV1::Tool(value.into())
        }
        JournalEventPayload::ArtifactRecorded(value) => {
            StoredEventPayloadV1::Artifact(value.into())
        }
        JournalEventPayload::RuntimeStarted(value) => {
            StoredEventPayloadV1::RuntimeStarted(value.into())
        }
        JournalEventPayload::RuntimeRecoveryPerformed(value) => {
            StoredEventPayloadV1::RuntimeRecovery(value.into())
        }
        JournalEventPayload::RuntimeStopping(value) => {
            StoredEventPayloadV1::RuntimeStopping(value.into())
        }
    };
    let json = match &stored {
        StoredEventPayloadV1::CraxiiInitialized(value) => to_json(value)?,
        StoredEventPayloadV1::ConversationCreated(value) => to_json(value)?,
        StoredEventPayloadV1::Message(value) => to_json(value)?,
        StoredEventPayloadV1::WorkQueued(value) => to_json(value)?,
        StoredEventPayloadV1::WorkTransition(value) => to_json(value)?,
        StoredEventPayloadV1::Model(value) => to_json(value)?,
        StoredEventPayloadV1::Tool(value) => to_json(value)?,
        StoredEventPayloadV1::Artifact(value) => to_json(value)?,
        StoredEventPayloadV1::RuntimeStarted(value) => to_json(value)?,
        StoredEventPayloadV1::RuntimeRecovery(value) => to_json(value)?,
        StoredEventPayloadV1::RuntimeStopping(value) => to_json(value)?,
    };
    let digest = Sha256Digest::hash_bytes(json.as_bytes());
    Ok((json, digest))
}

pub(super) fn decode_event_payload(
    event_type: &str,
    event_version: i64,
    payload_json: &str,
    payload_sha256: &str,
) -> Result<JournalEventPayload, SqliteAdapterError> {
    let stored_digest =
        Sha256Digest::parse_canonical(payload_sha256).map_err(|_| inconsistent())?;
    if Sha256Digest::hash_bytes(payload_json.as_bytes()) != stored_digest {
        return Err(inconsistent());
    }
    let kind = match resolve_event_version(event_type, event_version) {
        crate::domain::JournalVersionResolution::Supported(kind) => kind,
        crate::domain::JournalVersionResolution::UnsupportedKnown(_)
        | crate::domain::JournalVersionResolution::Unknown => return Err(inconsistent()),
    };
    let payload = match kind {
        JournalEventKind::CraxiiInitialized => JournalEventPayload::CraxiiInitialized(
            from_json::<StoredCraxiiInitializedV1>(payload_json)?.into(),
        ),
        JournalEventKind::ConversationCreated => JournalEventPayload::ConversationCreated(
            from_json::<StoredConversationCreatedV1>(payload_json)?.into(),
        ),
        JournalEventKind::MessageAccepted => JournalEventPayload::MessageAccepted(
            from_json::<StoredMessageCommittedV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkQueued => JournalEventPayload::WorkQueued(
            from_json::<StoredWorkQueuedV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkStarted => JournalEventPayload::WorkStarted(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkWaitingOnModel => JournalEventPayload::WorkWaitingOnModel(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkWaitingOnTool => JournalEventPayload::WorkWaitingOnTool(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkResumed => JournalEventPayload::WorkResumed(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkCancelRequested => JournalEventPayload::WorkCancelRequested(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkCancelled => JournalEventPayload::WorkCancelled(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkCompleted => JournalEventPayload::WorkCompleted(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkFailed => JournalEventPayload::WorkFailed(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::WorkInterrupted => JournalEventPayload::WorkInterrupted(
            from_json::<StoredWorkTransitionV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::ModelInvocationStarted => JournalEventPayload::ModelInvocationStarted(
            from_json::<StoredModelInvocationEventV1>(payload_json)?.into(),
        ),
        JournalEventKind::ModelInvocationStreaming => {
            JournalEventPayload::ModelInvocationStreaming(
                from_json::<StoredModelInvocationEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::ModelInvocationCompleted => {
            JournalEventPayload::ModelInvocationCompleted(
                from_json::<StoredModelInvocationEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::ModelInvocationFailed => JournalEventPayload::ModelInvocationFailed(
            from_json::<StoredModelInvocationEventV1>(payload_json)?.into(),
        ),
        JournalEventKind::ModelInvocationInterrupted => {
            JournalEventPayload::ModelInvocationInterrupted(
                from_json::<StoredModelInvocationEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::ToolExecutionRequested => JournalEventPayload::ToolExecutionRequested(
            from_json::<StoredToolExecutionEventV1>(payload_json)?.into(),
        ),
        JournalEventKind::ToolExecutionDispatching => {
            JournalEventPayload::ToolExecutionDispatching(
                from_json::<StoredToolExecutionEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::ToolExecutionCompleted => JournalEventPayload::ToolExecutionCompleted(
            from_json::<StoredToolExecutionEventV1>(payload_json)?.into(),
        ),
        JournalEventKind::ToolExecutionInterruptedBeforeDispatch => {
            JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(
                from_json::<StoredToolExecutionEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::ToolExecutionOutcomeUnknown => {
            JournalEventPayload::ToolExecutionOutcomeUnknown(
                from_json::<StoredToolExecutionEventV1>(payload_json)?.into(),
            )
        }
        JournalEventKind::AssistantMessageCommitted => {
            JournalEventPayload::AssistantMessageCommitted(
                from_json::<StoredMessageCommittedV1>(payload_json)?.try_into()?,
            )
        }
        JournalEventKind::ArtifactRecorded => JournalEventPayload::ArtifactRecorded(
            from_json::<StoredArtifactRecordedV1>(payload_json)?.into(),
        ),
        JournalEventKind::RuntimeStarted => JournalEventPayload::RuntimeStarted(
            from_json::<StoredRuntimeStartedV1>(payload_json)?.try_into()?,
        ),
        JournalEventKind::RuntimeRecoveryPerformed => {
            JournalEventPayload::RuntimeRecoveryPerformed(
                from_json::<StoredRuntimeRecoveryPerformedV1>(payload_json)?.try_into()?,
            )
        }
        JournalEventKind::RuntimeStopping => JournalEventPayload::RuntimeStopping(
            from_json::<StoredRuntimeStoppingV1>(payload_json)?.try_into()?,
        ),
    };
    validate_payload_kind(&payload)?;
    Ok(payload)
}

fn validate_payload_kind(payload: &JournalEventPayload) -> Result<(), SqliteAdapterError> {
    let valid = match payload {
        JournalEventPayload::CraxiiInitialized(value) => {
            value.display_name == "Craxii"
                && value.owner_label == "local-owner"
                && value.architecture_revision == "V0.0.01"
                && matches!(value.schema_revision.get(), 2 | 3)
                && crate::domain::LogicalPathReference::absolute(
                    value.workspace_logical_root.clone(),
                )
                .is_ok()
                && !value.workspace_logical_name.is_empty()
        }
        JournalEventPayload::ConversationCreated(value) => {
            value.kind == ConversationKind::Primary
                && value.lifecycle == ConversationLifecycle::Active
                && value.next_work_ordinal.get() == 1
                && value.state_version.get() == 1
        }
        JournalEventPayload::MessageAccepted(value) => {
            value.role == MessageRole::User && value.validate_contract().is_ok()
        }
        JournalEventPayload::AssistantMessageCommitted(value) => {
            value.role == MessageRole::Assistant && value.validate_contract().is_ok()
        }
        JournalEventPayload::WorkQueued(value) => {
            value.kind == WorkKind::Conversational
                && value.priority == 0
                && value.state_version.get() == 1
                && value.trigger.relationship == WorkInputRelationship::Trigger
                && value.trigger.ordinal_within_work.get() == 1
                && value.trigger.actor == WorkInputActor::User
        }
        JournalEventPayload::WorkStarted(value) => value.to_state == WorkState::Running,
        JournalEventPayload::WorkWaitingOnModel(value) => {
            value.to_state == WorkState::WaitingOnModel
        }
        JournalEventPayload::WorkWaitingOnTool(value) => value.to_state == WorkState::WaitingOnTool,
        JournalEventPayload::WorkResumed(value) => value.to_state == WorkState::Running,
        JournalEventPayload::WorkCancelRequested(value) => {
            value.to_state == WorkState::CancelRequested
        }
        JournalEventPayload::WorkCancelled(value) => value.to_state == WorkState::Cancelled,
        JournalEventPayload::WorkCompleted(value) => value.to_state == WorkState::Completed,
        JournalEventPayload::WorkFailed(value) => value.to_state == WorkState::Failed,
        JournalEventPayload::WorkInterrupted(value) => value.to_state == WorkState::Interrupted,
        JournalEventPayload::ModelInvocationStarted(value) => {
            value.state == ModelInvocationState::Requesting
        }
        JournalEventPayload::ModelInvocationStreaming(value) => {
            value.state == ModelInvocationState::Streaming
        }
        JournalEventPayload::ModelInvocationCompleted(value) => {
            value.state == ModelInvocationState::Completed
        }
        JournalEventPayload::ModelInvocationFailed(value) => {
            value.state == ModelInvocationState::Failed
        }
        JournalEventPayload::ModelInvocationInterrupted(value) => {
            matches!(
                value.state,
                ModelInvocationState::CancelledLocally
                    | ModelInvocationState::ProviderOutcomeUnknown
            )
        }
        JournalEventPayload::ToolExecutionRequested(value) => {
            value.state == ToolExecutionState::Requested
        }
        JournalEventPayload::ToolExecutionDispatching(value) => {
            value.state == ToolExecutionState::Dispatching
        }
        JournalEventPayload::ToolExecutionCompleted(value) => {
            value.state == ToolExecutionState::Completed
        }
        JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value) => {
            value.state == ToolExecutionState::InterruptedBeforeDispatch
                && value.outcome_classification.is_none()
        }
        JournalEventPayload::ToolExecutionOutcomeUnknown(value) => {
            value.state == ToolExecutionState::OutcomeUnknown
        }
        JournalEventPayload::ArtifactRecorded(value) => value.canonical_length <= i64::MAX as u64,
        JournalEventPayload::RuntimeStarted(value) => {
            value.schema_version.get() == 4
                && !value.linux_boot_id.as_str().is_empty()
                && !value.binary_version.as_str().is_empty()
                && !value.git_revision.as_str().is_empty()
        }
        JournalEventPayload::RuntimeRecoveryPerformed(value) => {
            value.schema_version.get() == 4 && value.counts_are_persistable()
        }
        JournalEventPayload::RuntimeStopping(value) => {
            value.shutdown_reason == RuntimeShutdownReason::GracefulShutdown
                && value.grace_deadline >= value.shutdown_requested_at
                && value.active_work_count <= i64::MAX as u64
                && value.active_task_count <= i64::MAX as u64
        }
    };
    if valid { Ok(()) } else { Err(inconsistent()) }
}

pub(super) struct JournalAppendIntent {
    pub event_id: JournalEventId,
    pub craxii_id: CraxiiId,
    pub stream_id: JournalStreamId,
    pub conversation_id: Option<ConversationId>,
    pub work_id: Option<WorkId>,
    pub causation_event_id: Option<JournalEventId>,
    pub correlation_id: CorrelationId,
    pub actor: JournalActor,
    pub runtime_instance_id: Option<RuntimeInstanceId>,
    pub payload: JournalEventPayload,
    pub recorded_at: UtcTimestamp,
    pub occurred_at: Option<UtcTimestamp>,
}

pub(super) struct PreparedJournalEvent {
    intent: JournalAppendIntent,
    payload_json: String,
    payload_sha256: Sha256Digest,
}

pub(super) fn prepare_event(
    intent: JournalAppendIntent,
) -> Result<PreparedJournalEvent, SqliteAdapterError> {
    validate_intent(&intent)?;
    validate_payload_kind(&intent.payload)?;
    let (payload_json, payload_sha256) = encode_event_payload(&intent.payload)?;
    Ok(PreparedJournalEvent {
        intent,
        payload_json,
        payload_sha256,
    })
}

fn validate_intent(intent: &JournalAppendIntent) -> Result<(), SqliteAdapterError> {
    let kind = intent.payload.kind();
    if kind.primary_stream() != intent.stream_id.family() {
        return Err(inconsistent());
    }
    let links_match = match (&intent.payload, intent.stream_id) {
        (JournalEventPayload::CraxiiInitialized(value), JournalStreamId::Craxii(id)) => {
            id == value.craxii_id
                && intent.craxii_id == value.craxii_id
                && intent.conversation_id == Some(value.primary_conversation_id)
                && intent.work_id.is_none()
                && intent.runtime_instance_id.is_none()
                && intent.causation_event_id.is_none()
                && intent.actor == JournalActor::Craxii(value.craxii_id)
        }
        (JournalEventPayload::ConversationCreated(value), JournalStreamId::Conversation(id)) => {
            id == value.conversation_id
                && intent.craxii_id == value.craxii_id
                && intent.conversation_id == Some(value.conversation_id)
                && intent.work_id.is_none()
                && intent.causation_event_id.is_some()
                && intent.actor == JournalActor::Craxii(value.craxii_id)
        }
        (JournalEventPayload::MessageAccepted(value), JournalStreamId::Conversation(id)) => {
            id == value.conversation_id
                && intent.craxii_id == value.craxii_id
                && intent.conversation_id == Some(value.conversation_id)
                && intent.work_id == value.produced_by_work_id
                && intent.actor == JournalActor::User(value.device_id)
        }
        (
            JournalEventPayload::AssistantMessageCommitted(value),
            JournalStreamId::Conversation(id),
        ) => {
            id == value.conversation_id
                && intent.craxii_id == value.craxii_id
                && intent.conversation_id == Some(value.conversation_id)
                && intent.work_id == value.produced_by_work_id
                && intent.actor == JournalActor::Craxii(value.craxii_id)
        }
        (JournalEventPayload::WorkQueued(value), JournalStreamId::Work(id)) => {
            id == value.work_id
                && intent.craxii_id == value.craxii_id
                && intent.conversation_id == Some(value.conversation_id)
                && intent.work_id == Some(value.work_id)
                && intent.correlation_id == value.correlation_id
                && intent.causation_event_id == Some(value.trigger.input_event_id)
                && intent.actor == JournalActor::Craxii(value.craxii_id)
        }
        (
            JournalEventPayload::WorkStarted(value)
            | JournalEventPayload::WorkWaitingOnModel(value)
            | JournalEventPayload::WorkWaitingOnTool(value)
            | JournalEventPayload::WorkResumed(value)
            | JournalEventPayload::WorkCancelRequested(value)
            | JournalEventPayload::WorkCancelled(value)
            | JournalEventPayload::WorkCompleted(value)
            | JournalEventPayload::WorkFailed(value)
            | JournalEventPayload::WorkInterrupted(value),
            JournalStreamId::Work(id),
        ) => id == value.work_id && intent.work_id == Some(value.work_id),
        (
            JournalEventPayload::ModelInvocationStarted(value)
            | JournalEventPayload::ModelInvocationStreaming(value)
            | JournalEventPayload::ModelInvocationCompleted(value)
            | JournalEventPayload::ModelInvocationFailed(value)
            | JournalEventPayload::ModelInvocationInterrupted(value),
            JournalStreamId::Work(id),
        ) => id == value.work_id && intent.work_id == Some(value.work_id),
        (
            JournalEventPayload::ToolExecutionRequested(value)
            | JournalEventPayload::ToolExecutionDispatching(value)
            | JournalEventPayload::ToolExecutionCompleted(value)
            | JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value)
            | JournalEventPayload::ToolExecutionOutcomeUnknown(value),
            JournalStreamId::Work(id),
        ) => id == value.work_id && intent.work_id == Some(value.work_id),
        (JournalEventPayload::ArtifactRecorded(value), JournalStreamId::Work(id)) => {
            id == value.work_id && intent.work_id == Some(value.work_id)
        }
        (JournalEventPayload::RuntimeStarted(value), JournalStreamId::Runtime(id)) => {
            id == value.runtime_instance_id
                && intent.runtime_instance_id == Some(value.runtime_instance_id)
                && intent.actor == JournalActor::Runtime(value.runtime_instance_id)
        }
        (JournalEventPayload::RuntimeRecoveryPerformed(value), JournalStreamId::Runtime(id)) => {
            id == value.runtime_instance_id
                && intent.runtime_instance_id == Some(value.runtime_instance_id)
                && intent.actor == JournalActor::Runtime(value.runtime_instance_id)
        }
        (JournalEventPayload::RuntimeStopping(value), JournalStreamId::Runtime(id)) => {
            id == value.runtime_instance_id
                && intent.runtime_instance_id == Some(value.runtime_instance_id)
                && intent.actor == JournalActor::Runtime(value.runtime_instance_id)
        }
        _ => false,
    };
    if links_match {
        Ok(())
    } else {
        Err(inconsistent())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommittedJournalPosition {
    pub offset: JournalOffset,
    pub stream_seq: StreamSeq,
}

pub(super) async fn allocate_stream_sequence(
    transaction: &mut WriteTransaction,
    stream_id: JournalStreamId,
) -> Result<StreamSeq, SqliteAdapterError> {
    let row = sqlx::query(
        "INSERT INTO stream_heads (stream_id, last_stream_seq) VALUES (?, 1) \
         ON CONFLICT(stream_id) DO UPDATE SET last_stream_seq = last_stream_seq + 1 \
         WHERE last_stream_seq < 9223372036854775807 \
         RETURNING last_stream_seq",
    )
    .bind(stream_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(|| SqliteAdapterError::new(SqliteFailureKind::InternalInvariant))?;
    StreamSeq::try_new(row.try_get::<i64, _>("last_stream_seq")?)
        .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InternalInvariant))
}

pub(super) async fn append_event(
    transaction: &mut WriteTransaction,
    prepared: PreparedJournalEvent,
) -> Result<CommittedJournalPosition, SqliteAdapterError> {
    let stream_seq = allocate_stream_sequence(transaction, prepared.intent.stream_id).await?;
    let actor_id = prepared.intent.actor.id();
    let row = sqlx::query(
        "INSERT INTO journal_events (event_id, craxii_id, stream_id, stream_seq, event_type, \
         event_version, conversation_id, work_id, causation_event_id, correlation_id, actor_kind, \
         actor_id, runtime_instance_id, payload_json, payload_sha256, recorded_at, occurred_at) \
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING journal_offset",
    )
    .bind(prepared.intent.event_id.to_string())
    .bind(prepared.intent.craxii_id.to_string())
    .bind(prepared.intent.stream_id.to_string())
    .bind(stream_seq.get())
    .bind(prepared.intent.payload.kind().as_str())
    .bind(prepared.intent.conversation_id.map(|id| id.to_string()))
    .bind(prepared.intent.work_id.map(|id| id.to_string()))
    .bind(prepared.intent.causation_event_id.map(|id| id.to_string()))
    .bind(prepared.intent.correlation_id.to_string())
    .bind(prepared.intent.actor.kind())
    .bind(actor_id)
    .bind(prepared.intent.runtime_instance_id.map(|id| id.to_string()))
    .bind(prepared.payload_json)
    .bind(prepared.payload_sha256.to_string())
    .bind(prepared.intent.recorded_at.to_string())
    .bind(prepared.intent.occurred_at.map(|at| at.to_string()))
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let offset = JournalOffset::try_new(row.try_get::<i64, _>("journal_offset")?)
        .map_err(|_| inconsistent())?;
    Ok(CommittedJournalPosition { offset, stream_seq })
}

pub(super) async fn insert_work_input(
    transaction: &mut WriteTransaction,
    work_id: WorkId,
    input: &WorkInputFactV1,
) -> Result<(), SqliteAdapterError> {
    if input.relationship == WorkInputRelationship::Trigger {
        if input.ordinal_within_work.get() != 1 || input.actor != WorkInputActor::User {
            return Err(inconsistent());
        }
        let row = sqlx::query(
            "SELECT event_type, journal_events.conversation_id AS input_conversation_id, \
             journal_events.correlation_id AS input_correlation_id, \
             work_items.conversation_id AS work_conversation_id, \
             work_items.correlation_id AS work_correlation_id \
             FROM journal_events JOIN work_items ON work_items.work_id = ? \
             WHERE journal_events.event_id = ?",
        )
        .bind(work_id.to_string())
        .bind(input.input_event_id.to_string())
        .fetch_optional(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .ok_or_else(inconsistent)?;
        if row.try_get::<String, _>("event_type")? != JournalEventKind::MessageAccepted.as_str()
            || row.try_get::<Option<String>, _>("input_conversation_id")?
                != Some(row.try_get::<String, _>("work_conversation_id")?)
            || row.try_get::<String, _>("input_correlation_id")?
                != row.try_get::<String, _>("work_correlation_id")?
        {
            return Err(inconsistent());
        }
    }
    sqlx::query(
        "INSERT INTO work_item_inputs (work_id, input_event_id, relationship, \
         ordinal_within_work, attached_at, attached_by_actor) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(work_id.to_string())
    .bind(input.input_event_id.to_string())
    .bind(match input.relationship {
        WorkInputRelationship::Trigger => "trigger",
        WorkInputRelationship::Steering => "steering",
        WorkInputRelationship::Supplemental => "supplemental",
        WorkInputRelationship::ScheduledTrigger => "scheduled_trigger",
        WorkInputRelationship::ExternalTrigger => "external_trigger",
        WorkInputRelationship::RecoveryInstruction => "recovery_instruction",
    })
    .bind(input.ordinal_within_work.get())
    .bind(input.attached_at.to_string())
    .bind(match input.actor {
        WorkInputActor::User => "user",
        WorkInputActor::Craxii => "craxii",
        WorkInputActor::System => "system",
        WorkInputActor::Recovery => "recovery",
    })
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    Ok(())
}

pub(super) async fn load_global_events(
    runtime: &super::runtime::SqliteRuntime,
) -> Result<Vec<JournalEvent>, SqliteAdapterError> {
    let mut connection = runtime.acquire().await?;
    let rows = sqlx::query("SELECT * FROM journal_events ORDER BY journal_offset ASC")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    rows.iter().map(decode_event_row).collect()
}

pub(super) async fn load_stream_events(
    runtime: &super::runtime::SqliteRuntime,
    stream_id: JournalStreamId,
) -> Result<Vec<JournalEvent>, SqliteAdapterError> {
    let mut connection = runtime.acquire().await?;
    let rows =
        sqlx::query("SELECT * FROM journal_events WHERE stream_id = ? ORDER BY stream_seq ASC")
            .bind(stream_id.to_string())
            .fetch_all(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    rows.iter().map(decode_event_row).collect()
}

pub(super) fn decode_event_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<JournalEvent, SqliteAdapterError> {
    let event_type = row.try_get::<String, _>("event_type")?;
    let event_version = row.try_get::<i64, _>("event_version")?;
    let payload_json = row.try_get::<String, _>("payload_json")?;
    let payload_sha256 = row.try_get::<String, _>("payload_sha256")?;
    let payload = decode_event_payload(&event_type, event_version, &payload_json, &payload_sha256)?;
    let actor_id = row.try_get::<Option<String>, _>("actor_id")?;
    let event = JournalEvent {
        journal_offset: JournalOffset::try_new(row.try_get("journal_offset")?)
            .map_err(|_| inconsistent())?,
        event_id: JournalEventId::parse_canonical(&row.try_get::<String, _>("event_id")?)
            .map_err(|_| inconsistent())?,
        craxii_id: CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        stream_id: row
            .try_get::<String, _>("stream_id")?
            .parse()
            .map_err(|_: JournalContractError| inconsistent())?,
        stream_seq: StreamSeq::try_new(row.try_get("stream_seq")?).map_err(|_| inconsistent())?,
        event_version,
        conversation_id: row
            .try_get::<Option<String>, _>("conversation_id")?
            .map(|id| ConversationId::parse_canonical(&id).map_err(|_| inconsistent()))
            .transpose()?,
        work_id: row
            .try_get::<Option<String>, _>("work_id")?
            .map(|id| WorkId::parse_canonical(&id).map_err(|_| inconsistent()))
            .transpose()?,
        causation_event_id: row
            .try_get::<Option<String>, _>("causation_event_id")?
            .map(|id| JournalEventId::parse_canonical(&id).map_err(|_| inconsistent()))
            .transpose()?,
        correlation_id: CorrelationId::parse_canonical(
            &row.try_get::<String, _>("correlation_id")?,
        )
        .map_err(|_| inconsistent())?,
        actor: JournalActor::parse(
            &row.try_get::<String, _>("actor_kind")?,
            actor_id.as_deref(),
        )
        .map_err(|_| inconsistent())?,
        runtime_instance_id: row
            .try_get::<Option<String>, _>("runtime_instance_id")?
            .map(|id| RuntimeInstanceId::parse_canonical(&id).map_err(|_| inconsistent()))
            .transpose()?,
        payload,
        payload_sha256: Sha256Digest::parse_canonical(&payload_sha256)
            .map_err(|_| inconsistent())?,
        recorded_at: UtcTimestamp::parse_canonical(&row.try_get::<String, _>("recorded_at")?)
            .map_err(|_| inconsistent())?,
        occurred_at: row
            .try_get::<Option<String>, _>("occurred_at")?
            .map(|at| UtcTimestamp::parse_canonical(&at).map_err(|_| inconsistent()))
            .transpose()?,
    };
    let intent = JournalAppendIntent {
        event_id: event.event_id,
        craxii_id: event.craxii_id,
        stream_id: event.stream_id,
        conversation_id: event.conversation_id,
        work_id: event.work_id,
        causation_event_id: event.causation_event_id,
        correlation_id: event.correlation_id,
        actor: event.actor,
        runtime_instance_id: event.runtime_instance_id,
        payload: event.payload.clone(),
        recorded_at: event.recorded_at,
        occurred_at: event.occurred_at,
    };
    validate_intent(&intent)?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    fn id<T: std::str::FromStr>() -> T
    where
        T::Err: std::fmt::Debug,
    {
        V7.parse().unwrap()
    }

    fn at() -> UtcTimestamp {
        "2026-08-28T00:00:00.000001Z".parse().unwrap()
    }

    fn transition(kind: JournalEventKind) -> WorkTransitionV1 {
        let (from_state, to_state, runtime_owner, current_attempt, cancellation_reason, terminal) =
            match kind {
                JournalEventKind::WorkStarted => (
                    WorkState::Queued,
                    WorkState::Running,
                    Some(id()),
                    JournalCurrentAttempt::None,
                    None,
                    None,
                ),
                JournalEventKind::WorkWaitingOnModel => (
                    WorkState::Running,
                    WorkState::WaitingOnModel,
                    Some(id()),
                    JournalCurrentAttempt::Model(id()),
                    None,
                    None,
                ),
                JournalEventKind::WorkWaitingOnTool => (
                    WorkState::Running,
                    WorkState::WaitingOnTool,
                    Some(id()),
                    JournalCurrentAttempt::Tool(id()),
                    None,
                    None,
                ),
                JournalEventKind::WorkResumed => (
                    WorkState::WaitingOnModel,
                    WorkState::Running,
                    Some(id()),
                    JournalCurrentAttempt::None,
                    None,
                    None,
                ),
                JournalEventKind::WorkCancelRequested => (
                    WorkState::Running,
                    WorkState::CancelRequested,
                    Some(id()),
                    JournalCurrentAttempt::None,
                    Some(WorkCancellationReason::UserRequest),
                    None,
                ),
                JournalEventKind::WorkCancelled => (
                    WorkState::Queued,
                    WorkState::Cancelled,
                    None,
                    JournalCurrentAttempt::None,
                    None,
                    Some(crate::domain::JournalWorkTerminalReason::UserRequest),
                ),
                JournalEventKind::WorkCompleted => (
                    WorkState::Running,
                    WorkState::Completed,
                    None,
                    JournalCurrentAttempt::None,
                    None,
                    Some(crate::domain::JournalWorkTerminalReason::Answered),
                ),
                JournalEventKind::WorkFailed => (
                    WorkState::Running,
                    WorkState::Failed,
                    None,
                    JournalCurrentAttempt::None,
                    None,
                    Some(crate::domain::JournalWorkTerminalReason::ProviderExhausted),
                ),
                JournalEventKind::WorkInterrupted => (
                    WorkState::Running,
                    WorkState::Interrupted,
                    None,
                    JournalCurrentAttempt::None,
                    None,
                    Some(crate::domain::JournalWorkTerminalReason::RuntimeOwnershipLost),
                ),
                _ => unreachable!(),
            };
        let (expected_runtime_owner, expected_current_attempt, expected_cancellation_reason) =
            match from_state {
                WorkState::Queued => (None, JournalCurrentAttempt::None, None),
                WorkState::Running => (Some(id()), JournalCurrentAttempt::None, None),
                WorkState::WaitingOnModel => (Some(id()), JournalCurrentAttempt::Model(id()), None),
                WorkState::WaitingOnTool => (Some(id()), JournalCurrentAttempt::Tool(id()), None),
                WorkState::CancelRequested => (
                    Some(id()),
                    JournalCurrentAttempt::None,
                    Some(WorkCancellationReason::UserRequest),
                ),
                WorkState::Completed
                | WorkState::Failed
                | WorkState::Cancelled
                | WorkState::Interrupted => unreachable!(),
            };
        WorkTransitionV1 {
            work_id: id(),
            from_state,
            to_state,
            expected_state_version: ProjectionVersion::try_new(1).unwrap(),
            expected_runtime_owner,
            expected_current_attempt,
            expected_cancellation_reason,
            state_version: ProjectionVersion::try_new(2).unwrap(),
            runtime_owner,
            current_attempt,
            cancellation_reason,
            terminal_reason: terminal,
            transitioned_at: at(),
        }
    }

    fn sample(kind: JournalEventKind) -> JournalEventPayload {
        match kind {
            JournalEventKind::CraxiiInitialized => {
                JournalEventPayload::CraxiiInitialized(CraxiiInitializedV1 {
                    craxii_id: id(),
                    display_name: "Craxii".into(),
                    owner_label: "local-owner".into(),
                    architecture_revision: "V0.0.01".into(),
                    schema_revision: SchemaVersion::try_new(2).unwrap(),
                    workstation_id: id(),
                    workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                    workstation_architecture: "aarch64".into(),
                    workstation_os_release: "linux".into(),
                    capabilities_sha256: Sha256Digest::hash_bytes(b"capabilities"),
                    workspace_id: id(),
                    workspace_logical_name: "primary".into(),
                    workspace_logical_root: "/workspace".into(),
                    primary_conversation_id: id(),
                    created_at: at(),
                })
            }
            JournalEventKind::ConversationCreated => {
                JournalEventPayload::ConversationCreated(ConversationCreatedV1 {
                    conversation_id: id(),
                    craxii_id: id(),
                    kind: ConversationKind::Primary,
                    lifecycle: ConversationLifecycle::Active,
                    next_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                    state_version: ProjectionVersion::try_new(1).unwrap(),
                    created_at: at(),
                })
            }
            JournalEventKind::MessageAccepted | JournalEventKind::AssistantMessageCommitted => {
                let assistant = kind == JournalEventKind::AssistantMessageCommitted;
                let content =
                    MessageContent::try_new(vec![ContentBlock::text("golden").unwrap()]).unwrap();
                let value = MessageCommittedV1 {
                    message_id: id(),
                    craxii_id: id(),
                    conversation_id: id(),
                    role: if assistant {
                        MessageRole::Assistant
                    } else {
                        MessageRole::User
                    },
                    content_sha256: content.content_sha256(),
                    content,
                    produced_by_work_id: assistant.then(id),
                    device_id: (!assistant).then(id),
                    client_message_id: (!assistant).then(id),
                    committed_at: at(),
                };
                if assistant {
                    JournalEventPayload::AssistantMessageCommitted(value)
                } else {
                    JournalEventPayload::MessageAccepted(value)
                }
            }
            JournalEventKind::WorkQueued => JournalEventPayload::WorkQueued(WorkQueuedV1 {
                work_id: id(),
                craxii_id: id(),
                conversation_id: id(),
                conversation_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                kind: WorkKind::Conversational,
                priority: 0,
                workspace_id: id(),
                correlation_id: id(),
                state_version: ProjectionVersion::try_new(1).unwrap(),
                created_at: at(),
                queued_at: at(),
                trigger: WorkInputFactV1 {
                    input_event_id: id(),
                    relationship: WorkInputRelationship::Trigger,
                    ordinal_within_work: WorkInputOrdinal::try_new(1).unwrap(),
                    attached_at: at(),
                    actor: WorkInputActor::User,
                },
            }),
            JournalEventKind::WorkStarted => JournalEventPayload::WorkStarted(transition(kind)),
            JournalEventKind::WorkWaitingOnModel => {
                JournalEventPayload::WorkWaitingOnModel(transition(kind))
            }
            JournalEventKind::WorkWaitingOnTool => {
                JournalEventPayload::WorkWaitingOnTool(transition(kind))
            }
            JournalEventKind::WorkResumed => JournalEventPayload::WorkResumed(transition(kind)),
            JournalEventKind::WorkCancelRequested => {
                JournalEventPayload::WorkCancelRequested(transition(kind))
            }
            JournalEventKind::WorkCancelled => JournalEventPayload::WorkCancelled(transition(kind)),
            JournalEventKind::WorkCompleted => JournalEventPayload::WorkCompleted(transition(kind)),
            JournalEventKind::WorkFailed => JournalEventPayload::WorkFailed(transition(kind)),
            JournalEventKind::WorkInterrupted => {
                JournalEventPayload::WorkInterrupted(transition(kind))
            }
            JournalEventKind::ModelInvocationStarted
            | JournalEventKind::ModelInvocationStreaming
            | JournalEventKind::ModelInvocationCompleted
            | JournalEventKind::ModelInvocationFailed
            | JournalEventKind::ModelInvocationInterrupted => {
                let state = match kind {
                    JournalEventKind::ModelInvocationStarted => ModelInvocationState::Requesting,
                    JournalEventKind::ModelInvocationStreaming => ModelInvocationState::Streaming,
                    JournalEventKind::ModelInvocationCompleted => ModelInvocationState::Completed,
                    JournalEventKind::ModelInvocationFailed => ModelInvocationState::Failed,
                    JournalEventKind::ModelInvocationInterrupted => {
                        ModelInvocationState::ProviderOutcomeUnknown
                    }
                    _ => unreachable!(),
                };
                let value = ModelInvocationEventV1 {
                    work_id: id(),
                    model_invocation_id: id(),
                    logical_invocation_id: id(),
                    state,
                    observed_at: at(),
                };
                match kind {
                    JournalEventKind::ModelInvocationStarted => {
                        JournalEventPayload::ModelInvocationStarted(value)
                    }
                    JournalEventKind::ModelInvocationStreaming => {
                        JournalEventPayload::ModelInvocationStreaming(value)
                    }
                    JournalEventKind::ModelInvocationCompleted => {
                        JournalEventPayload::ModelInvocationCompleted(value)
                    }
                    JournalEventKind::ModelInvocationFailed => {
                        JournalEventPayload::ModelInvocationFailed(value)
                    }
                    JournalEventKind::ModelInvocationInterrupted => {
                        JournalEventPayload::ModelInvocationInterrupted(value)
                    }
                    _ => unreachable!(),
                }
            }
            JournalEventKind::ToolExecutionRequested
            | JournalEventKind::ToolExecutionDispatching
            | JournalEventKind::ToolExecutionCompleted
            | JournalEventKind::ToolExecutionInterruptedBeforeDispatch
            | JournalEventKind::ToolExecutionOutcomeUnknown => {
                let state = match kind {
                    JournalEventKind::ToolExecutionRequested => ToolExecutionState::Requested,
                    JournalEventKind::ToolExecutionDispatching => ToolExecutionState::Dispatching,
                    JournalEventKind::ToolExecutionCompleted => ToolExecutionState::Completed,
                    JournalEventKind::ToolExecutionInterruptedBeforeDispatch => {
                        ToolExecutionState::InterruptedBeforeDispatch
                    }
                    JournalEventKind::ToolExecutionOutcomeUnknown => {
                        ToolExecutionState::OutcomeUnknown
                    }
                    _ => unreachable!(),
                };
                let value = ToolExecutionEventV1 {
                    work_id: id(),
                    tool_execution_id: id(),
                    state,
                    outcome_classification: (kind == JournalEventKind::ToolExecutionCompleted)
                        .then_some(ToolResultClass::Success),
                    observed_at: at(),
                };
                match kind {
                    JournalEventKind::ToolExecutionRequested => {
                        JournalEventPayload::ToolExecutionRequested(value)
                    }
                    JournalEventKind::ToolExecutionDispatching => {
                        JournalEventPayload::ToolExecutionDispatching(value)
                    }
                    JournalEventKind::ToolExecutionCompleted => {
                        JournalEventPayload::ToolExecutionCompleted(value)
                    }
                    JournalEventKind::ToolExecutionInterruptedBeforeDispatch => {
                        JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value)
                    }
                    JournalEventKind::ToolExecutionOutcomeUnknown => {
                        JournalEventPayload::ToolExecutionOutcomeUnknown(value)
                    }
                    _ => unreachable!(),
                }
            }
            JournalEventKind::ArtifactRecorded => {
                JournalEventPayload::ArtifactRecorded(ArtifactRecordedV1 {
                    work_id: id(),
                    artifact_id: id(),
                    sha256: Sha256Digest::hash_bytes(b"artifact"),
                    canonical_length: 8,
                    retention: ArtifactRetention::CanonicalEvidence,
                    recorded_at: at(),
                })
            }
            JournalEventKind::RuntimeStarted => {
                JournalEventPayload::RuntimeStarted(RuntimeStartedV1 {
                    runtime_instance_id: id(),
                    craxii_id: id(),
                    workstation_id: id(),
                    workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                    linux_boot_id: LinuxBootId::try_new("non_linux_not_applicable").unwrap(),
                    process_id: DiagnosticPid::try_new(42).unwrap(),
                    binary_version: PackageVersion::try_new("0.0.1").unwrap(),
                    git_revision: GitRevision::try_new("test").unwrap(),
                    schema_version: SchemaVersion::try_new(4).unwrap(),
                    started_at: at(),
                })
            }
            JournalEventKind::RuntimeRecoveryPerformed => {
                JournalEventPayload::RuntimeRecoveryPerformed(RuntimeRecoveryPerformedV1 {
                    runtime_instance_id: id(),
                    stale_runtimes_observed: 1,
                    stale_runtimes_closed: 1,
                    retained_queued_work: 1,
                    interrupted_work: 2,
                    model_attempts_provider_outcome_unknown: 3,
                    model_attempts_terminal_preserved: 4,
                    tool_attempts_interrupted_before_dispatch: 5,
                    tool_attempts_outcome_unknown: 6,
                    tool_attempts_terminal_preserved: 7,
                    drafts_abandoned: 8,
                    orphan_artifacts_observed: 9,
                    cleanup_checks_performed: 10,
                    cleanup_unconfirmed: 11,
                    recovery_duration_ms: 12,
                    binary_version: PackageVersion::try_new("0.0.1").unwrap(),
                    schema_version: SchemaVersion::try_new(4).unwrap(),
                    recovered_at: at(),
                })
            }
            JournalEventKind::RuntimeStopping => {
                JournalEventPayload::RuntimeStopping(RuntimeStoppingV1 {
                    runtime_instance_id: id(),
                    shutdown_requested_at: at(),
                    shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
                    grace_deadline: at(),
                    active_work_count: 1,
                    active_task_count: 2,
                })
            }
        }
    }

    #[test]
    fn exact_stored_bytes_are_hashed_without_reserialization() {
        let payload = JournalEventPayload::ConversationCreated(ConversationCreatedV1 {
            conversation_id: ConversationId::generate(),
            craxii_id: CraxiiId::generate(),
            kind: ConversationKind::Primary,
            lifecycle: ConversationLifecycle::Active,
            next_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
            state_version: ProjectionVersion::try_new(1).unwrap(),
            created_at: "2026-08-28T00:00:00.000001Z".parse().unwrap(),
        });
        let (json, digest) = encode_event_payload(&payload).unwrap();
        assert_eq!(digest, Sha256Digest::hash_bytes(json.as_bytes()));
        assert_eq!(
            decode_event_payload(payload.kind().as_str(), 1, &json, &digest.to_string()).unwrap(),
            payload
        );
        let changed = format!("{json} ");
        assert!(
            decode_event_payload(payload.kind().as_str(), 1, &changed, &digest.to_string())
                .is_err()
        );
    }

    #[test]
    fn unknown_fields_types_versions_and_malformed_payloads_fail_closed() {
        let digest = Sha256Digest::hash_bytes(b"{}");
        assert!(decode_event_payload("future.unknown", 1, "{}", &digest.to_string()).is_err());
        assert!(
            decode_event_payload("conversation.created", 2, "{}", &digest.to_string()).is_err()
        );
        let malformed = "{not-json}";
        let malformed_digest = Sha256Digest::hash_bytes(malformed.as_bytes());
        assert!(
            decode_event_payload(
                "conversation.created",
                1,
                malformed,
                &malformed_digest.to_string()
            )
            .is_err()
        );
        let extra = r#"{"conversation_id":"01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d","craxii_id":"01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d","kind":"primary","lifecycle":"active","next_work_ordinal":1,"state_version":1,"created_at":"2026-08-28T00:00:00.000001Z","extra":true}"#;
        let extra_digest = Sha256Digest::hash_bytes(extra.as_bytes());
        assert!(
            decode_event_payload("conversation.created", 1, extra, &extra_digest.to_string())
                .is_err()
        );
    }

    #[test]
    fn all_twenty_eight_v1_payloads_have_golden_exact_bytes_hashes_and_roundtrip() {
        let expected = [
            (
                JournalEventKind::CraxiiInitialized,
                "b4077973273f9f8bbadb4448c0bae9da27d0154fad27d160a074e74c034df3eb",
            ),
            (
                JournalEventKind::ConversationCreated,
                "c2627b5c3852734dfe0747e73c8d14ee4f9d3609891eedc5b3ad2ee89fd9dafe",
            ),
            (
                JournalEventKind::MessageAccepted,
                "7b62b177a7c0a2e9c73c51c659f926b8f099d835d581e0fb93459bbd9318b2c5",
            ),
            (
                JournalEventKind::WorkQueued,
                "f46b98378aeec653b2114678ea48e13fe84599a4d2bac4cf9346f305b9465138",
            ),
            (
                JournalEventKind::WorkStarted,
                "9a1f8ae71ad452aaacef99bc417020418a1181e58ea7797dbe04920aa9e7d527",
            ),
            (
                JournalEventKind::WorkWaitingOnModel,
                "70fd153bd550ef5a79ac603723df282393bf88a0e7f7c836ea660df4baa45e31",
            ),
            (
                JournalEventKind::WorkWaitingOnTool,
                "47c86dbb2713eb38debc15b42f8e9a25737af2db3520fa95689a5a2581ec6008",
            ),
            (
                JournalEventKind::WorkResumed,
                "7df7bfc46d691149dec8a95f4595b10e54d74e85cf3847a0cfdb04a492189df9",
            ),
            (
                JournalEventKind::WorkCancelRequested,
                "1df9e76dafaae244081f1bf0a2e04204edfce18e5fbee62b7b0c8b39bf8cc344",
            ),
            (
                JournalEventKind::WorkCancelled,
                "f4370697b4300e24db6c06a93ecae47e08e23e59fde1c3af596db4af79ae46f7",
            ),
            (
                JournalEventKind::WorkCompleted,
                "cd6f831f678d4d5b78aec2037a480fc38e35e7f41bfafd13b4901611836b6baf",
            ),
            (
                JournalEventKind::WorkFailed,
                "e397ba7e35add01d3fe0b1c5164ff0643e151116859d2edd83df268bcc747cd8",
            ),
            (
                JournalEventKind::WorkInterrupted,
                "41d1fc7fd621b2f09c355c571d85218906d105bc50fbf9cd4c43bd6cf1ffb747",
            ),
            (
                JournalEventKind::ModelInvocationStarted,
                "d4ea825576ee37e5df76f4af038a05a64a13991e365a91fd186a477400927645",
            ),
            (
                JournalEventKind::ModelInvocationStreaming,
                "e62bac3a4c181e84503d47b177ebeba1ca96bd0c76664500274c20b590d988d5",
            ),
            (
                JournalEventKind::ModelInvocationCompleted,
                "dc44f2d26bf94223e8d546866a12d53068207247ca3fc2845ed734a7c54f8fed",
            ),
            (
                JournalEventKind::ModelInvocationFailed,
                "c3daa15319d670cab9ea91699e63fe66950e231828be2616ce18195cba231f9a",
            ),
            (
                JournalEventKind::ModelInvocationInterrupted,
                "3d51aaac28e56437b7e01d4d384846abdc5fb224c619633b8de14b49cd8f79b3",
            ),
            (
                JournalEventKind::ToolExecutionRequested,
                "5987676427102884ebc7ce8c9067c7eb401176b9203dad55ac68a86b8b0cedf5",
            ),
            (
                JournalEventKind::ToolExecutionDispatching,
                "ce58dcc4b4f5b9fe2b51590129c6037a1fde90a899de05622c01a91e45e5e9d3",
            ),
            (
                JournalEventKind::ToolExecutionCompleted,
                "21b48ea7ee0ee995f7d4406c0fd3e78bb44b3b812091aca6486f639af6d33a6a",
            ),
            (
                JournalEventKind::ToolExecutionInterruptedBeforeDispatch,
                "b285c7e4019b95de3b4088f28d14d2c0c9289355ba8a6ca171a3c0d65e6d5f49",
            ),
            (
                JournalEventKind::ToolExecutionOutcomeUnknown,
                "e33619c8299a3e3f83b3ae400d0cf772f6a5ff8b8ec41bb0f13e0b69b2fe27bb",
            ),
            (
                JournalEventKind::AssistantMessageCommitted,
                "1ba5b1664564c2afcdd671c19316c740d1e776801a44ce3c81ea9fb594b03825",
            ),
            (
                JournalEventKind::ArtifactRecorded,
                "57e298656fe2962e6b3d8cece8ba275198414272b28f68c438671e43a16cd27f",
            ),
            (
                JournalEventKind::RuntimeStarted,
                "124ce4a11d3df09669a2db9bd55fff9ce018a4101f3a8c26a2118c6b1105e6b7",
            ),
            (
                JournalEventKind::RuntimeRecoveryPerformed,
                "1f8c7167e46c5d82b33b1afdd183118becb3ffe636673aab1ec52ba926704454",
            ),
            (
                JournalEventKind::RuntimeStopping,
                "34a6d68da2887d07530a8e5cd1ddda902d7f31872e989d316f36e2bf90ce8095",
            ),
        ];
        assert_eq!(expected.len(), JournalEventKind::ALL.len());
        for (kind, expected_digest) in expected {
            let payload = sample(kind);
            let (json, digest) = encode_event_payload(&payload).unwrap();
            assert_eq!(payload.kind(), kind);
            assert_eq!(digest.to_string(), expected_digest, "{}", kind.as_str());
            assert_eq!(digest, Sha256Digest::hash_bytes(json.as_bytes()));
            assert_eq!(
                encode_event_payload(&payload).unwrap().0.as_bytes(),
                json.as_bytes()
            );
            assert_eq!(
                decode_event_payload(kind.as_str(), 1, &json, &digest.to_string()).unwrap(),
                payload
            );
        }
    }
}
