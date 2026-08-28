//! Immutable Stage 3.2 evidence and neutral provider/tool/artifact references.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    AgentStepNo, ArtifactId, AttemptNo, CanonicalByteCount, ContextManifestId, CraxiiId,
    DomainValidationError, DomainValidationKind, ExecutionId, LogicalInvocationId,
    LogicalPathReference, MAX_LOGICAL_PATH_BYTES, ModelInvocationId, RuntimeInstanceId,
    SchemaVersion, Sha256Digest, ToolExecutionId, ToolOrdinal, UtcTimestamp, WorkId, WorkspaceId,
    WorkstationGeneration, WorkstationId,
};

macro_rules! bounded_reference {
    ($name:ident, $validator:ident, $max:expr, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Validates and preserves the exact canonical reference string.
            pub fn try_new(value: impl Into<String>) -> Result<Self, DomainValidationError> {
                let value = value.into();
                if !$validator(&value, $max) {
                    return Err(invalid_identifier());
                }
                Ok(Self(value))
            }

            /// Returns the exact preserved reference string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }
    };
}

fn invalid_identifier() -> DomainValidationError {
    DomainValidationError::new(DomainValidationKind::InvalidBoundedIdentifier)
}

fn lower_reference(value: &str, max: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && value.is_ascii()
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn reason_code(value: &str, max: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && value.is_ascii()
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn provider_model(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn visible_ascii(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn bounded_opaque(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn bounded_trimmed(value: &str, max: usize) -> bool {
    bounded_opaque(value, max) && value.trim() == value
}

bounded_reference!(
    ProviderId,
    lower_reference,
    64,
    "A canonical model-provider identifier."
);
bounded_reference!(
    ModelTargetId,
    lower_reference,
    64,
    "A canonical configured model-target identifier."
);
bounded_reference!(
    ProviderModelId,
    provider_model,
    128,
    "A provider-native model name preserved without normalization."
);
bounded_reference!(
    ToolName,
    lower_reference,
    64,
    "A canonical registered tool name."
);
bounded_reference!(
    ToolVersion,
    visible_ascii,
    64,
    "A canonical visible-ASCII tool version."
);
bounded_reference!(
    AuthorityReasonCode,
    reason_code,
    64,
    "A canonical authority reason code."
);
/// Canonical content-addressed artifact storage key with no absolute-path meaning.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactStorageKey(String);

impl ArtifactStorageKey {
    /// Derives `sha256/<two-hex>/<digest>` from captured-byte identity.
    #[must_use]
    pub fn from_digest(digest: Sha256Digest) -> Self {
        let digest = digest.to_string();
        Self(format!("sha256/{}/{}", &digest[..2], digest))
    }

    /// Parses only the exact canonical relative storage-key grammar.
    pub fn parse_canonical(value: &str) -> Result<Self, DomainValidationError> {
        if value.len() != 74
            || !value.starts_with("sha256/")
            || value.as_bytes().get(9) != Some(&b'/')
        {
            return Err(invalid_identifier());
        }
        let shard = &value[7..9];
        let digest = &value[10..];
        Sha256Digest::parse_canonical(digest)?;
        if shard != &digest[..2] {
            return Err(invalid_identifier());
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical logical key; this is never an absolute filesystem path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ArtifactStorageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ArtifactStorageKey")
            .field(&self.0)
            .finish()
    }
}
bounded_reference!(
    ArtifactMimeType,
    visible_ascii,
    255,
    "A bounded MIME-type reference."
);
bounded_reference!(
    ArtifactEncoding,
    visible_ascii,
    64,
    "A bounded artifact encoding reference."
);
bounded_reference!(
    ArtifactCompression,
    visible_ascii,
    64,
    "A bounded artifact compression reference."
);
bounded_reference!(
    ArtifactLogicalName,
    bounded_trimmed,
    255,
    "A bounded artifact logical name."
);
bounded_reference!(
    LinuxBootId,
    visible_ascii,
    64,
    "A bounded diagnostic Linux boot identifier."
);
bounded_reference!(
    PackageVersion,
    visible_ascii,
    128,
    "A bounded runtime package-version value."
);
bounded_reference!(
    GitRevision,
    visible_ascii,
    128,
    "A bounded runtime Git-revision value."
);

macro_rules! positive_reference_integer {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(i64);

        impl $name {
            /// Constructs a positive signed-64-bit-safe value.
            pub const fn try_new(value: i64) -> Result<Self, DomainValidationError> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(DomainValidationError::new(
                        DomainValidationKind::InvalidEvidenceReference,
                    ))
                }
            }

            /// Returns the exact positive numeric value.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = i64::deserialize(deserializer)?;
                Self::try_new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

positive_reference_integer!(
    TargetConfigurationVersion,
    "A positive model-target configuration version."
);
positive_reference_integer!(TokenCount, "A positive bounded token count.");
positive_reference_integer!(DiagnosticPid, "A positive diagnostic process identifier.");

/// Immutable model capability evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilitySnapshot {
    text_input: bool,
    text_output: bool,
    custom_tool_calling: bool,
    streaming: bool,
    ordered_output_items: bool,
    structured_output: bool,
    reasoning_continuation: bool,
    context_window_tokens: TokenCount,
    max_output_tokens: TokenCount,
}

/// Construction data for model capability evidence.
pub struct ModelCapabilitySnapshotInput {
    pub text_input: bool,
    pub text_output: bool,
    pub custom_tool_calling: bool,
    pub streaming: bool,
    pub ordered_output_items: bool,
    pub structured_output: bool,
    pub reasoning_continuation: bool,
    pub context_window_tokens: TokenCount,
    pub max_output_tokens: TokenCount,
}

impl ModelCapabilitySnapshot {
    /// Constructs an immutable snapshot from already bounded values.
    #[must_use]
    pub const fn new(input: ModelCapabilitySnapshotInput) -> Self {
        Self {
            text_input: input.text_input,
            text_output: input.text_output,
            custom_tool_calling: input.custom_tool_calling,
            streaming: input.streaming,
            ordered_output_items: input.ordered_output_items,
            structured_output: input.structured_output,
            reasoning_continuation: input.reasoning_continuation,
            context_window_tokens: input.context_window_tokens,
            max_output_tokens: input.max_output_tokens,
        }
    }

    pub const fn text_input(&self) -> bool {
        self.text_input
    }
    pub const fn text_output(&self) -> bool {
        self.text_output
    }
    pub const fn custom_tool_calling(&self) -> bool {
        self.custom_tool_calling
    }
    pub const fn streaming(&self) -> bool {
        self.streaming
    }
    pub const fn ordered_output_items(&self) -> bool {
        self.ordered_output_items
    }
    pub const fn structured_output(&self) -> bool {
        self.structured_output
    }
    pub const fn reasoning_continuation(&self) -> bool {
        self.reasoning_continuation
    }
    pub const fn context_window_tokens(&self) -> TokenCount {
        self.context_window_tokens
    }
    pub const fn max_output_tokens(&self) -> TokenCount {
        self.max_output_tokens
    }
}

/// Neutral configured-target and provider/model evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderModelReference {
    model_target_id: ModelTargetId,
    provider_id: ProviderId,
    provider_model_id: ProviderModelId,
    target_configuration_version: TargetConfigurationVersion,
    capabilities: ModelCapabilitySnapshot,
}

impl ProviderModelReference {
    /// Constructs neutral provider/model linkage with no credential or pricing data.
    #[must_use]
    pub const fn new(
        model_target_id: ModelTargetId,
        provider_id: ProviderId,
        provider_model_id: ProviderModelId,
        target_configuration_version: TargetConfigurationVersion,
        capabilities: ModelCapabilitySnapshot,
    ) -> Self {
        Self {
            model_target_id,
            provider_id,
            provider_model_id,
            target_configuration_version,
            capabilities,
        }
    }

    pub const fn model_target_id(&self) -> &ModelTargetId {
        &self.model_target_id
    }
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }
    pub const fn provider_model_id(&self) -> &ProviderModelId {
        &self.provider_model_id
    }
    pub const fn target_configuration_version(&self) -> TargetConfigurationVersion {
        self.target_configuration_version
    }
    pub const fn capabilities(&self) -> &ModelCapabilitySnapshot {
        &self.capabilities
    }
}

/// Exactly one specific producer identity, or none.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArtifactProducer {
    /// No model/tool attempt produced the artifact.
    None,
    /// One model attempt produced it.
    Model(ModelInvocationId),
    /// One tool attempt produced it.
    Tool(ToolExecutionId),
}

/// The only V0 artifact storage backend.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStorageBackend {
    /// The local content-addressed artifact store.
    Local,
}

/// Frozen artifact retention classes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRetention {
    /// Required to interpret canonical committed evidence.
    CanonicalEvidence,
    /// Useful diagnostic evidence not required for reconstruction.
    Diagnostic,
    /// Derived evidence that may be regenerated.
    Regenerable,
}

