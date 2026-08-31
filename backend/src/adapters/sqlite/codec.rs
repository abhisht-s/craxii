use std::fmt::Display;

use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};

use crate::domain::{
    ClientCommandId, ClientMessageId, ContentBlock, ConversationId, CorrelationId, CraxiiId,
    DeviceId, HostingProvider, LogicalPathKind, LogicalPathReference, Message, MessageContent,
    MessageId, MessageInput, MessageRole, ModelInvocationId, NormalizedError, RuntimeInstanceId,
    Sha256Digest, ToolExecutionId, UtcTimestamp, WorkCancellationReason, WorkCompletionReason,
    WorkId, WorkInterruptionReason, WorkState, WorkspaceCapabilityRef, WorkspaceId,
    WorkstationCapabilities, WorkstationCapabilitiesInput, WorkstationCapabilityFlags,
    WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits, WorkstationGeneration,
    WorkstationId, WorkstationIdentity, WorkstationIdentityInput, WorkstationKind,
};

use super::error::{SqliteAdapterError, SqliteFailureKind};

pub(super) trait CanonicalSqlId: Sized + Display {
    fn parse_sql(input: &str) -> Result<Self, SqliteAdapterError>;
}

macro_rules! sql_id_codec {
    ($($name:ty),+ $(,)?) => {
        $(
            impl CanonicalSqlId for $name {
                fn parse_sql(input: &str) -> Result<Self, SqliteAdapterError> {
                    <$name>::parse_canonical(input).map_err(|_| corrupt_row())
                }
            }
        )+
    };
}

sql_id_codec!(
    CraxiiId,
    ConversationId,
    MessageId,
    WorkId,
    WorkstationId,
    WorkspaceId,
    RuntimeInstanceId,
    DeviceId,
    ClientCommandId,
    ClientMessageId,
    ModelInvocationId,
    ToolExecutionId,
    CorrelationId,
);

pub(super) fn encode_id(value: impl CanonicalSqlId) -> String {
    value.to_string()
}

pub(super) fn decode_id<T: CanonicalSqlId>(value: &str) -> Result<T, SqliteAdapterError> {
    T::parse_sql(value)
}

pub(super) fn decode_optional_id<T: CanonicalSqlId>(
    value: Option<&str>,
) -> Result<Option<T>, SqliteAdapterError> {
    value.map(T::parse_sql).transpose()
}

pub(super) fn decode_timestamp(value: &str) -> Result<UtcTimestamp, SqliteAdapterError> {
    UtcTimestamp::parse_canonical(value).map_err(|_| corrupt_row())
}

pub(super) fn decode_optional_timestamp(
    value: Option<&str>,
) -> Result<Option<UtcTimestamp>, SqliteAdapterError> {
    value.map(decode_timestamp).transpose()
}

pub(super) fn decode_digest(value: &str) -> Result<Sha256Digest, SqliteAdapterError> {
    Sha256Digest::parse_canonical(value).map_err(|_| corrupt_row())
}

pub(super) fn decode_work_state(value: &str) -> Result<WorkState, SqliteAdapterError> {
    match value {
        "queued" => Ok(WorkState::Queued),
        "running" => Ok(WorkState::Running),
        "waiting_on_model" => Ok(WorkState::WaitingOnModel),
        "waiting_on_tool" => Ok(WorkState::WaitingOnTool),
        "cancel_requested" => Ok(WorkState::CancelRequested),
        "completed" => Ok(WorkState::Completed),
        "failed" => Ok(WorkState::Failed),
        "cancelled" => Ok(WorkState::Cancelled),
        "interrupted" => Ok(WorkState::Interrupted),
        _ => Err(corrupt_row()),
    }
}

pub(super) fn decode_cancellation_reason(
    value: &str,
) -> Result<WorkCancellationReason, SqliteAdapterError> {
    match value {
        "user_request" => Ok(WorkCancellationReason::UserRequest),
        "graceful_shutdown" => Ok(WorkCancellationReason::GracefulShutdown),
        _ => Err(corrupt_row()),
    }
}

pub(super) fn decode_completion_reason(
    value: &str,
) -> Result<WorkCompletionReason, SqliteAdapterError> {
    match value {
        "answered" => Ok(WorkCompletionReason::Answered),
        "refused" => Ok(WorkCompletionReason::Refused),
        _ => Err(corrupt_row()),
    }
}