/// Construction data for immutable artifact metadata/reference.
pub struct ArtifactReferenceInput {
    pub artifact_id: ArtifactId,
    pub craxii_id: CraxiiId,
    pub producing_work_id: Option<WorkId>,
    pub producer: ArtifactProducer,
    pub storage_key: ArtifactStorageKey,
    pub sha256: Sha256Digest,
    pub canonical_length: CanonicalByteCount,
    pub observed_length: Option<CanonicalByteCount>,
    pub mime_type: ArtifactMimeType,
    pub encoding: Option<ArtifactEncoding>,
    pub logical_name: Option<ArtifactLogicalName>,
    pub retention: ArtifactRetention,
    pub truncated: bool,
    pub compression: Option<ArtifactCompression>,
    pub created_at: UtcTimestamp,
}

/// Immutable Stage 3.2 artifact metadata and provenance reference.
#[derive(Clone, Eq, PartialEq)]
pub struct ArtifactReference {
    artifact_id: ArtifactId,
    craxii_id: CraxiiId,
    producing_work_id: Option<WorkId>,
    producer: ArtifactProducer,
    storage_backend: ArtifactStorageBackend,
    storage_key: ArtifactStorageKey,
    sha256: Sha256Digest,
    canonical_length: CanonicalByteCount,
    observed_length: Option<CanonicalByteCount>,
    mime_type: ArtifactMimeType,
    encoding: Option<ArtifactEncoding>,
    logical_name: Option<ArtifactLogicalName>,
    retention: ArtifactRetention,
    truncated: bool,
    compression: Option<ArtifactCompression>,
    created_at: UtcTimestamp,
}

impl ArtifactReference {
    /// Constructs local immutable metadata; no storage access occurs.
    #[must_use]
    pub fn new(input: ArtifactReferenceInput) -> Self {
        Self {
            artifact_id: input.artifact_id,
            craxii_id: input.craxii_id,
            producing_work_id: input.producing_work_id,
            producer: input.producer,
            storage_backend: ArtifactStorageBackend::Local,
            storage_key: input.storage_key,
            sha256: input.sha256,
            canonical_length: input.canonical_length,
            observed_length: input.observed_length,
            mime_type: input.mime_type,
            encoding: input.encoding,
            logical_name: input.logical_name,
            retention: input.retention,
            truncated: input.truncated,
            compression: input.compression,
            created_at: input.created_at,
        }
    }

    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn producing_work_id(&self) -> Option<WorkId> {
        self.producing_work_id
    }
    pub const fn producer(&self) -> ArtifactProducer {
        self.producer
    }
    pub const fn storage_backend(&self) -> ArtifactStorageBackend {
        self.storage_backend
    }
    pub const fn storage_key(&self) -> &ArtifactStorageKey {
        &self.storage_key
    }
    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }
    pub const fn canonical_length(&self) -> CanonicalByteCount {
        self.canonical_length
    }
    pub const fn observed_length(&self) -> Option<CanonicalByteCount> {
        self.observed_length
    }
    pub const fn mime_type(&self) -> &ArtifactMimeType {
        &self.mime_type
    }
    pub const fn encoding(&self) -> Option<&ArtifactEncoding> {
        self.encoding.as_ref()
    }
    pub const fn logical_name(&self) -> Option<&ArtifactLogicalName> {
        self.logical_name.as_ref()
    }
    pub const fn retention(&self) -> ArtifactRetention {
        self.retention
    }
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
    pub const fn compression(&self) -> Option<&ArtifactCompression> {
        self.compression.as_ref()
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

impl fmt::Debug for ArtifactReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactReference")
            .field("artifact_id", &self.artifact_id)
            .field("craxii_id", &self.craxii_id)
            .field("producing_work_id", &self.producing_work_id)
            .field("producer", &self.producer)
            .field("storage_backend", &self.storage_backend)
            .field("storage_key", &"[REDACTED]")
            .field("sha256", &self.sha256)
            .field("canonical_length", &self.canonical_length)
            .field("observed_length", &self.observed_length)
            .field("retention", &self.retention)
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

/// Frozen authority outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityDecision {
    /// The request is allowed.
    Allow,
    /// The request is denied.
    Deny,
}

/// Frozen effective privilege modes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeMode {
    /// Ordinary workstation-user privilege.
    User,
    /// Explicit administrative privilege.
    Administrative,
}

/// The exact Stage 3.2 V0 authority policy version.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AuthorityPolicyVersion;

impl AuthorityPolicyVersion {
    pub const V0_DEVELOPMENT_WORKSTATION: Self = Self;
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        "v0-development-workstation"
    }
}

/// Immutable authority-decision evidence, not an evaluator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityDecisionSnapshot {
    decision: AuthorityDecision,
    effective_privilege: PrivilegeMode,
    policy_version: AuthorityPolicyVersion,
    reason_code: AuthorityReasonCode,
}

impl AuthorityDecisionSnapshot {
    /// Constructs evidence using the exact V0 policy version.
    #[must_use]
    pub const fn new(
        decision: AuthorityDecision,
        effective_privilege: PrivilegeMode,
        reason_code: AuthorityReasonCode,
    ) -> Self {
        Self {
            decision,
            effective_privilege,
            policy_version: AuthorityPolicyVersion::V0_DEVELOPMENT_WORKSTATION,
            reason_code,
        }
    }