pub(super) fn decode_interruption_reason(
    value: &str,
) -> Result<WorkInterruptionReason, SqliteAdapterError> {
    match value {
        "runtime_ownership_lost" => Ok(WorkInterruptionReason::RuntimeOwnershipLost),
        "provider_outcome_unknown" => Ok(WorkInterruptionReason::ProviderOutcomeUnknown),
        "tool_interrupted_before_dispatch" => {
            Ok(WorkInterruptionReason::ToolInterruptedBeforeDispatch)
        }
        "tool_outcome_unknown" => Ok(WorkInterruptionReason::ToolOutcomeUnknown),
        "cleanup_unconfirmed" => Ok(WorkInterruptionReason::CleanupUnconfirmed),
        _ => Err(corrupt_row()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkFailureCode {
    DefiniteNormalizedError,
    ProviderExhausted,
    InvalidModelOutput,
    LifecycleLimit,
}

impl WorkFailureCode {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::DefiniteNormalizedError => "definite_normalized_error",
            Self::ProviderExhausted => "provider_exhausted",
            Self::InvalidModelOutput => "invalid_model_output",
            Self::LifecycleLimit => "lifecycle_limit",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, SqliteAdapterError> {
        match value {
            "definite_normalized_error" => Ok(Self::DefiniteNormalizedError),
            "provider_exhausted" => Ok(Self::ProviderExhausted),
            "invalid_model_output" => Ok(Self::InvalidModelOutput),
            "lifecycle_limit" => Ok(Self::LifecycleLimit),
            _ => Err(corrupt_row()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeState {
    Running,
    Stopping,
    Stopped,
}

impl RuntimeState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, SqliteAdapterError> {
        match value {
            "running" => Ok(Self::Running),
            "stopping" => Ok(Self::Stopping),
            "stopped" => Ok(Self::Stopped),
            _ => Err(corrupt_row()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeStopReason {
    GracefulShutdown,
    StartupFailure,
}

impl RuntimeStopReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::GracefulShutdown => "graceful_shutdown",
            Self::StartupFailure => "startup_failure",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, SqliteAdapterError> {
        match value {
            "graceful_shutdown" => Ok(Self::GracefulShutdown),
            "startup_failure" => Ok(Self::StartupFailure),
            _ => Err(corrupt_row()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClientCommandType {
    Message,
    Cancel,
}

impl ClientCommandType {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Cancel => "cancel",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, SqliteAdapterError> {
        match value {
            "message" => Ok(Self::Message),
            "cancel" => Ok(Self::Cancel),
            _ => Err(corrupt_row()),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredWorkstationKindV1 {
    Local,
}

impl StoredWorkstationKindV1 {
    const fn from_domain(value: WorkstationKind) -> Self {
        match value {
            WorkstationKind::Local => Self::Local,
        }
    }

    const fn into_domain(self) -> WorkstationKind {
        match self {
            Self::Local => WorkstationKind::Local,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredLogicalPathKindV1 {
    WorkspaceRelative,
    Absolute,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredLogicalPathReferenceV1 {
    kind: StoredLogicalPathKindV1,
    canonical: String,
}

impl StoredLogicalPathReferenceV1 {
    fn from_domain(value: &LogicalPathReference) -> Self {
        let kind = match value.kind() {
            LogicalPathKind::WorkspaceRelative => StoredLogicalPathKindV1::WorkspaceRelative,
            LogicalPathKind::Absolute => StoredLogicalPathKindV1::Absolute,
        };
        Self {
            kind,
            canonical: value.canonical().to_owned(),
        }
    }

    fn into_domain(self) -> Result<LogicalPathReference, SqliteAdapterError> {
        match self.kind {
            StoredLogicalPathKindV1::WorkspaceRelative => {
                LogicalPathReference::workspace_relative(self.canonical)
            }
            StoredLogicalPathKindV1::Absolute => LogicalPathReference::absolute(self.canonical),
        }
        .map_err(|_| corrupt_row())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkstationCapabilityFlagsV1 {
    filesystem_read: bool,
    foreground_execute: bool,
    cancel_execution: bool,
    inspect_execution: bool,
    privilege_user: bool,
    privilege_administrative: bool,
    process_group_cleanup: bool,
    cgroup_cleanup: bool,
}

impl StoredWorkstationCapabilityFlagsV1 {
    const fn from_domain(value: WorkstationCapabilityFlags) -> Self {
        Self {
            filesystem_read: value.filesystem_read(),
            foreground_execute: value.foreground_execute(),
            cancel_execution: value.cancel_execution(),
            inspect_execution: value.inspect_execution(),
            privilege_user: value.privilege_user(),
            privilege_administrative: value.privilege_administrative(),
            process_group_cleanup: value.process_group_cleanup(),
            cgroup_cleanup: value.cgroup_cleanup(),
        }
    }

    const fn into_domain(self) -> WorkstationCapabilityFlags {
        WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
            filesystem_read: self.filesystem_read,
            foreground_execute: self.foreground_execute,
            cancel_execution: self.cancel_execution,
            inspect_execution: self.inspect_execution,
            privilege_user: self.privilege_user,
            privilege_administrative: self.privilege_administrative,
            process_group_cleanup: self.process_group_cleanup,
            cgroup_cleanup: self.cgroup_cleanup,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkstationCapabilityLimitsV1 {
    max_execution_timeout_ms: u64,
    max_stdout_bytes: u64,
    max_stderr_bytes: u64,
}

impl StoredWorkstationCapabilityLimitsV1 {
    const fn from_domain(value: WorkstationCapabilityLimits) -> Self {
        Self {
            max_execution_timeout_ms: value.max_execution_timeout_ms(),
            max_stdout_bytes: value.max_stdout_bytes(),
            max_stderr_bytes: value.max_stderr_bytes(),
        }
    }

    fn into_domain(self) -> Result<WorkstationCapabilityLimits, SqliteAdapterError> {
        WorkstationCapabilityLimits::try_new(
            self.max_execution_timeout_ms,
            self.max_stdout_bytes,
            self.max_stderr_bytes,
        )
        .map_err(|_| corrupt_row())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkspaceCapabilityRefV1 {
    workspace_id: String,
    logical_root: StoredLogicalPathReferenceV1,
}

impl StoredWorkspaceCapabilityRefV1 {
    fn from_domain(value: &WorkspaceCapabilityRef) -> Self {
        Self {
            workspace_id: encode_id(value.workspace_id()),
            logical_root: StoredLogicalPathReferenceV1::from_domain(value.logical_root()),
        }
    }

    fn into_domain(self) -> Result<WorkspaceCapabilityRef, SqliteAdapterError> {
        WorkspaceCapabilityRef::try_new(
            decode_id(&self.workspace_id)?,
            self.logical_root.into_domain()?,
        )
        .map_err(|_| corrupt_row())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkstationCapabilitiesV1 {
    version: i64,
    workstation_id: String,
    generation: i64,
    kind: StoredWorkstationKindV1,
    architecture: String,
    os_release: String,
    default_shell: StoredLogicalPathReferenceV1,
    flags: StoredWorkstationCapabilityFlagsV1,
    limits: StoredWorkstationCapabilityLimitsV1,
    workspaces: Vec<StoredWorkspaceCapabilityRefV1>,
}

pub(super) fn encode_workstation_capabilities(
    capabilities: &WorkstationCapabilities,
) -> Result<String, SqliteAdapterError> {
    if capabilities.version().get() != 1 {
        return Err(corrupt_row());
    }
    let stored = StoredWorkstationCapabilitiesV1 {
        version: 1,
        workstation_id: encode_id(capabilities.workstation_id()),
        generation: capabilities.generation().get(),
        kind: StoredWorkstationKindV1::from_domain(capabilities.kind()),
        architecture: capabilities.cpu_architecture().to_owned(),
        os_release: capabilities.os_release().to_owned(),
        default_shell: StoredLogicalPathReferenceV1::from_domain(capabilities.default_shell()),
        flags: StoredWorkstationCapabilityFlagsV1::from_domain(capabilities.flags()),
        limits: StoredWorkstationCapabilityLimitsV1::from_domain(capabilities.limits()),
        workspaces: capabilities
            .workspaces()
            .iter()
            .map(StoredWorkspaceCapabilityRefV1::from_domain)
            .collect(),
    };
    serde_json::to_string(&stored).map_err(|_| corrupt_row())
}

pub(super) fn decode_workstation_capabilities(
    capabilities_json: &str,
) -> Result<WorkstationCapabilities, SqliteAdapterError> {
    let stored: StoredWorkstationCapabilitiesV1 =
        serde_json::from_str(capabilities_json).map_err(|_| corrupt_row())?;
    if stored.version != 1 {
        return Err(corrupt_row());
    }
    let kind = stored.kind.into_domain();
    let capabilities = WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
        workstation_id: decode_id(&stored.workstation_id)?,
        generation: decode_workstation_generation(stored.generation)?,
        cpu_architecture: stored.architecture,
        os_release: stored.os_release,
        default_shell: stored.default_shell.into_domain()?,
        flags: stored.flags.into_domain(),
        limits: stored.limits.into_domain()?,
        workspaces: stored
            .workspaces
            .into_iter()
            .map(StoredWorkspaceCapabilityRefV1::into_domain)
            .collect::<Result<Vec<_>, _>>()?,
    })
    .map_err(|_| corrupt_row())?;
    if capabilities.kind() != kind {
        return Err(corrupt_row());
    }
    Ok(capabilities)
}

pub(super) fn decode_workstation_generation(
    value: i64,
) -> Result<WorkstationGeneration, SqliteAdapterError> {
    WorkstationGeneration::try_new(value).map_err(|_| corrupt_row())
}

fn decode_workstation_kind(value: &str) -> Result<WorkstationKind, SqliteAdapterError> {
    match value {
        "local" => Ok(WorkstationKind::Local),
        _ => Err(corrupt_row()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DecodedWorkstationRecord {
    identity: WorkstationIdentity,
    capabilities: WorkstationCapabilities,
    last_seen_at: UtcTimestamp,
}

impl DecodedWorkstationRecord {
    pub(super) const fn identity(&self) -> &WorkstationIdentity {
        &self.identity
    }

    pub(super) const fn capabilities(&self) -> &WorkstationCapabilities {
        &self.capabilities
    }

    pub(super) const fn last_seen_at(&self) -> UtcTimestamp {
        self.last_seen_at
    }
}

pub(super) fn decode_workstation_row(
    row: &SqliteRow,
) -> Result<DecodedWorkstationRecord, SqliteAdapterError> {
    let workstation_id = decode_id(&row.try_get::<String, _>("workstation_id")?)?;
    let craxii_id = decode_id(&row.try_get::<String, _>("craxii_id")?)?;
    let kind = decode_workstation_kind(&row.try_get::<String, _>("kind")?)?;
    let generation = decode_workstation_generation(row.try_get::<i64, _>("generation")?)?;
    let hosting_provider = HostingProvider::try_new(row.try_get::<String, _>("hosting_provider")?)
        .map_err(|_| corrupt_row())?;
    let provider_instance_id = row.try_get::<Option<String>, _>("provider_instance_id")?;
    let image_id = row.try_get::<Option<String>, _>("provider_image_id")?;
    let provisioning_revision = row.try_get::<Option<String>, _>("provisioning_revision")?;
    let cpu_architecture = row.try_get::<String, _>("architecture")?;
    let os_release = row.try_get::<String, _>("os_release")?;
    let capabilities_json = row.try_get::<String, _>("capabilities_json")?;
    let created_at = decode_timestamp(&row.try_get::<String, _>("created_at")?)?;
    let last_seen_at = decode_timestamp(&row.try_get::<String, _>("last_seen_at")?)?;

    let identity = WorkstationIdentity::try_new(WorkstationIdentityInput {
        workstation_id,
        craxii_id,
        generation,
        hosting_provider,
        provider_instance_id,
        image_id,
        provisioning_revision,
        cpu_architecture,
        os_release,
        created_at,
    })
    .map_err(|_| corrupt_row())?;
    let capabilities = decode_workstation_capabilities(&capabilities_json)?;

    if capabilities.workstation_id() != identity.workstation_id()
        || capabilities.generation() != identity.generation()
        || capabilities.kind() != kind
        || capabilities.kind() != identity.kind()
        || capabilities.cpu_architecture() != identity.cpu_architecture()
        || capabilities.os_release() != identity.os_release()
    {
        return Err(corrupt_row());
    }

    Ok(DecodedWorkstationRecord {
        identity,
        capabilities,
        last_seen_at,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessageContent {
    version: u8,
    blocks: Vec<StoredContentBlock>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum StoredContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

pub(super) fn encode_message_content(
    content: &MessageContent,
) -> Result<(String, Sha256Digest), SqliteAdapterError> {
    let stored = StoredMessageContent {
        version: 1,
        blocks: content
            .blocks()
            .iter()
            .map(|block| StoredContentBlock::Text {
                text: block.as_text().to_owned(),
            })
            .collect(),
    };
    let json = serde_json::to_string(&stored).map_err(|_| corrupt_row())?;
    Ok((json, content.content_sha256()))
}

pub(super) fn decode_message_content(
    content_json: &str,
    stored_digest: &str,
) -> Result<MessageContent, SqliteAdapterError> {
    let stored: StoredMessageContent =
        serde_json::from_str(content_json).map_err(|_| corrupt_row())?;
    if stored.version != 1 {
        return Err(corrupt_row());
    }
    let blocks = stored
        .blocks
        .into_iter()
        .map(|block| match block {
            StoredContentBlock::Text { text } => {
                ContentBlock::text(text).map_err(|_| corrupt_row())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let content = MessageContent::try_new(blocks).map_err(|_| corrupt_row())?;
    let digest = decode_digest(stored_digest)?;
    if content.content_sha256() != digest {
        return Err(corrupt_row());
    }
    Ok(content)
}

pub(super) fn decode_message_row(row: &SqliteRow) -> Result<Message, SqliteAdapterError> {
    let message_id = decode_id(&row.try_get::<String, _>("message_id")?)?;
    let craxii_id = decode_id(&row.try_get::<String, _>("craxii_id")?)?;
    let conversation_id = decode_id(&row.try_get::<String, _>("conversation_id")?)?;
    let role = match row.try_get::<String, _>("role")?.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => return Err(corrupt_row()),
    };
    let content_json = row.try_get::<String, _>("content_json")?;
    let content_sha256 = row.try_get::<String, _>("content_sha256")?;
    let content = decode_message_content(&content_json, &content_sha256)?;
    let produced_by_work_id = decode_optional_id(
        row.try_get::<Option<String>, _>("produced_by_work_id")?
            .as_deref(),
    )?;
    let device_id = decode_optional_id(
        row.try_get::<Option<String>, _>("client_device_id")?
            .as_deref(),
    )?;
    let client_message_id = decode_optional_id(
        row.try_get::<Option<String>, _>("client_message_id")?
            .as_deref(),
    )?;
    let committed_at = decode_timestamp(&row.try_get::<String, _>("committed_at")?)?;

    Message::try_new(MessageInput {
        message_id,
        craxii_id,
        conversation_id,
        role,
        content,
        produced_by_work_id,
        device_id,
        client_message_id,
        committed_at,
    })
    .map_err(|_| corrupt_row())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PersistedNormalizedError {
    pub(super) category: String,
    pub(super) code: String,
    pub(super) retryability: String,
    pub(super) certainty: String,
    pub(super) safe_message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source_status: Option<PersistedSourceStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PersistedSourceStatus {
    kind: String,
    code: i32,
}

pub(super) fn encode_normalized_error_detail(
    error: &NormalizedError,
) -> Result<String, SqliteAdapterError> {
    encode_persisted_normalized_error(error, false)
}

pub(super) fn encode_attempt_normalized_error(
    error: &NormalizedError,
    allow_outcome_unknown: bool,
) -> Result<String, SqliteAdapterError> {
    encode_persisted_normalized_error(error, allow_outcome_unknown)
}

fn encode_persisted_normalized_error(
    error: &NormalizedError,
    allow_outcome_unknown: bool,
) -> Result<String, SqliteAdapterError> {
    if error.certainty().as_str() != "definite"
        && (!allow_outcome_unknown || error.certainty().as_str() != "outcome_unknown")
    {
        return Err(corrupt_row());
    }
    let stored = PersistedNormalizedError {
        category: error.category().as_str().to_owned(),
        code: error.code().as_str().to_owned(),
        retryability: error.retryability().as_str().to_owned(),
        certainty: error.certainty().as_str().to_owned(),
        safe_message: error.safe_message().as_str().to_owned(),
        source_status: error.source_status().map(|status| PersistedSourceStatus {
            kind: status.kind().to_owned(),
            code: status.code(),
        }),
    };
    serde_json::to_string(&stored).map_err(|_| corrupt_row())
}

pub(super) fn decode_normalized_error_detail(
    json: &str,
) -> Result<PersistedNormalizedError, SqliteAdapterError> {
    decode_persisted_normalized_error(json, false)
}

pub(super) fn decode_attempt_normalized_error(
    json: &str,
    allow_outcome_unknown: bool,
) -> Result<PersistedNormalizedError, SqliteAdapterError> {
    decode_persisted_normalized_error(json, allow_outcome_unknown)
}

fn decode_persisted_normalized_error(
    json: &str,
    allow_outcome_unknown: bool,
) -> Result<PersistedNormalizedError, SqliteAdapterError> {
    let stored: PersistedNormalizedError = serde_json::from_str(json).map_err(|_| corrupt_row())?;
    if !valid_persisted_normalized_error(&stored, allow_outcome_unknown) {
        return Err(corrupt_row());
    }
    Ok(stored)
}

fn valid_persisted_normalized_error(
    stored: &PersistedNormalizedError,
    allow_outcome_unknown: bool,
) -> bool {
    if stored.certainty != "definite"
        && (!allow_outcome_unknown || stored.certainty != "outcome_unknown")
    {
        return false;
    }
    let (code, message, retryabilities, source_allowed) = match stored.category.as_str() {
        "authentication_error" => (
            "authentication_error",
            "Authentication is required.",
            &["user_action"][..],
            false,
        ),
        "client_protocol_error" if stored.code == "domain_validation" => (
            "domain_validation",
            "The supplied value is invalid.",
            &["never"][..],
            false,
        ),
        "client_protocol_error" => (
            "client_protocol_error",
            "The request is invalid.",
            &["never"][..],
            false,
        ),
        "idempotency_error" => (
            "idempotency_error",
            "The request conflicts with an earlier request.",
            &["user_action"][..],
            false,
        ),
        "storage_error" => (
            "storage_error",
            "A storage operation failed.",
            &["operator_action"][..],
            true,
        ),
        "state_conflict" => (
            "state_conflict",
            "The requested operation conflicts with the current state.",
            &["bounded"][..],
            false,
        ),
        "context_error" => (
            if stored.code == "context_limit_exceeded" {
                "context_limit_exceeded"
            } else {
                "context_error"
            },
            "The requested context cannot be processed.",
            &["never"][..],
            false,
        ),
        "model_selection_error" => (
            "model_selection_error",
            "No suitable model is currently available.",
            &["operator_action"][..],
            false,
        ),
        "provider_error" => (
            "provider_error",
            "The model provider request failed.",
            &["never", "bounded"][..],
            true,
        ),
        "tool_validation_error" => (
            "tool_validation_error",
            "The tool request is invalid.",
            &["never"][..],
            false,
        ),
        "authority_error" => (
            "authority_error",
            "The requested operation is not permitted.",
            &["never"][..],
            false,
        ),
        "workstation_error" => (
            "workstation_error",
            "The workstation operation failed.",
            &["never"][..],
            true,
        ),
        "artifact_error" => (
            "artifact_error",
            "The artifact operation failed.",
            &["operator_action"][..],
            true,
        ),
        "cancellation_error" => (
            "cancellation_error",
            "The operation could not be confirmed as cancelled.",
            &["never"][..],
            false,
        ),
        "internal_invariant_error" => (
            "internal_invariant_error",
            "An internal consistency error occurred.",
            &["operator_action"][..],
            false,
        ),
        _ => return false,
    };
    if stored.code != code
        || stored.safe_message != message
        || !retryabilities.contains(&stored.retryability.as_str())
    {
        return false;
    }
    match &stored.source_status {
        None => true,
        Some(status) if source_allowed => match status.kind.as_str() {
            "provider_http" => (100..=599).contains(&status.code),
            "os_errno" => status.code > 0,
            _ => false,
        },
        Some(_) => false,
    }
}

fn corrupt_row() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Certainty, SourceStatus};
    use serde_json::{Value, json};

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";
    const OTHER_V7: &str = "01890f6c-7b3a-7cc0-a8f1-2e6f7a8b9c0d";

    fn content(texts: &[&str]) -> MessageContent {
        MessageContent::try_new(
            texts
                .iter()
                .map(|text| ContentBlock::text(*text).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn workstation_capabilities() -> WorkstationCapabilities {
        WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
            workstation_id: WorkstationId::parse_canonical(V7).unwrap(),
            generation: WorkstationGeneration::try_new(7).unwrap(),
            cpu_architecture: "x86_64".to_owned(),
            os_release: "Ubuntu 24.04 LTS".to_owned(),
            default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
            flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
                filesystem_read: true,
                foreground_execute: true,
                cancel_execution: true,
                inspect_execution: true,
                privilege_user: true,
                privilege_administrative: false,
                process_group_cleanup: true,
                cgroup_cleanup: true,
            }),
            limits: WorkstationCapabilityLimits::try_new(60_000, 1_048_576, 524_288).unwrap(),
            workspaces: vec![
                WorkspaceCapabilityRef::try_new(
                    WorkspaceId::parse_canonical(OTHER_V7).unwrap(),
                    LogicalPathReference::absolute("/srv/craxii/workspaces/main").unwrap(),
                )
                .unwrap(),
            ],
        })
        .unwrap()
    }

    fn workstation_capabilities_value() -> Value {
        serde_json::from_str(&encode_workstation_capabilities(&workstation_capabilities()).unwrap())
            .unwrap()
    }

    fn assert_capabilities_value_rejected(value: Value) {
        let json = serde_json::to_string(&value).unwrap();
        assert!(decode_workstation_capabilities(&json).is_err(), "{json}");
    }

    #[test]
    fn every_stage_6_id_uses_the_strict_canonical_codec() {
        macro_rules! roundtrip {
            ($name:ty) => {{
                let decoded: $name = decode_id(V7).unwrap();
                assert_eq!(encode_id(decoded), V7);
                assert!(decode_id::<$name>(&V7.to_uppercase()).is_err());
            }};
        }
        roundtrip!(CraxiiId);
        roundtrip!(ConversationId);
        roundtrip!(MessageId);
        roundtrip!(WorkId);
        roundtrip!(WorkstationId);
        roundtrip!(WorkspaceId);
        roundtrip!(RuntimeInstanceId);
        roundtrip!(DeviceId);
        roundtrip!(ClientCommandId);
        roundtrip!(ClientMessageId);
        roundtrip!(ModelInvocationId);
        roundtrip!(ToolExecutionId);
        roundtrip!(CorrelationId);
        assert!(decode_id::<WorkId>("not-a-uuid").is_err());
    }

    #[test]
    fn timestamp_digest_and_closed_literals_fail_closed() {
        let timestamp = decode_timestamp("2026-08-28T01:02:03.456789Z").unwrap();
        assert_eq!(timestamp.to_string(), "2026-08-28T01:02:03.456789Z");
        assert!(decode_timestamp("2026-02-30T01:02:03.456789Z").is_err());
        let digest = decode_digest(&"a".repeat(64)).unwrap();
        assert_eq!(digest.to_string(), "a".repeat(64));
        assert!(decode_digest(&"A".repeat(64)).is_err());

        for state in WorkState::ALL {
            assert_eq!(decode_work_state(state.as_str()).unwrap(), *state);
        }
        assert!(decode_work_state("unknown").is_err());
        for reason in WorkCancellationReason::ALL {
            assert_eq!(
                decode_cancellation_reason(reason.as_str()).unwrap(),
                *reason
            );
        }
        for reason in WorkCompletionReason::ALL {
            assert_eq!(decode_completion_reason(reason.as_str()).unwrap(), *reason);
        }
        for reason in WorkInterruptionReason::ALL {
            assert_eq!(
                decode_interruption_reason(reason.as_str()).unwrap(),
                *reason
            );
        }
        for reason in [
            WorkFailureCode::DefiniteNormalizedError,
            WorkFailureCode::ProviderExhausted,
            WorkFailureCode::InvalidModelOutput,
            WorkFailureCode::LifecycleLimit,
        ] {
            assert_eq!(WorkFailureCode::parse(reason.as_str()).unwrap(), reason);
        }
        assert!(WorkFailureCode::parse("unknown_failure").is_err());
        for state in [
            RuntimeState::Running,
            RuntimeState::Stopping,
            RuntimeState::Stopped,
        ] {
            assert_eq!(RuntimeState::parse(state.as_str()).unwrap(), state);
        }
        for reason in [
            RuntimeStopReason::GracefulShutdown,
            RuntimeStopReason::StartupFailure,
        ] {
            assert_eq!(RuntimeStopReason::parse(reason.as_str()).unwrap(), reason);
        }
        for command in [ClientCommandType::Message, ClientCommandType::Cancel] {
            assert_eq!(ClientCommandType::parse(command.as_str()).unwrap(), command);
        }
    }

    #[test]
    fn workstation_capabilities_v1_roundtrips_every_domain_field() {
        let original = workstation_capabilities();
        let first = encode_workstation_capabilities(&original).unwrap();
        let second = encode_workstation_capabilities(&original).unwrap();
        assert_eq!(first, second);
        assert_eq!(decode_workstation_capabilities(&first).unwrap(), original);

        let stored: Value = serde_json::from_str(&first).unwrap();
        assert_eq!(stored["version"], 1);
        assert_eq!(stored["kind"], "local");
        assert_eq!(stored["default_shell"]["kind"], "absolute");
        assert_eq!(stored["workspaces"][0]["logical_root"]["kind"], "absolute");
        assert!(stored.get("provider_instance_id").is_none());
        assert!(stored.get("local_resolved_root").is_none());
    }

    #[test]
    fn workstation_capabilities_v1_rejects_structural_corruption() {
        let mut unknown_version = workstation_capabilities_value();
        unknown_version["version"] = json!(2);
        assert_capabilities_value_rejected(unknown_version);

        let mut unknown_field = workstation_capabilities_value();
        unknown_field["unexpected"] = json!(true);
        assert_capabilities_value_rejected(unknown_field);

        let mut nested_unknown_field = workstation_capabilities_value();
        nested_unknown_field["flags"]["unexpected"] = json!(true);
        assert_capabilities_value_rejected(nested_unknown_field);

        let mut missing_field = workstation_capabilities_value();
        missing_field
            .as_object_mut()
            .unwrap()
            .remove("default_shell");
        assert_capabilities_value_rejected(missing_field);

        let mut missing_nested_field = workstation_capabilities_value();
        missing_nested_field["limits"]
            .as_object_mut()
            .unwrap()
            .remove("max_stderr_bytes");
        assert_capabilities_value_rejected(missing_nested_field);

        let mut invalid_kind = workstation_capabilities_value();
        invalid_kind["kind"] = json!("remote");
        assert_capabilities_value_rejected(invalid_kind);

        let mut invalid_flag = workstation_capabilities_value();
        invalid_flag["flags"]["filesystem_read"] = json!(1);
        assert_capabilities_value_rejected(invalid_flag);

        for json in ["not-json", "{}"] {
            assert!(decode_workstation_capabilities(json).is_err());
        }
    }

    #[test]
    fn workstation_capabilities_v1_rejects_domain_invariants() {
        let mut invalid_workstation_id = workstation_capabilities_value();
        invalid_workstation_id["workstation_id"] = json!("not-a-workstation-id");
        assert_capabilities_value_rejected(invalid_workstation_id);

        let mut zero_generation = workstation_capabilities_value();
        zero_generation["generation"] = json!(0);
        assert_capabilities_value_rejected(zero_generation);

        let mut overflowing_generation = workstation_capabilities_value();
        overflowing_generation["generation"] = json!(i64::MAX as u64 + 1);
        assert_capabilities_value_rejected(overflowing_generation);

        let mut empty_architecture = workstation_capabilities_value();
        empty_architecture["architecture"] = json!("");
        assert_capabilities_value_rejected(empty_architecture);

        let mut invalid_os_release = workstation_capabilities_value();
        invalid_os_release["os_release"] = json!(" Ubuntu 24.04 LTS");
        assert_capabilities_value_rejected(invalid_os_release);

        let mut invalid_limit = workstation_capabilities_value();
        invalid_limit["limits"]["max_execution_timeout_ms"] = json!(i64::MAX as u64 + 1);
        assert_capabilities_value_rejected(invalid_limit);

        let mut duplicate_workspace = workstation_capabilities_value();
        let duplicate = duplicate_workspace["workspaces"][0].clone();
        duplicate_workspace["workspaces"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_capabilities_value_rejected(duplicate_workspace);

        let mut invalid_workspace_id = workstation_capabilities_value();
        invalid_workspace_id["workspaces"][0]["workspace_id"] = json!("not-a-workspace-id");
        assert_capabilities_value_rejected(invalid_workspace_id);

        let mut relative_workspace_root = workstation_capabilities_value();
        relative_workspace_root["workspaces"][0]["logical_root"] =
            json!({"kind": "workspace_relative", "canonical": "main"});
        assert_capabilities_value_rejected(relative_workspace_root);

        let mut invalid_default_shell = workstation_capabilities_value();
        invalid_default_shell["default_shell"] =
            json!({"kind": "absolute", "canonical": "bin/bash"});
        assert_capabilities_value_rejected(invalid_default_shell);
    }

    #[test]
    fn workstation_generation_reconstruction_is_checked_and_errors_are_redacted() {
        assert_eq!(decode_workstation_generation(1).unwrap().get(), 1);
        assert_eq!(
            decode_workstation_generation(i64::MAX).unwrap().get(),
            i64::MAX
        );
        for value in [0, -1, i64::MIN] {
            assert!(decode_workstation_generation(value).is_err());
        }

        let sentinel = "/secret/workspace provider-secret malformed-json";
        let error = decode_workstation_capabilities(sentinel).unwrap_err();
        for surface in [error.to_string(), format!("{error:?}")] {
            assert!(!surface.contains(sentinel));
            assert!(!surface.contains("secret"));
            assert!(!surface.contains("json"));
        }
        assert_eq!(error.kind(), SqliteFailureKind::InconsistentSchema);
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn message_content_v1_roundtrips_exact_utf8_order_and_hash() {
        let original = content(&["first\n", "é🙂", "  preserved  "]);
        let (json, digest) = encode_message_content(&original).unwrap();
        assert_eq!(
            json,
            "{\"version\":1,\"blocks\":[{\"type\":\"text\",\"text\":\"first\\n\"},{\"type\":\"text\",\"text\":\"é🙂\"},{\"type\":\"text\",\"text\":\"  preserved  \"}]}"
        );
        let decoded = decode_message_content(&json, &digest.to_string()).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(decoded.canonical_bytes(), original.canonical_bytes());
    }

    #[test]
    fn message_content_corruption_is_never_coerced() {
        let valid = content(&["ok"]);
        let (_, digest) = encode_message_content(&valid).unwrap();
        let digest = digest.to_string();
        for json in [
            r#"{"version":1,"blocks":[{"type":"text","text":""}]}"#,
            r#"{"version":2,"blocks":[{"type":"text","text":"ok"}]}"#,
            r#"{"version":1,"blocks":[{"type":"image","text":"ok"}]}"#,
            r#"{"version":1,"blocks":[{"type":"text","text":"ok","extra":1}]}"#,
            r#"{"version":1,"blocks":[{"type":"text","text":"ok"}],"extra":1}"#,
            r#"{"version":1,"blocks":[]}"#,
            "not-json",
        ] {
            assert!(decode_message_content(json, &digest).is_err(), "{json}");
        }
        assert!(
            decode_message_content(
                r#"{"version":1,"blocks":[{"type":"text","text":"changed"}]}"#,
                &digest,
            )
            .is_err()
        );
    }

    #[test]
    fn normalized_error_detail_contains_only_safe_allowlisted_fields() {
        let error = NormalizedError::provider_bounded(
            Certainty::Definite,
            SourceStatus::provider_http(503),
        );
        let json = encode_normalized_error_detail(&error).unwrap();
        assert!(!json.contains("internal_detail"));
        let decoded = decode_normalized_error_detail(&json).unwrap();
        assert_eq!(decoded.category, "provider_error");
        assert_eq!(decoded.retryability, "bounded");
        assert_eq!(decoded.source_status.unwrap().code, 503);
        assert!(decode_normalized_error_detail(
            r#"{"category":"provider_error","code":"provider_error","retryability":"bounded","certainty":"definite","safe_message":"The model provider request failed.","internal_detail":"secret"}"#,
        )
        .is_err());
        assert!(
            encode_normalized_error_detail(&NormalizedError::provider(
                Certainty::OutcomeUnknown,
                None,
            ))
            .is_err()
        );
    }
}