    pub const fn decision(&self) -> AuthorityDecision {
        self.decision
    }
    pub const fn effective_privilege(&self) -> PrivilegeMode {
        self.effective_privilege
    }
    pub const fn policy_version(&self) -> AuthorityPolicyVersion {
        self.policy_version
    }
    pub const fn reason_code(&self) -> &AuthorityReasonCode {
        &self.reason_code
    }
}

/// Construction data for immutable model-attempt linkage.
pub struct ModelAttemptReferenceInput {
    pub logical_invocation_id: LogicalInvocationId,
    pub model_invocation_id: ModelInvocationId,
    pub work_id: WorkId,
    pub runtime_instance_id: RuntimeInstanceId,
    pub context_manifest_id: ContextManifestId,
    pub agent_step_no: AgentStepNo,
    pub attempt_no: AttemptNo,
    pub provider_model: ProviderModelReference,
    pub retry_of: Option<ModelInvocationId>,
}

/// Immutable model-attempt identity/linkage without lifecycle or outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelAttemptReference {
    logical_invocation_id: LogicalInvocationId,
    model_invocation_id: ModelInvocationId,
    work_id: WorkId,
    runtime_instance_id: RuntimeInstanceId,
    context_manifest_id: ContextManifestId,
    agent_step_no: AgentStepNo,
    attempt_no: AttemptNo,
    provider_model: ProviderModelReference,
    retry_of: Option<ModelInvocationId>,
}

impl ModelAttemptReference {
    #[must_use]
    pub fn new(input: ModelAttemptReferenceInput) -> Self {
        Self {
            logical_invocation_id: input.logical_invocation_id,
            model_invocation_id: input.model_invocation_id,
            work_id: input.work_id,
            runtime_instance_id: input.runtime_instance_id,
            context_manifest_id: input.context_manifest_id,
            agent_step_no: input.agent_step_no,
            attempt_no: input.attempt_no,
            provider_model: input.provider_model,
            retry_of: input.retry_of,
        }
    }

    pub const fn logical_invocation_id(&self) -> LogicalInvocationId {
        self.logical_invocation_id
    }
    pub const fn model_invocation_id(&self) -> ModelInvocationId {
        self.model_invocation_id
    }
    pub const fn work_id(&self) -> WorkId {
        self.work_id
    }
    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }
    pub const fn context_manifest_id(&self) -> ContextManifestId {
        self.context_manifest_id
    }
    pub const fn agent_step_no(&self) -> AgentStepNo {
        self.agent_step_no
    }
    pub const fn attempt_no(&self) -> AttemptNo {
        self.attempt_no
    }
    pub const fn provider_model(&self) -> &ProviderModelReference {
        &self.provider_model
    }
    pub const fn retry_of(&self) -> Option<ModelInvocationId> {
        self.retry_of
    }
}

/// Construction data for immutable tool-attempt linkage.
pub struct ToolAttemptReferenceInput {
    pub tool_execution_id: ToolExecutionId,
    pub execution_id: ExecutionId,
    pub work_id: WorkId,
    pub runtime_instance_id: RuntimeInstanceId,
    pub source_model_invocation_id: ModelInvocationId,
    pub agent_step_no: AgentStepNo,
    pub tool_ordinal: ToolOrdinal,
    pub tool_name: ToolName,
    pub tool_version: ToolVersion,
    pub schema_version: SchemaVersion,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    pub requested_path: Option<LogicalPathReference>,
    pub authority: AuthorityDecisionSnapshot,
}

/// Immutable tool-attempt identity/linkage without dispatch state or outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolAttemptReference {
    tool_execution_id: ToolExecutionId,
    execution_id: ExecutionId,
    work_id: WorkId,
    runtime_instance_id: RuntimeInstanceId,
    source_model_invocation_id: ModelInvocationId,
    agent_step_no: AgentStepNo,
    tool_ordinal: ToolOrdinal,
    tool_name: ToolName,
    tool_version: ToolVersion,
    schema_version: SchemaVersion,
    workstation_id: WorkstationId,
    workstation_generation: WorkstationGeneration,
    workspace_id: WorkspaceId,
    requested_path: Option<LogicalPathReference>,
    authority: AuthorityDecisionSnapshot,
}

impl ToolAttemptReference {
    #[must_use]
    pub fn new(input: ToolAttemptReferenceInput) -> Self {
        Self {
            tool_execution_id: input.tool_execution_id,
            execution_id: input.execution_id,
            work_id: input.work_id,
            runtime_instance_id: input.runtime_instance_id,
            source_model_invocation_id: input.source_model_invocation_id,
            agent_step_no: input.agent_step_no,
            tool_ordinal: input.tool_ordinal,
            tool_name: input.tool_name,
            tool_version: input.tool_version,
            schema_version: input.schema_version,
            workstation_id: input.workstation_id,
            workstation_generation: input.workstation_generation,
            workspace_id: input.workspace_id,
            requested_path: input.requested_path,
            authority: input.authority,
        }
    }

    pub const fn tool_execution_id(&self) -> ToolExecutionId {
        self.tool_execution_id
    }
    pub const fn execution_id(&self) -> ExecutionId {
        self.execution_id
    }
    pub const fn work_id(&self) -> WorkId {
        self.work_id
    }
    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }
    pub const fn source_model_invocation_id(&self) -> ModelInvocationId {
        self.source_model_invocation_id
    }
    pub const fn agent_step_no(&self) -> AgentStepNo {
        self.agent_step_no
    }
    pub const fn tool_ordinal(&self) -> ToolOrdinal {
        self.tool_ordinal
    }
    pub const fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }
    pub const fn tool_version(&self) -> &ToolVersion {
        &self.tool_version
    }
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }
    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub const fn workstation_generation(&self) -> WorkstationGeneration {
        self.workstation_generation
    }
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub const fn requested_path(&self) -> Option<&LogicalPathReference> {
        self.requested_path.as_ref()
    }
    pub const fn authority(&self) -> &AuthorityDecisionSnapshot {
        &self.authority
    }
}

/// Adapter-observed physical path evidence separated from logical identity.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedPathEvidence {
    workstation_id: WorkstationId,
    workstation_generation: WorkstationGeneration,
    workspace_id: WorkspaceId,
    requested_path: LogicalPathReference,
    resolved_absolute_path: String,
}

impl ResolvedPathEvidence {
    /// Validates only syntactic absolute/bounded physical evidence.
    pub fn try_new(
        workstation_id: WorkstationId,
        workstation_generation: WorkstationGeneration,
        workspace_id: WorkspaceId,
        requested_path: LogicalPathReference,
        resolved_absolute_path: impl Into<String>,
    ) -> Result<Self, DomainValidationError> {
        let resolved_absolute_path = resolved_absolute_path.into();
        if !resolved_absolute_path.starts_with('/')
            || resolved_absolute_path.contains('\0')
            || resolved_absolute_path.len() > MAX_LOGICAL_PATH_BYTES
        {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidEvidenceReference,
            ));
        }
        Ok(Self {
            workstation_id,
            workstation_generation,
            workspace_id,
            requested_path,
            resolved_absolute_path,
        })
    }

    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub const fn workstation_generation(&self) -> WorkstationGeneration {
        self.workstation_generation
    }
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub const fn requested_path(&self) -> &LogicalPathReference {
        &self.requested_path
    }
    pub fn resolved_absolute_path(&self) -> &str {
        &self.resolved_absolute_path
    }
}

impl fmt::Debug for ResolvedPathEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedPathEvidence")
            .field("workstation_id", &self.workstation_id)
            .field("workstation_generation", &self.workstation_generation)
            .field("workspace_id", &self.workspace_id)
            .field("requested_path", &self.requested_path)
            .field("resolved_absolute_path", &"[REDACTED]")
            .finish()
    }
}

/// Construction data for immutable runtime-start evidence.
pub struct RuntimeStartEvidenceInput {
    pub runtime_instance_id: RuntimeInstanceId,
    pub craxii_id: CraxiiId,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub linux_boot_id: Option<LinuxBootId>,
    pub diagnostic_pid: Option<DiagnosticPid>,
    pub package_version: PackageVersion,
    pub git_revision: GitRevision,
    pub schema_version: SchemaVersion,
    pub started_at: UtcTimestamp,
}

/// Immutable runtime-start linkage with diagnostic boot/PID evidence.
#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeStartEvidence {
    runtime_instance_id: RuntimeInstanceId,
    craxii_id: CraxiiId,
    workstation_id: WorkstationId,
    workstation_generation: WorkstationGeneration,
    linux_boot_id: Option<LinuxBootId>,
    diagnostic_pid: Option<DiagnosticPid>,
    package_version: PackageVersion,
    git_revision: GitRevision,
    schema_version: SchemaVersion,
    started_at: UtcTimestamp,
}

impl RuntimeStartEvidence {
    #[must_use]
    pub fn new(input: RuntimeStartEvidenceInput) -> Self {
        Self {
            runtime_instance_id: input.runtime_instance_id,
            craxii_id: input.craxii_id,
            workstation_id: input.workstation_id,
            workstation_generation: input.workstation_generation,
            linux_boot_id: input.linux_boot_id,
            diagnostic_pid: input.diagnostic_pid,
            package_version: input.package_version,
            git_revision: input.git_revision,
            schema_version: input.schema_version,
            started_at: input.started_at,
        }
    }

    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub const fn workstation_generation(&self) -> WorkstationGeneration {
        self.workstation_generation
    }
    pub const fn linux_boot_id(&self) -> Option<&LinuxBootId> {
        self.linux_boot_id.as_ref()
    }
    pub const fn diagnostic_pid(&self) -> Option<DiagnosticPid> {
        self.diagnostic_pid
    }
    pub const fn package_version(&self) -> &PackageVersion {
        &self.package_version
    }
    pub const fn git_revision(&self) -> &GitRevision {
        &self.git_revision
    }
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }
    pub const fn started_at(&self) -> UtcTimestamp {
        self.started_at
    }
}

impl fmt::Debug for RuntimeStartEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeStartEvidence")
            .field("runtime_instance_id", &self.runtime_instance_id)
            .field("craxii_id", &self.craxii_id)
            .field("workstation_id", &self.workstation_id)
            .field("workstation_generation", &self.workstation_generation)
            .field("package_version", &self.package_version)
            .field("git_revision", &self.git_revision)
            .field("schema_version", &self.schema_version)
            .field("started_at", &self.started_at)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    fn id<T: std::str::FromStr>(value: &str) -> T
    where
        T::Err: fmt::Debug,
    {
        value.parse().unwrap()
    }

    fn now() -> UtcTimestamp {
        "2026-08-27T12:34:56.000001Z".parse().unwrap()
    }

    #[test]
    fn bounded_provider_model_tool_and_reason_grammars_are_exact() {
        for valid in ["a", "0", "openai", "openai.responses_v1", "a-b_c.d9"] {
            assert_eq!(ProviderId::try_new(valid).unwrap().as_str(), valid);
            assert_eq!(ModelTargetId::try_new(valid).unwrap().as_str(), valid);
            assert_eq!(ToolName::try_new(valid).unwrap().as_str(), valid);
        }
        for invalid in ["", "Upper", "-leading", "a/b", "a b", &"a".repeat(65)] {
            assert!(
                ProviderId::try_new(invalid).is_err(),
                "provider accepted {invalid:?}"
            );
            assert!(
                ModelTargetId::try_new(invalid).is_err(),
                "target accepted {invalid:?}"
            );
            assert!(
                ToolName::try_new(invalid).is_err(),
                "tool accepted {invalid:?}"
            );
        }

        assert_eq!(
            ProviderModelId::try_new("gpt-5.6/日本").unwrap().as_str(),
            "gpt-5.6/日本"
        );
        for invalid in ["", " leading", "trailing ", "bad\u{7f}"] {
            assert!(ProviderModelId::try_new(invalid).is_err());
        }
        assert!(ToolVersion::try_new("1.0.0+dev").is_ok());
        assert!(ToolVersion::try_new("1.0 dev").is_err());
        assert!(AuthorityReasonCode::try_new("policy_allow_1").is_ok());
        for invalid in ["1starts_numeric", "has-hyphen", "Upper", ""] {
            assert!(AuthorityReasonCode::try_new(invalid).is_err());
        }

        assert!(ProviderModelId::try_new("m".repeat(128)).is_ok());
        assert!(ProviderModelId::try_new("m".repeat(129)).is_err());
        assert!(ToolVersion::try_new("v".repeat(64)).is_ok());
        assert!(ToolVersion::try_new("v".repeat(65)).is_err());
        assert!(AuthorityReasonCode::try_new(format!("a{}", "1".repeat(63))).is_ok());
        assert!(AuthorityReasonCode::try_new(format!("a{}", "1".repeat(64))).is_err());
    }

    #[test]
    fn artifact_storage_key_and_closed_literals_are_exact() {
        let digest = Sha256Digest::hash_bytes(b"artifact");
        let storage_key = ArtifactStorageKey::from_digest(digest);
        assert_eq!(
            storage_key.as_str(),
            "sha256/c7/c7c5c1d70c5dec4416ab6158afd0b223ef40c29b1dc1f97ed9428b94d4cadb1c"
        );
        assert_eq!(
            ArtifactStorageKey::parse_canonical(storage_key.as_str()).unwrap(),
            storage_key
        );
        for invalid in ["", "sha256/AB/00", "../sha256/00"] {
            assert!(ArtifactStorageKey::parse_canonical(invalid).is_err());
        }

        let literals = [
            (
                serde_json::to_string(&ArtifactStorageBackend::Local).unwrap(),
                "\"local\"",
            ),
            (
                serde_json::to_string(&ArtifactRetention::CanonicalEvidence).unwrap(),
                "\"canonical_evidence\"",
            ),
            (
                serde_json::to_string(&ArtifactRetention::Diagnostic).unwrap(),
                "\"diagnostic\"",
            ),
            (
                serde_json::to_string(&ArtifactRetention::Regenerable).unwrap(),
                "\"regenerable\"",
            ),
            (
                serde_json::to_string(&AuthorityDecision::Allow).unwrap(),
                "\"allow\"",
            ),
            (
                serde_json::to_string(&AuthorityDecision::Deny).unwrap(),
                "\"deny\"",
            ),
            (
                serde_json::to_string(&PrivilegeMode::User).unwrap(),
                "\"user\"",
            ),
            (
                serde_json::to_string(&PrivilegeMode::Administrative).unwrap(),
                "\"administrative\"",
            ),
        ];
        for (actual, expected) in literals {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn artifact_producer_is_exclusive_and_work_provenance_can_coexist() {
        let model_id = id(V7);
        let producer = ArtifactProducer::Model(model_id);
        assert!(matches!(producer, ArtifactProducer::Model(value) if value == model_id));
        let reference = ArtifactReference::new(ArtifactReferenceInput {
            artifact_id: id(V7),
            craxii_id: id(V7),
            producing_work_id: Some(id(V7)),
            producer,
            storage_key: ArtifactStorageKey::from_digest(Sha256Digest::hash_bytes(b"artifact")),
            sha256: Sha256Digest::hash_bytes(b"artifact"),
            canonical_length: CanonicalByteCount::try_new(8).unwrap(),
            observed_length: Some(CanonicalByteCount::try_new(9).unwrap()),
            mime_type: ArtifactMimeType::try_new("text/plain").unwrap(),
            encoding: Some(ArtifactEncoding::try_new("utf-8").unwrap()),
            logical_name: Some(ArtifactLogicalName::try_new("test evidence").unwrap()),
            retention: ArtifactRetention::CanonicalEvidence,
            truncated: true,
            compression: None,
            created_at: now(),
        });
        assert!(reference.producing_work_id().is_some());
        assert!(matches!(reference.producer(), ArtifactProducer::Model(_)));
        assert_ne!(
            reference.artifact_id().to_string(),
            reference.sha256().to_string()
        );
        assert_eq!(reference.storage_backend(), ArtifactStorageBackend::Local);
        assert!(!format!("{reference:?}").contains("opaque/key-not-client-path"));
    }

    #[test]
    fn authority_snapshot_has_exact_policy_and_no_payload_surface() {
        let authority = AuthorityDecisionSnapshot::new(
            AuthorityDecision::Allow,
            PrivilegeMode::Administrative,
            AuthorityReasonCode::try_new("registered_tool").unwrap(),
        );
        assert_eq!(
            authority.policy_version().as_str(),
            "v0-development-workstation"
        );
        assert_eq!(authority.decision(), AuthorityDecision::Allow);
        assert_eq!(
            authority.effective_privilege(),
            PrivilegeMode::Administrative
        );
        let debug = format!("{authority:?}");
        assert!(!debug.contains("token"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn model_capability_and_provider_reference_are_neutral_and_bounded() {
        assert!(TokenCount::try_new(0).is_err());
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(128_000).unwrap(),
            max_output_tokens: TokenCount::try_new(16_384).unwrap(),
        });
        let reference = ProviderModelReference::new(
            ModelTargetId::try_new("primary").unwrap(),
            ProviderId::try_new("openai").unwrap(),
            ProviderModelId::try_new("gpt-5.6").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities,
        );
        assert_eq!(reference.provider_id().as_str(), "openai");
        assert!(reference.capabilities().ordered_output_items());
        assert_eq!(
            reference.capabilities().context_window_tokens().get(),
            128_000
        );
        assert_eq!(
            serde_json::to_string(&reference.target_configuration_version()).unwrap(),
            "1"
        );
    }

    #[test]
    fn model_and_tool_attempt_references_preserve_distinct_linkage_types() {
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(128_000).unwrap(),
            max_output_tokens: TokenCount::try_new(16_384).unwrap(),
        });
        let provider_model = ProviderModelReference::new(
            ModelTargetId::try_new("primary").unwrap(),
            ProviderId::try_new("openai").unwrap(),
            ProviderModelId::try_new("gpt-5.6").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities,
        );
        let work_id = id(V7);
        let runtime_instance_id = RuntimeInstanceId::generate();
        let model_invocation_id = ModelInvocationId::generate();
        let model = ModelAttemptReference::new(ModelAttemptReferenceInput {
            logical_invocation_id: id(V7),
            model_invocation_id,
            work_id,
            runtime_instance_id,
            context_manifest_id: id(V7),
            agent_step_no: AgentStepNo::try_new(1).unwrap(),
            attempt_no: AttemptNo::try_new(1).unwrap(),
            provider_model,
            retry_of: None,
        });
        assert_eq!(model.model_invocation_id(), model_invocation_id);
        assert_eq!(model.runtime_instance_id(), runtime_instance_id);

        let tool_execution_id = ToolExecutionId::generate();
        let execution_id = ExecutionId::generate();
        let authority = AuthorityDecisionSnapshot::new(
            AuthorityDecision::Allow,
            PrivilegeMode::User,
            AuthorityReasonCode::try_new("registered_tool").unwrap(),
        );
        let tool = ToolAttemptReference::new(ToolAttemptReferenceInput {
            tool_execution_id,
            execution_id,
            work_id,
            runtime_instance_id,
            source_model_invocation_id: model_invocation_id,
            agent_step_no: AgentStepNo::try_new(1).unwrap(),
            tool_ordinal: ToolOrdinal::try_new(1).unwrap(),
            tool_name: ToolName::try_new("read_file").unwrap(),
            tool_version: ToolVersion::try_new("1.0.0").unwrap(),
            schema_version: SchemaVersion::try_new(1).unwrap(),
            workstation_id: id(V7),
            workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
            workspace_id: id(V7),
            requested_path: Some(LogicalPathReference::workspace_relative("src/lib.rs").unwrap()),
            authority,
        });
        assert_eq!(tool.tool_execution_id(), tool_execution_id);
        assert_eq!(tool.execution_id(), execution_id);
        assert_eq!(tool.source_model_invocation_id(), model_invocation_id);
        assert_eq!(tool.runtime_instance_id(), runtime_instance_id);
        assert_eq!(tool.requested_path().unwrap().canonical(), "src/lib.rs");
    }

    #[test]
    fn resolved_path_is_distinct_evidence_and_debug_redacts_physical_value() {
        let requested = LogicalPathReference::workspace_relative("src/lib.rs").unwrap();
        let sentinel = "/srv/craxii/workspaces/private/src/lib.rs";
        let evidence = ResolvedPathEvidence::try_new(
            id(V7),
            WorkstationGeneration::try_new(1).unwrap(),
            id(V7),
            requested.clone(),
            sentinel,
        )
        .unwrap();
        assert_eq!(evidence.requested_path(), &requested);
        assert_eq!(evidence.resolved_absolute_path(), sentinel);
        let debug = format!("{evidence:?}");
        assert!(!debug.contains(sentinel));
        assert!(debug.contains("[REDACTED]"));
        let exact = format!("/{}", "a".repeat(MAX_LOGICAL_PATH_BYTES - 1));
        assert_eq!(exact.len(), MAX_LOGICAL_PATH_BYTES);
        assert!(
            ResolvedPathEvidence::try_new(
                id(V7),
                WorkstationGeneration::try_new(1).unwrap(),
                id(V7),
                requested.clone(),
                exact
            )
            .is_ok()
        );
        assert!(
            ResolvedPathEvidence::try_new(
                id(V7),
                WorkstationGeneration::try_new(1).unwrap(),
                id(V7),
                requested.clone(),
                "relative"
            )
            .is_err()
        );
        assert!(
            ResolvedPathEvidence::try_new(
                id(V7),
                WorkstationGeneration::try_new(1).unwrap(),
                id(V7),
                requested.clone(),
                format!("/{}", "a".repeat(4_096))
            )
            .is_err()
        );
        assert!(
            ResolvedPathEvidence::try_new(
                id(V7),
                WorkstationGeneration::try_new(1).unwrap(),
                id(V7),
                requested,
                "/bad/\0path"
            )
            .is_err()
        );
    }

    #[test]
    fn runtime_identity_is_separate_and_pid_boot_id_are_diagnostic() {
        let runtime_id = id(V7);
        let evidence = RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: runtime_id,
            craxii_id: id(V7),
            workstation_id: id(V7),
            workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
            linux_boot_id: Some(LinuxBootId::try_new("boot-id-123").unwrap()),
            diagnostic_pid: Some(DiagnosticPid::try_new(42).unwrap()),
            package_version: PackageVersion::try_new("0.0.1").unwrap(),
            git_revision: GitRevision::try_new("4d29bf4").unwrap(),
            schema_version: SchemaVersion::try_new(1).unwrap(),
            started_at: now(),
        });
        assert_eq!(evidence.runtime_instance_id(), runtime_id);
        assert_eq!(evidence.diagnostic_pid().unwrap().get(), 42);
        assert_eq!(evidence.linux_boot_id().unwrap().as_str(), "boot-id-123");
        assert!(!format!("{evidence:?}").contains("boot-id-123"));
    }
}
