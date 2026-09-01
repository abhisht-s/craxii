//! Provider-neutral Stage 15 model targets, requests, ordered output, and stream contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};

use serde_json::{Map, Value, json};

use super::{
    ContextManifestId, LogicalInvocationId, ModelCapabilitySnapshot, ModelTargetId, ProviderId,
    ProviderModelId, ProviderModelReference, SchemaVersion, Sha256Digest,
    TargetConfigurationVersion, TokenCount, ToolName, ToolVersion,
};

/// Hard V0 limit for ordered normalized provider output.
pub const MAX_MODEL_OUTPUT_ITEMS: usize = 64;
/// Hard V0 limit for one complete raw tool-call argument string.
pub const MAX_MODEL_TOOL_ARGUMENT_BYTES: usize = 65_536;
/// Hard V0 limit for the compact canonical normalized response envelope.
pub const MAX_NORMALIZED_MODEL_RESPONSE_BYTES: usize = 262_144;
/// Bound for one provider-neutral text/opaque/structured-data component.
pub const MAX_MODEL_COMPONENT_BYTES: usize = 65_536;
/// Bound for provider request/response/call identifiers.
pub const MAX_PROVIDER_EVIDENCE_ID_BYTES: usize = 128;
const MAX_ENDPOINT_REFERENCE_BYTES: usize = 2_048;
const MAX_CONFIG_REFERENCE_BYTES: usize = 128;
const MAX_METADATA_ENTRIES: usize = 32;

/// Stable redacted model-contract validation categories.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModelContractErrorKind {
    InvalidTarget,
    InvalidRequest,
    InvalidText,
    InvalidStructuredData,
    InvalidToolName,
    InvalidToolArguments,
    ToolArgumentsTooLarge,
    TooManyOutputItems,
    DuplicateToolCallId,
    InvalidUsage,
    InvalidProviderEvidence,
    UnknownSemanticItem,
    InvalidTerminalSemantics,
    InvalidStreamOrdering,
    NormalizedOutputTooLarge,
}

impl ModelContractErrorKind {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidTarget => "invalid_model_target",
            Self::InvalidRequest => "invalid_model_request",
            Self::InvalidText => "invalid_model_text",
            Self::InvalidStructuredData => "invalid_structured_data",
            Self::InvalidToolName => "invalid_tool_name",
            Self::InvalidToolArguments => "malformed_tool_arguments",
            Self::ToolArgumentsTooLarge => "tool_arguments_too_large",
            Self::TooManyOutputItems => "too_many_output_items",
            Self::DuplicateToolCallId => "duplicate_tool_call_id",
            Self::InvalidUsage => "invalid_model_usage",
            Self::InvalidProviderEvidence => "invalid_provider_evidence",
            Self::UnknownSemanticItem => "unsupported_provider_item",
            Self::InvalidTerminalSemantics => "invalid_model_terminal_semantics",
            Self::InvalidStreamOrdering => "invalid_provider_stream_order",
            Self::NormalizedOutputTooLarge => "normalized_output_too_large",
        }
    }
}

/// Redacted contract failure. Rejected content and provider data are never retained.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ModelContractError(ModelContractErrorKind);

impl ModelContractError {
    #[must_use]
    pub const fn new(kind: ModelContractErrorKind) -> Self {
        Self(kind)
    }

    #[must_use]
    pub const fn kind(self) -> ModelContractErrorKind {
        self.0
    }
}

impl Display for ModelContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.code())
    }
}

impl fmt::Debug for ModelContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelContractError")
            .field("kind", &self.0)
            .finish()
    }
}

impl std::error::Error for ModelContractError {}

/// Validated safe configuration references carried by an immutable target snapshot.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModelConfigReference(String);

impl ModelConfigReference {
    pub fn endpoint(value: impl Into<String>) -> Result<Self, ModelContractError> {
        Self::try_new(value.into(), MAX_ENDPOINT_REFERENCE_BYTES)
    }

    pub fn named(value: impl Into<String>) -> Result<Self, ModelContractError> {
        Self::try_new(value.into(), MAX_CONFIG_REFERENCE_BYTES)
    }

    fn try_new(value: String, maximum: usize) -> Result<Self, ModelContractError> {
        if value.is_empty()
            || value.len() > maximum
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidTarget,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ModelConfigReference {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ModelConfigReference")
            .field(&self.0)
            .finish()
    }
}

/// Stable estimator identity and semantic version from validated target configuration.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TokenEstimatorIdentity {
    id: ModelConfigReference,
    version: u64,
}

impl TokenEstimatorIdentity {
    pub fn try_new(id: impl Into<String>, version: u64) -> Result<Self, ModelContractError> {
        if version == 0 || version > i64::MAX as u64 {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidTarget,
            ));
        }
        Ok(Self {
            id: ModelConfigReference::named(id)?,
            version,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

/// Closed provider-native option set already validated by startup configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderNativeOptions {
    reasoning_continuation: bool,
}

impl ProviderNativeOptions {
    #[must_use]
    pub const fn new(reasoning_continuation: bool) -> Self {
        Self {
            reasoning_continuation,
        }
    }

    #[must_use]
    pub const fn reasoning_continuation(self) -> bool {
        self.reasoning_continuation
    }
}

/// Immutable configured model-target snapshot used for selection and invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTarget {
    reference: ProviderModelReference,
    enabled: bool,
    endpoint_reference: ModelConfigReference,
    account_reference: ModelConfigReference,
    requested_output_tokens: TokenCount,
    estimator: TokenEstimatorIdentity,
    provider_native_options: ProviderNativeOptions,
}

pub struct ModelTargetInput {
    pub reference: ProviderModelReference,
    pub enabled: bool,
    pub endpoint_reference: ModelConfigReference,
    pub account_reference: ModelConfigReference,
    pub requested_output_tokens: TokenCount,
    pub estimator: TokenEstimatorIdentity,
    pub provider_native_options: ProviderNativeOptions,
}

impl ModelTarget {
    pub fn try_new(input: ModelTargetInput) -> Result<Self, ModelContractError> {
        if input.requested_output_tokens > input.reference.capabilities().max_output_tokens()
            || input.provider_native_options.reasoning_continuation()
                && !input.reference.capabilities().reasoning_continuation()
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidTarget,
            ));
        }
        Ok(Self {
            reference: input.reference,
            enabled: input.enabled,
            endpoint_reference: input.endpoint_reference,
            account_reference: input.account_reference,
            requested_output_tokens: input.requested_output_tokens,
            estimator: input.estimator,
            provider_native_options: input.provider_native_options,
        })
    }

    #[must_use]
    pub const fn reference(&self) -> &ProviderModelReference {
        &self.reference
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn endpoint_reference(&self) -> &ModelConfigReference {
        &self.endpoint_reference
    }

    #[must_use]
    pub const fn account_reference(&self) -> &ModelConfigReference {
        &self.account_reference
    }

    #[must_use]
    pub const fn requested_output_tokens(&self) -> TokenCount {
        self.requested_output_tokens
    }

    #[must_use]
    pub const fn estimator(&self) -> &TokenEstimatorIdentity {
        &self.estimator
    }

    #[must_use]
    pub const fn provider_native_options(&self) -> ProviderNativeOptions {
        self.provider_native_options
    }

    #[must_use]
    pub fn identity(&self) -> ModelTargetIdentity {
        ModelTargetIdentity::from_reference(&self.reference)
    }
}

/// Provider-neutral selected target identity included in canonical requests and responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTargetIdentity {
    model_target_id: ModelTargetId,
    provider_id: ProviderId,
    provider_model_id: ProviderModelId,
    target_configuration_version: TargetConfigurationVersion,
}

impl ModelTargetIdentity {
    #[must_use]
    pub fn from_reference(reference: &ProviderModelReference) -> Self {
        Self {
            model_target_id: reference.model_target_id().clone(),
            provider_id: reference.provider_id().clone(),
            provider_model_id: reference.provider_model_id().clone(),
            target_configuration_version: reference.target_configuration_version(),
        }
    }

    #[must_use]
    pub const fn model_target_id(&self) -> &ModelTargetId {
        &self.model_target_id
    }

    #[must_use]
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    #[must_use]
    pub const fn provider_model_id(&self) -> &ProviderModelId {
        &self.provider_model_id
    }

    #[must_use]
    pub const fn target_configuration_version(&self) -> TargetConfigurationVersion {
        self.target_configuration_version
    }

    fn semantic_value(&self) -> Value {
        json!({
            "model_target_id": self.model_target_id.as_str(),
            "provider_id": self.provider_id.as_str(),
            "provider_model_id": self.provider_model_id.as_str(),
            "target_configuration_version": self.target_configuration_version.get(),
        })
    }
}

/// Required model abilities derived before Stage 16 context rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequiredModelCapabilities {
    pub text_input: bool,
    pub text_output: bool,
    pub custom_tool_calling: bool,
    pub streaming: bool,
    pub ordered_output_items: bool,
    pub structured_output: bool,
    pub reasoning_continuation: bool,
    pub required_output_tokens: TokenCount,
}

impl RequiredModelCapabilities {
    #[must_use]
    pub const fn satisfied_by(self, available: &ModelCapabilitySnapshot) -> bool {
        (!self.text_input || available.text_input())
            && (!self.text_output || available.text_output())
            && (!self.custom_tool_calling || available.custom_tool_calling())
            && (!self.streaming || available.streaming())
            && (!self.ordered_output_items || available.ordered_output_items())
            && (!self.structured_output || available.structured_output())
            && (!self.reasoning_continuation || available.reasoning_continuation())
            && self.required_output_tokens.get() <= available.max_output_tokens().get()
    }

    #[must_use]
    pub fn canonical_bytes(self) -> Vec<u8> {
        canonical_json_bytes(&self.semantic_value())
    }

    #[must_use]
    pub fn canonical_sha256(self) -> Sha256Digest {
        Sha256Digest::hash_bytes(&self.canonical_bytes())
    }

    fn semantic_value(self) -> Value {
        json!({
            "custom_tool_calling": self.custom_tool_calling,
            "ordered_output_items": self.ordered_output_items,
            "reasoning_continuation": self.reasoning_continuation,
            "required_output_tokens": self.required_output_tokens.get(),
            "streaming": self.streaming,
            "structured_output": self.structured_output,
            "text_input": self.text_input,
            "text_output": self.text_output,
        })
    }
}

/// One nonempty bounded text part whose debug form never exposes content.
#[derive(Clone, Eq, PartialEq)]
pub struct ModelTextPart(String);

impl ModelTextPart {
    pub fn try_new(value: impl Into<String>) -> Result<Self, ModelContractError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_MODEL_COMPONENT_BYTES {
            return Err(ModelContractError::new(ModelContractErrorKind::InvalidText));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ModelTextPart {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelTextPart")
            .field("utf8_bytes", &self.0.len())
            .field("text", &"[REDACTED]")
            .finish()
    }
}

/// Bounded provider/canonical tool-call identity preserved exactly.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelToolCallId(String);

impl ModelToolCallId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, ModelContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROVIDER_EVIDENCE_ID_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidProviderEvidence,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ModelToolCallId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ModelToolCallId")
            .field(&self.0)
            .finish()
    }
}

/// Complete canonical tool call. Raw arguments are always retained exactly.
#[derive(Clone, Eq, PartialEq)]
pub struct CanonicalModelToolCall {
    call_id: ModelToolCallId,
    name: ToolName,
    raw_arguments: String,
    parsed_arguments: Option<Value>,
}

impl CanonicalModelToolCall {
    pub fn try_new(
        call_id: ModelToolCallId,
        name: impl Into<String>,
        raw_arguments: impl Into<String>,
    ) -> Result<Self, ModelContractError> {
        let name = ToolName::try_new(name.into())
            .map_err(|_| ModelContractError::new(ModelContractErrorKind::InvalidToolName))?;
        let raw_arguments = raw_arguments.into();
        if raw_arguments.len() > MAX_MODEL_TOOL_ARGUMENT_BYTES {
            return Err(ModelContractError::new(
                ModelContractErrorKind::ToolArgumentsTooLarge,
            ));
        }
        let parsed_arguments = serde_json::from_str(&raw_arguments)
            .ok()
            .map(canonicalize_json);
        Ok(Self {
            call_id,
            name,
            raw_arguments,
            parsed_arguments,
        })
    }

    #[must_use]
    pub const fn call_id(&self) -> &ModelToolCallId {
        &self.call_id
    }

    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    #[must_use]
    pub fn raw_arguments(&self) -> &str {
        &self.raw_arguments
    }

    #[must_use]
    pub const fn parsed_arguments(&self) -> Option<&Value> {
        self.parsed_arguments.as_ref()
    }

    #[must_use]
    pub const fn arguments_are_valid_json(&self) -> bool {
        self.parsed_arguments.is_some()
    }

    pub fn require_valid_arguments(&self) -> Result<&Value, ModelContractError> {
        self.parsed_arguments
            .as_ref()
            .ok_or_else(|| ModelContractError::new(ModelContractErrorKind::InvalidToolArguments))
    }

    fn semantic_value(&self) -> Value {
        json!({
            "arguments_valid_json": self.arguments_are_valid_json(),
            "call_id": self.call_id.as_str(),
            "name": self.name.as_str(),
            "parsed_arguments": self.parsed_arguments,
            "raw_arguments": self.raw_arguments,
        })
    }
}

impl fmt::Debug for CanonicalModelToolCall {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalModelToolCall")
            .field("call_id", &self.call_id)
            .field("name", &self.name)
            .field("raw_argument_bytes", &self.raw_arguments.len())
            .field("arguments_valid_json", &self.arguments_are_valid_json())
            .finish()
    }
}

/// Provider-scoped bounded opaque continuation/diagnostic evidence.
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderOpaqueEvidence {
    provider_id: ProviderId,
    type_label: ModelConfigReference,
    opaque: String,
    sha256: Sha256Digest,
}

impl ProviderOpaqueEvidence {
    pub fn try_new(
        provider_id: ProviderId,
        type_label: impl Into<String>,
        opaque: impl Into<String>,
    ) -> Result<Self, ModelContractError> {
        let opaque = opaque.into();
        if opaque.is_empty() || opaque.len() > MAX_MODEL_COMPONENT_BYTES {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidProviderEvidence,
            ));
        }
        Ok(Self {
            provider_id,
            type_label: ModelConfigReference::named(type_label)?,
            sha256: Sha256Digest::hash_bytes(opaque.as_bytes()),
            opaque,
        })
    }

    #[must_use]
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    #[must_use]
    pub fn type_label(&self) -> &str {
        self.type_label.as_str()
    }

    #[must_use]
    pub fn opaque(&self) -> &str {
        &self.opaque
    }

    #[must_use]
    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }

    fn semantic_value(&self) -> Value {
        json!({
            "opaque": self.opaque,
            "provider_id": self.provider_id.as_str(),
            "sha256": self.sha256.to_string(),
            "type_label": self.type_label.as_str(),
        })
    }
}

impl fmt::Debug for ProviderOpaqueEvidence {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderOpaqueEvidence")
            .field("provider_id", &self.provider_id)
            .field("type_label", &self.type_label)
            .field("opaque_bytes", &self.opaque.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

/// Provider-neutral input roles that preserve system/developer/user distinction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelInputRole {
    System,
    Developer,
    User,
}

impl ModelInputRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
        }
    }
}

/// Ordered provider-neutral context item. Stage 16 owns constructing these from history.
#[derive(Clone, Eq, PartialEq)]
pub enum ModelInputItem {
    Message {
        role: ModelInputRole,
        content_parts: Vec<ModelTextPart>,
    },
    PriorAssistant {
        content_parts: Vec<ModelTextPart>,
    },
    ToolCall(CanonicalModelToolCall),
    ToolResult {
        call_id: ModelToolCallId,
        result: Value,
    },
    HistoricalRefusal {
        content_parts: Vec<ModelTextPart>,
    },
    HistoricalReasoningSummary {
        content_parts: Vec<ModelTextPart>,
    },
    StructuredData {
        data: Value,
    },
    SyntheticRuntimeStatus {
        status: ModelConfigReference,
        details: Value,
    },
    ProviderOpaqueContinuation(ProviderOpaqueEvidence),
}

impl fmt::Debug for ModelInputItem {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ModelInputItem");
        match self {
            Self::Message {
                role,
                content_parts,
            } => debug
                .field("kind", &"message")
                .field("role", role)
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::PriorAssistant { content_parts } => debug
                .field("kind", &"prior_assistant")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::ToolCall(call) => debug.field("kind", &"tool_call").field("call", call),
            Self::ToolResult { call_id, result } => debug
                .field("kind", &"tool_result")
                .field("call_id", call_id)
                .field("result_bytes", &json_bytes(result)),
            Self::HistoricalRefusal { content_parts } => debug
                .field("kind", &"historical_refusal")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::HistoricalReasoningSummary { content_parts } => debug
                .field("kind", &"historical_reasoning_summary")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::StructuredData { data } => debug
                .field("kind", &"structured_data")
                .field("data_bytes", &json_bytes(data)),
            Self::SyntheticRuntimeStatus { status, details } => debug
                .field("kind", &"synthetic_runtime_status")
                .field("status", status)
                .field("details_bytes", &json_bytes(details)),
            Self::ProviderOpaqueContinuation(value) => debug
                .field("kind", &"provider_opaque_continuation")
                .field("evidence", value),
        };
        debug.finish()
    }
}

impl ModelInputItem {
    pub fn message(
        role: ModelInputRole,
        content_parts: Vec<ModelTextPart>,
    ) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::Message {
            role,
            content_parts,
        })
    }

    pub fn prior_assistant(content_parts: Vec<ModelTextPart>) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::PriorAssistant { content_parts })
    }

    pub fn historical_refusal(
        content_parts: Vec<ModelTextPart>,
    ) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::HistoricalRefusal { content_parts })
    }

    /// Constructs provider-exposed historical reasoning summary without reclassifying it as text.
    pub fn historical_reasoning_summary(
        content_parts: Vec<ModelTextPart>,
    ) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::HistoricalReasoningSummary { content_parts })
    }

    pub fn structured_data(data: Value) -> Result<Self, ModelContractError> {
        Ok(Self::StructuredData {
            data: bounded_json(data)?,
        })
    }

    pub fn tool_result(
        call_id: ModelToolCallId,
        result: Value,
    ) -> Result<Self, ModelContractError> {
        Ok(Self::ToolResult {
            call_id,
            result: bounded_json(result)?,
        })
    }

    pub fn synthetic_runtime_status(
        status: impl Into<String>,
        details: Value,
    ) -> Result<Self, ModelContractError> {
        Ok(Self::SyntheticRuntimeStatus {
            status: ModelConfigReference::named(status)?,
            details: bounded_json(details)?,
        })
    }

    fn semantic_value(&self) -> Value {
        match self {
            Self::Message {
                role,
                content_parts,
            } => json!({
                "content_parts": text_values(content_parts),
                "kind": "message",
                "role": role.as_str(),
            }),
            Self::PriorAssistant { content_parts } => json!({
                "content_parts": text_values(content_parts),
                "kind": "prior_assistant",
            }),
            Self::ToolCall(call) => json!({"call": call.semantic_value(), "kind": "tool_call"}),
            Self::ToolResult { call_id, result } => json!({
                "call_id": call_id.as_str(),
                "kind": "tool_result",
                "result": result,
            }),
            Self::HistoricalRefusal { content_parts } => json!({
                "content_parts": text_values(content_parts),
                "kind": "historical_refusal",
            }),
            Self::HistoricalReasoningSummary { content_parts } => json!({
                "content_parts": text_values(content_parts),
                "kind": "historical_reasoning_summary",
            }),
            Self::StructuredData { data } => json!({"data": data, "kind": "structured_data"}),
            Self::SyntheticRuntimeStatus { status, details } => json!({
                "details": details,
                "kind": "synthetic_runtime_status",
                "status": status.as_str(),
            }),
            Self::ProviderOpaqueContinuation(value) => json!({
                "evidence": value.semantic_value(),
                "kind": "provider_opaque_continuation",
            }),
        }
    }

    /// Exact compact canonical bytes used by Stage 16 contribution accounting.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_json_bytes(&self.semantic_value())
    }
}

/// Provider-neutral model-facing projection of a trusted Stage 14 definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelToolDefinition {
    name: ToolName,
    implementation_version: ToolVersion,
    schema_version: SchemaVersion,
    description: ModelTextPart,
    input_schema: Value,
}

impl ModelToolDefinition {
    pub fn try_new(
        name: ToolName,
        implementation_version: ToolVersion,
        schema_version: SchemaVersion,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Result<Self, ModelContractError> {
        Ok(Self {
            name,
            implementation_version,
            schema_version,
            description: ModelTextPart::try_new(description)?,
            input_schema: bounded_json(input_schema)?,
        })
    }

    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    #[must_use]
    pub const fn description(&self) -> &ModelTextPart {
        &self.description
    }

    #[must_use]
    pub const fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    /// Exact compact canonical model-visible definition bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_json_bytes(&self.semantic_value())
    }

    fn semantic_value(&self) -> Value {
        json!({
            "description": self.description.as_str(),
            "implementation_version": self.implementation_version.as_str(),
            "input_schema": self.input_schema,
            "name": self.name.as_str(),
            "schema_version": self.schema_version.get(),
        })
    }
}

/// Exact ordered model-visible toolset fingerprint used by Stage 16 request provenance.
#[must_use]
pub fn model_toolset_fingerprint(definitions: &[ModelToolDefinition]) -> Sha256Digest {
    let semantic = Value::Array(
        definitions
            .iter()
            .map(ModelToolDefinition::semantic_value)
            .collect(),
    );
    Sha256Digest::hash_bytes(&canonical_json_bytes(&semantic))
}

/// Closed V0 provider-neutral tool choice policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelToolChoicePolicy {
    Automatic,
    None,
}

impl ModelToolChoicePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::None => "none",
        }
    }
}

/// Provider-neutral request construction data.
pub struct ModelRequestInput {
    pub logical_invocation_id: LogicalInvocationId,
    pub target: ModelTarget,
    pub ordered_input_items: Vec<ModelInputItem>,
    pub instructions: Vec<ModelTextPart>,
    pub tool_definitions: Vec<ModelToolDefinition>,
    pub requested_output_limit: TokenCount,
    pub tool_choice_policy: ModelToolChoicePolicy,
    pub provider_native_options: ProviderNativeOptions,
    pub context_manifest_id: ContextManifestId,
}

/// Complete provider-neutral request. Parallel tool calls are fixed false by construction.
#[derive(Clone, Eq, PartialEq)]
pub struct ModelRequest {
    logical_invocation_id: LogicalInvocationId,
    target: ModelTarget,
    ordered_input_items: Vec<ModelInputItem>,
    instructions: Vec<ModelTextPart>,
    tool_definitions: Vec<ModelToolDefinition>,
    requested_output_limit: TokenCount,
    tool_choice_policy: ModelToolChoicePolicy,
    provider_native_options: ProviderNativeOptions,
    context_manifest_id: ContextManifestId,
}

impl fmt::Debug for ModelRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelRequest")
            .field("logical_invocation_id", &self.logical_invocation_id)
            .field("target", &self.target.identity())
            .field("ordered_input_item_count", &self.ordered_input_items.len())
            .field("instruction_count", &self.instructions.len())
            .field("tool_definition_count", &self.tool_definitions.len())
            .field("requested_output_limit", &self.requested_output_limit)
            .field("tool_choice_policy", &self.tool_choice_policy)
            .field("parallel_tool_calls", &false)
            .field("context_manifest_id", &self.context_manifest_id)
            .field("canonical_sha256", &self.canonical_sha256())
            .finish()
    }
}

impl ModelRequest {
    pub fn try_new(input: ModelRequestInput) -> Result<Self, ModelContractError> {
        if input.ordered_input_items.is_empty()
            || input.requested_output_limit
                > input.target.reference().capabilities().max_output_tokens()
            || input.provider_native_options != input.target.provider_native_options()
            || input.tool_choice_policy == ModelToolChoicePolicy::None
                && !input.tool_definitions.is_empty()
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidRequest,
            ));
        }
        let mut tools = BTreeSet::new();
        if input
            .tool_definitions
            .iter()
            .any(|definition| !tools.insert(definition.name().as_str()))
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidRequest,
            ));
        }
        Ok(Self {
            logical_invocation_id: input.logical_invocation_id,
            target: input.target,
            ordered_input_items: input.ordered_input_items,
            instructions: input.instructions,
            tool_definitions: input.tool_definitions,
            requested_output_limit: input.requested_output_limit,
            tool_choice_policy: input.tool_choice_policy,
            provider_native_options: input.provider_native_options,
            context_manifest_id: input.context_manifest_id,
        })
    }

    #[must_use]
    pub const fn logical_invocation_id(&self) -> LogicalInvocationId {
        self.logical_invocation_id
    }

    #[must_use]
    pub const fn target(&self) -> &ModelTarget {
        &self.target
    }

    #[must_use]
    pub fn ordered_input_items(&self) -> &[ModelInputItem] {
        &self.ordered_input_items
    }

    #[must_use]
    pub fn instructions(&self) -> &[ModelTextPart] {
        &self.instructions
    }

    #[must_use]
    pub fn tool_definitions(&self) -> &[ModelToolDefinition] {
        &self.tool_definitions
    }

    #[must_use]
    pub const fn requested_output_limit(&self) -> TokenCount {
        self.requested_output_limit
    }

    #[must_use]
    pub const fn tool_choice_policy(&self) -> ModelToolChoicePolicy {
        self.tool_choice_policy
    }

    #[must_use]
    pub const fn provider_native_options(&self) -> ProviderNativeOptions {
        self.provider_native_options
    }

    #[must_use]
    pub const fn parallel_tool_calls(&self) -> bool {
        false
    }

    #[must_use]
    pub const fn context_manifest_id(&self) -> ContextManifestId {
        self.context_manifest_id
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_json_bytes(&self.semantic_value())
    }

    #[must_use]
    pub fn canonical_sha256(&self) -> Sha256Digest {
        Sha256Digest::hash_bytes(&self.canonical_bytes())
    }

    fn semantic_value(&self) -> Value {
        json!({
            "context_manifest_id": self.context_manifest_id.to_string(),
            "instructions": text_values(&self.instructions),
            "logical_invocation_id": self.logical_invocation_id.to_string(),
            "ordered_input_items": self.ordered_input_items.iter().map(ModelInputItem::semantic_value).collect::<Vec<_>>(),
            "parallel_tool_calls": false,
            "provider_native_options": {
                "reasoning_continuation": self.provider_native_options.reasoning_continuation(),
            },
            "requested_output_limit": self.requested_output_limit.get(),
            "target": self.target.identity().semantic_value(),
            "tool_choice_policy": self.tool_choice_policy.as_str(),
            "tool_definitions": self.tool_definitions.iter().map(ModelToolDefinition::semantic_value).collect::<Vec<_>>(),
        })
    }
}

/// Ordered canonical response item inventory. Provider order is vector position.
#[derive(Clone, Eq, PartialEq)]
pub enum ModelOutputItem {
    Text { content_parts: Vec<ModelTextPart> },
    ToolCall(CanonicalModelToolCall),
    StructuredData { data: Value },
    Refusal { content_parts: Vec<ModelTextPart> },
    ReasoningSummary { content_parts: Vec<ModelTextPart> },
    ProviderOpaque(ProviderOpaqueEvidence),
    UnknownProviderItem(ProviderOpaqueEvidence),
}

impl fmt::Debug for ModelOutputItem {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ModelOutputItem");
        match self {
            Self::Text { content_parts } => debug
                .field("kind", &"text")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::ToolCall(call) => debug.field("kind", &"tool_call").field("call", call),
            Self::StructuredData { data } => debug
                .field("kind", &"structured_data")
                .field("data_bytes", &json_bytes(data)),
            Self::Refusal { content_parts } => debug
                .field("kind", &"refusal")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::ReasoningSummary { content_parts } => debug
                .field("kind", &"reasoning_summary")
                .field("content_part_count", &content_parts.len())
                .field("content_bytes", &text_part_bytes(content_parts)),
            Self::ProviderOpaque(value) => debug
                .field("kind", &"provider_opaque")
                .field("evidence", value),
            Self::UnknownProviderItem(value) => debug
                .field("kind", &"unknown_provider_item")
                .field("evidence", value),
        };
        debug.finish()
    }
}

impl ModelOutputItem {
    pub fn text(content_parts: Vec<ModelTextPart>) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::Text { content_parts })
    }

    pub fn refusal(content_parts: Vec<ModelTextPart>) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::Refusal { content_parts })
    }

    pub fn reasoning_summary(
        content_parts: Vec<ModelTextPart>,
    ) -> Result<Self, ModelContractError> {
        require_parts(&content_parts)?;
        Ok(Self::ReasoningSummary { content_parts })
    }

    pub fn structured_data(data: Value) -> Result<Self, ModelContractError> {
        Ok(Self::StructuredData {
            data: bounded_json(data)?,
        })
    }

    #[must_use]
    pub const fn is_semantic_output(&self) -> bool {
        true
    }

    /// Ordered text parts for answer, refusal, or provider-exposed reasoning-summary items.
    #[must_use]
    pub fn content_parts(&self) -> Option<&[ModelTextPart]> {
        match self {
            Self::Text { content_parts }
            | Self::Refusal { content_parts }
            | Self::ReasoningSummary { content_parts } => Some(content_parts),
            _ => None,
        }
    }

    /// Canonicalized structured data, when this is a structured-data item.
    #[must_use]
    pub const fn structured_data_value(&self) -> Option<&Value> {
        match self {
            Self::StructuredData { data } => Some(data),
            _ => None,
        }
    }

    /// Complete validated tool call, when this item requests one.
    #[must_use]
    pub const fn tool_call(&self) -> Option<&CanonicalModelToolCall> {
        match self {
            Self::ToolCall(call) => Some(call),
            _ => None,
        }
    }

    fn semantic_value(&self) -> Value {
        match self {
            Self::Text { content_parts } => {
                json!({"content_parts": text_values(content_parts), "kind": "text"})
            }
            Self::ToolCall(call) => {
                json!({"call": call.semantic_value(), "kind": "tool_call"})
            }
            Self::StructuredData { data } => {
                json!({"data": data, "kind": "structured_data"})
            }
            Self::Refusal { content_parts } => {
                json!({"content_parts": text_values(content_parts), "kind": "refusal"})
            }
            Self::ReasoningSummary { content_parts } => json!({
                "content_parts": text_values(content_parts),
                "kind": "reasoning_summary",
            }),
            Self::ProviderOpaque(value) => {
                json!({"evidence": value.semantic_value(), "kind": "provider_opaque"})
            }
            Self::UnknownProviderItem(value) => json!({
                "evidence": value.semantic_value(),
                "kind": "unknown_provider_item",
            }),
        }
    }
}

/// Provider-neutral terminal/incomplete reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelStopReason {
    Completed,
    ToolContinuation,
    Refusal,
    IncompleteProviderLimit,
    Cancelled,
    ProviderFailure,
}

impl ModelStopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ToolContinuation => "tool_continuation",
            Self::Refusal => "refusal",
            Self::IncompleteProviderLimit => "incomplete_provider_limit",
            Self::Cancelled => "cancelled",
            Self::ProviderFailure => "provider_failure",
        }
    }
}

/// Validated nonnegative provider usage compatible with durable SQLite i64 columns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
}

impl ModelUsage {
    pub fn try_new(
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
        reasoning_tokens: u64,
        total_tokens: u64,
    ) -> Result<Self, ModelContractError> {
        let calculated = input_tokens
            .checked_add(output_tokens)
            .ok_or_else(|| ModelContractError::new(ModelContractErrorKind::InvalidUsage))?;
        if [
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_tokens,
            total_tokens,
        ]
        .into_iter()
        .any(|value| value > i64::MAX as u64)
            || cached_input_tokens > input_tokens
            || reasoning_tokens > output_tokens
            || total_tokens != calculated
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidUsage,
            ));
        }
        Ok(Self {
            input_tokens,
            cached_input_tokens,
            output_tokens,
            reasoning_tokens,
            total_tokens,
        })
    }

    #[must_use]
    pub const fn input_tokens(self) -> u64 {
        self.input_tokens
    }
    #[must_use]
    pub const fn cached_input_tokens(self) -> u64 {
        self.cached_input_tokens
    }
    #[must_use]
    pub const fn output_tokens(self) -> u64 {
        self.output_tokens
    }
    #[must_use]
    pub const fn reasoning_tokens(self) -> u64 {
        self.reasoning_tokens
    }
    #[must_use]
    pub const fn total_tokens(self) -> u64 {
        self.total_tokens
    }

    fn semantic_value(self) -> Value {
        json!({
            "cached_input_tokens": self.cached_input_tokens,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "reasoning_tokens": self.reasoning_tokens,
            "total_tokens": self.total_tokens,
        })
    }
}

/// Bounded provider request/response identifier with no raw body or header data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderEvidenceId(String);

impl ProviderEvidenceId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, ModelContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROVIDER_EVIDENCE_ID_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidProviderEvidence,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Safe bounded provider metadata. Values cannot carry arbitrary provider text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderMetadataValue {
    Boolean(bool),
    Integer(i64),
    Identifier(ModelConfigReference),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderMetadata(BTreeMap<String, ProviderMetadataValue>);

impl ProviderMetadata {
    pub fn try_new(
        entries: impl IntoIterator<Item = (String, ProviderMetadataValue)>,
    ) -> Result<Self, ModelContractError> {
        let entries: BTreeMap<_, _> = entries.into_iter().collect();
        if entries.len() > MAX_METADATA_ENTRIES
            || entries.keys().any(|key| {
                key.is_empty()
                    || key.len() > 64
                    || !key.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-' | b'.')
                    })
            })
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidProviderEvidence,
            ));
        }
        Ok(Self(entries))
    }

    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<String, ProviderMetadataValue> {
        &self.0
    }

    fn semantic_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        ProviderMetadataValue::Boolean(value) => Value::Bool(*value),
                        ProviderMetadataValue::Integer(value) => (*value).into(),
                        ProviderMetadataValue::Identifier(value) => value.as_str().into(),
                    };
                    (key.clone(), value)
                })
                .collect(),
        )
    }
}

pub struct ModelResponseInput {
    pub selected_target: ModelTargetIdentity,
    pub output_items: Vec<ModelOutputItem>,
    pub stop_reason: ModelStopReason,
    pub usage: ModelUsage,
    pub provider_request_id: Option<ProviderEvidenceId>,
    pub provider_response_id: Option<ProviderEvidenceId>,
    pub provider_continuation: Option<ProviderOpaqueEvidence>,
    pub provider_metadata: ProviderMetadata,
}

/// Complete bounded normalized response with provider order preserved exactly.
#[derive(Clone, Eq, PartialEq)]
pub struct ModelResponse {
    selected_target: ModelTargetIdentity,
    output_items: Vec<ModelOutputItem>,
    stop_reason: ModelStopReason,
    usage: ModelUsage,
    provider_request_id: Option<ProviderEvidenceId>,
    provider_response_id: Option<ProviderEvidenceId>,
    provider_continuation: Option<ProviderOpaqueEvidence>,
    provider_metadata: ProviderMetadata,
}

impl fmt::Debug for ModelResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelResponse")
            .field("selected_target", &self.selected_target)
            .field("output_item_count", &self.output_items.len())
            .field("stop_reason", &self.stop_reason)
            .field("usage", &self.usage)
            .field("provider_request_id", &self.provider_request_id)
            .field("provider_response_id", &self.provider_response_id)
            .field(
                "has_provider_continuation",
                &self.provider_continuation.is_some(),
            )
            .field("provider_metadata", &self.provider_metadata)
            .field("canonical_sha256", &self.canonical_sha256())
            .finish()
    }
}

impl ModelResponse {
    pub fn try_new(input: ModelResponseInput) -> Result<Self, ModelContractError> {
        if input.output_items.len() > MAX_MODEL_OUTPUT_ITEMS {
            return Err(ModelContractError::new(
                ModelContractErrorKind::TooManyOutputItems,
            ));
        }
        let mut call_ids = BTreeSet::new();
        let mut semantics = ResponseSemantics::default();
        for item in &input.output_items {
            match item {
                ModelOutputItem::ToolCall(call) => {
                    semantics.tool_call = true;
                    if !call_ids.insert(call.call_id().as_str()) {
                        return Err(ModelContractError::new(
                            ModelContractErrorKind::DuplicateToolCallId,
                        ));
                    }
                }
                ModelOutputItem::Text { .. } => semantics.answer_text = true,
                ModelOutputItem::StructuredData { .. } => semantics.structured_data = true,
                ModelOutputItem::Refusal { .. } => semantics.refusal = true,
                ModelOutputItem::ReasoningSummary { .. } => semantics.reasoning_summary = true,
                ModelOutputItem::ProviderOpaque(_) => semantics.provider_opaque = true,
                ModelOutputItem::UnknownProviderItem(_) => semantics.unknown = true,
            }
        }
        let stop_is_consistent = match input.stop_reason {
            ModelStopReason::Completed => {
                semantics.has_normal_answer()
                    && !semantics.tool_call
                    && !semantics.refusal
                    && !semantics.unknown
            }
            ModelStopReason::ToolContinuation => {
                semantics.tool_call
                    && !semantics.structured_data
                    && !semantics.refusal
                    && !semantics.unknown
            }
            ModelStopReason::Refusal => {
                semantics.refusal
                    && !semantics.answer_text
                    && !semantics.tool_call
                    && !semantics.structured_data
                    && !semantics.reasoning_summary
                    && !semantics.unknown
            }
            ModelStopReason::IncompleteProviderLimit => {
                !semantics.tool_call && !semantics.refusal && !semantics.unknown
            }
            ModelStopReason::Cancelled => !semantics.has_exposed_semantics(),
            ModelStopReason::ProviderFailure => {
                !semantics.answer_text
                    && !semantics.tool_call
                    && !semantics.structured_data
                    && !semantics.refusal
                    && !semantics.reasoning_summary
            }
        };
        if !stop_is_consistent {
            return Err(ModelContractError::new(
                ModelContractErrorKind::InvalidTerminalSemantics,
            ));
        }
        let response = Self {
            selected_target: input.selected_target,
            output_items: input.output_items,
            stop_reason: input.stop_reason,
            usage: input.usage,
            provider_request_id: input.provider_request_id,
            provider_response_id: input.provider_response_id,
            provider_continuation: input.provider_continuation,
            provider_metadata: input.provider_metadata,
        };
        if response.canonical_bytes().len() > MAX_NORMALIZED_MODEL_RESPONSE_BYTES {
            return Err(ModelContractError::new(
                ModelContractErrorKind::NormalizedOutputTooLarge,
            ));
        }
        Ok(response)
    }

    #[must_use]
    pub const fn selected_target(&self) -> &ModelTargetIdentity {
        &self.selected_target
    }

    #[must_use]
    pub fn output_items(&self) -> &[ModelOutputItem] {
        &self.output_items
    }

    #[must_use]
    pub const fn stop_reason(&self) -> ModelStopReason {
        self.stop_reason
    }

    #[must_use]
    pub const fn usage(&self) -> ModelUsage {
        self.usage
    }

    #[must_use]
    pub const fn provider_request_id(&self) -> Option<&ProviderEvidenceId> {
        self.provider_request_id.as_ref()
    }

    #[must_use]
    pub const fn provider_response_id(&self) -> Option<&ProviderEvidenceId> {
        self.provider_response_id.as_ref()
    }

    #[must_use]
    pub const fn provider_continuation(&self) -> Option<&ProviderOpaqueEvidence> {
        self.provider_continuation.as_ref()
    }

    #[must_use]
    pub const fn provider_metadata(&self) -> &ProviderMetadata {
        &self.provider_metadata
    }

    pub fn require_supported_semantics(&self) -> Result<(), ModelContractError> {
        if self
            .output_items
            .iter()
            .any(|item| matches!(item, ModelOutputItem::UnknownProviderItem(_)))
        {
            return Err(ModelContractError::new(
                ModelContractErrorKind::UnknownSemanticItem,
            ));
        }
        for item in &self.output_items {
            if let ModelOutputItem::ToolCall(call) = item {
                call.require_valid_arguments()?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        canonical_json_bytes(&self.semantic_value())
    }

    #[must_use]
    pub fn canonical_sha256(&self) -> Sha256Digest {
        Sha256Digest::hash_bytes(&self.canonical_bytes())
    }

    fn semantic_value(&self) -> Value {
        json!({
            "output_items": self.output_items.iter().map(ModelOutputItem::semantic_value).collect::<Vec<_>>(),
            "provider_continuation": self.provider_continuation.as_ref().map(ProviderOpaqueEvidence::semantic_value),
            "provider_metadata": self.provider_metadata.semantic_value(),
            "provider_request_id": self.provider_request_id.as_ref().map(ProviderEvidenceId::as_str),
            "provider_response_id": self.provider_response_id.as_ref().map(ProviderEvidenceId::as_str),
            "selected_target": self.selected_target.semantic_value(),
            "stop_reason": self.stop_reason.as_str(),
            "usage": self.usage.semantic_value(),
        })
    }
}

#[derive(Default)]
struct ResponseSemantics {
    answer_text: bool,
    tool_call: bool,
    structured_data: bool,
    refusal: bool,
    reasoning_summary: bool,
    provider_opaque: bool,
    unknown: bool,
}

impl ResponseSemantics {
    const fn has_normal_answer(&self) -> bool {
        self.answer_text || self.structured_data
    }

    const fn has_exposed_semantics(&self) -> bool {
        self.answer_text
            || self.tool_call
            || self.structured_data
            || self.refusal
            || self.reasoning_summary
            || self.unknown
    }
}

/// Provider-neutral internal stream event; never an HTTP/SSE wire event.
#[derive(Clone, Eq, PartialEq)]
pub enum ModelStreamEvent {
    ResponseStarted {
        target: ModelTargetIdentity,
        provider_request_id: Option<ProviderEvidenceId>,
        provider_response_id: Option<ProviderEvidenceId>,
    },
    TextDelta {
        item_ordinal: u32,
        delta: ModelTextPart,
    },
    ReasoningSummaryDelta {
        item_ordinal: u32,
        delta: ModelTextPart,
    },
    ToolCallStarted {
        item_ordinal: u32,
        call_id: ModelToolCallId,
        name: ToolName,
    },
    ToolArgumentDelta {
        item_ordinal: u32,
        call_id: ModelToolCallId,
        delta: String,
    },
    ToolCallCompleted {
        item_ordinal: u32,
        call: CanonicalModelToolCall,
    },
    RefusalDelta {
        item_ordinal: u32,
        delta: ModelTextPart,
    },
    RefusalCompleted {
        item_ordinal: u32,
    },
    StructuredData {
        item_ordinal: u32,
        data: Value,
    },
    Usage(ModelUsage),
    UsageUnavailable,
    Completed(ModelResponse),
    ProviderError {
        kind: ModelStreamProviderErrorKind,
    },
    UnknownProviderEvent(ProviderOpaqueEvidence),
}

impl fmt::Debug for ModelStreamEvent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ModelStreamEvent");
        match self {
            Self::ResponseStarted {
                target,
                provider_request_id,
                provider_response_id,
            } => debug
                .field("kind", &"response_started")
                .field("target", target)
                .field("provider_request_id", provider_request_id)
                .field("provider_response_id", provider_response_id),
            Self::TextDelta {
                item_ordinal,
                delta,
            } => debug
                .field("kind", &"text_delta")
                .field("item_ordinal", item_ordinal)
                .field("delta_bytes", &delta.as_str().len()),
            Self::ReasoningSummaryDelta {
                item_ordinal,
                delta,
            } => debug
                .field("kind", &"reasoning_summary_delta")
                .field("item_ordinal", item_ordinal)
                .field("delta_bytes", &delta.as_str().len()),
            Self::ToolCallStarted {
                item_ordinal,
                call_id,
                name,
            } => debug
                .field("kind", &"tool_call_started")
                .field("item_ordinal", item_ordinal)
                .field("call_id", call_id)
                .field("name", name),
            Self::ToolArgumentDelta {
                item_ordinal,
                call_id,
                delta,
            } => debug
                .field("kind", &"tool_argument_delta")
                .field("item_ordinal", item_ordinal)
                .field("call_id", call_id)
                .field("delta_bytes", &delta.len()),
            Self::ToolCallCompleted { item_ordinal, call } => debug
                .field("kind", &"tool_call_completed")
                .field("item_ordinal", item_ordinal)
                .field("call", call),
            Self::RefusalDelta {
                item_ordinal,
                delta,
            } => debug
                .field("kind", &"refusal_delta")
                .field("item_ordinal", item_ordinal)
                .field("delta_bytes", &delta.as_str().len()),
            Self::RefusalCompleted { item_ordinal } => debug
                .field("kind", &"refusal_completed")
                .field("item_ordinal", item_ordinal),
            Self::StructuredData { item_ordinal, data } => debug
                .field("kind", &"structured_data")
                .field("item_ordinal", item_ordinal)
                .field("data_bytes", &json_bytes(data)),
            Self::Usage(usage) => debug.field("kind", &"usage").field("usage", usage),
            Self::UsageUnavailable => debug.field("kind", &"usage_unavailable"),
            Self::Completed(response) => debug
                .field("kind", &"completed")
                .field("response", response),
            Self::ProviderError { kind } => debug.field("kind", kind),
            Self::UnknownProviderEvent(value) => debug
                .field("kind", &"unknown_provider_event")
                .field("evidence", value),
        };
        debug.finish()
    }
}

/// Closed provider-neutral terminal stream error evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelStreamProviderErrorKind {
    DefiniteFailure,
    TransientUnavailable,
    Cancelled,
    OutcomeUnknown,
    TimeoutBeforeOutput,
    TimeoutAfterOutput,
    ProtocolFailure,
}

impl ModelStreamEvent {
    pub fn tool_argument_delta(
        item_ordinal: u32,
        call_id: ModelToolCallId,
        delta: impl Into<String>,
    ) -> Result<Self, ModelContractError> {
        let delta = delta.into();
        if delta.is_empty() || delta.len() > MAX_MODEL_TOOL_ARGUMENT_BYTES {
            return Err(ModelContractError::new(
                ModelContractErrorKind::ToolArgumentsTooLarge,
            ));
        }
        Ok(Self::ToolArgumentDelta {
            item_ordinal,
            call_id,
            delta,
        })
    }

    #[must_use]
    pub const fn is_semantic_output(&self) -> bool {
        matches!(
            self,
            Self::TextDelta { .. }
                | Self::ReasoningSummaryDelta { .. }
                | Self::ToolCallStarted { .. }
                | Self::ToolArgumentDelta { .. }
                | Self::ToolCallCompleted { .. }
                | Self::RefusalDelta { .. }
                | Self::RefusalCompleted { .. }
                | Self::StructuredData { .. }
                | Self::UnknownProviderEvent(_)
        )
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed(_) | Self::ProviderError { .. })
    }
}

/// Pure stream-state classifier used by later retry and terminal orchestration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelStreamState {
    NoSemanticOutput,
    SemanticOutputObserved,
    Completed,
    DefiniteProviderError,
    Cancelled,
    AmbiguousTransportLoss,
    TimeoutBeforeOutput,
    TimeoutAfterOutput,
    ProtocolFailure,
}

struct ActiveToolCall {
    item_ordinal: u32,
    name: ToolName,
    arguments: String,
}

struct ModelStreamValidator {
    started_target: Option<ModelTargetIdentity>,
    semantic_output_observed: bool,
    usage: Option<ModelUsage>,
    usage_unavailable: bool,
    terminal: Option<ModelStreamState>,
    active_calls: BTreeMap<String, ActiveToolCall>,
    seen_call_ids: BTreeSet<String>,
}

impl ModelStreamValidator {
    fn new() -> Self {
        Self {
            started_target: None,
            semantic_output_observed: false,
            usage: None,
            usage_unavailable: false,
            terminal: None,
            active_calls: BTreeMap::new(),
            seen_call_ids: BTreeSet::new(),
        }
    }

    fn reject() -> ModelContractError {
        ModelContractError::new(ModelContractErrorKind::InvalidStreamOrdering)
    }

    fn usage_result_seen(&self) -> bool {
        self.usage.is_some() || self.usage_unavailable
    }

    fn observe(&mut self, event: &ModelStreamEvent) -> Result<(), ModelContractError> {
        if self.terminal.is_some() {
            return Err(Self::reject());
        }
        if self.started_target.is_none() {
            if let ModelStreamEvent::ResponseStarted { target, .. } = event {
                self.started_target = Some(target.clone());
                return Ok(());
            }
            return Err(Self::reject());
        }
        if matches!(event, ModelStreamEvent::ResponseStarted { .. }) {
            return Err(Self::reject());
        }
        if self.usage_result_seen() && !event.is_terminal() {
            return Err(Self::reject());
        }

        match event {
            ModelStreamEvent::TextDelta { item_ordinal, .. }
            | ModelStreamEvent::ReasoningSummaryDelta { item_ordinal, .. }
            | ModelStreamEvent::RefusalDelta { item_ordinal, .. }
            | ModelStreamEvent::RefusalCompleted { item_ordinal }
            | ModelStreamEvent::StructuredData { item_ordinal, .. }
                if *item_ordinal as usize >= MAX_MODEL_OUTPUT_ITEMS =>
            {
                return Err(Self::reject());
            }
            ModelStreamEvent::ToolCallStarted {
                item_ordinal,
                call_id,
                name,
            } => {
                let call_id = call_id.as_str().to_owned();
                if *item_ordinal as usize >= MAX_MODEL_OUTPUT_ITEMS
                    || !self.seen_call_ids.insert(call_id.clone())
                {
                    return Err(Self::reject());
                }
                self.active_calls.insert(
                    call_id,
                    ActiveToolCall {
                        item_ordinal: *item_ordinal,
                        name: name.clone(),
                        arguments: String::new(),
                    },
                );
            }
            ModelStreamEvent::ToolArgumentDelta {
                item_ordinal,
                call_id,
                delta,
            } => {
                let Some(active) = self.active_calls.get_mut(call_id.as_str()) else {
                    return Err(Self::reject());
                };
                if active.item_ordinal != *item_ordinal {
                    return Err(Self::reject());
                }
                if active.arguments.len().saturating_add(delta.len())
                    > MAX_MODEL_TOOL_ARGUMENT_BYTES
                {
                    return Err(ModelContractError::new(
                        ModelContractErrorKind::ToolArgumentsTooLarge,
                    ));
                }
                active.arguments.push_str(delta);
            }
            ModelStreamEvent::ToolCallCompleted { item_ordinal, call } => {
                let Some(active) = self.active_calls.remove(call.call_id().as_str()) else {
                    return Err(Self::reject());
                };
                if active.item_ordinal != *item_ordinal
                    || active.name != *call.name()
                    || active.arguments != call.raw_arguments()
                {
                    return Err(Self::reject());
                }
            }
            ModelStreamEvent::Usage(usage) => {
                if self.usage_result_seen() {
                    return Err(Self::reject());
                }
                self.usage = Some(*usage);
            }
            ModelStreamEvent::UsageUnavailable => {
                if self.usage_result_seen() {
                    return Err(Self::reject());
                }
                self.usage_unavailable = true;
            }
            ModelStreamEvent::Completed(response) => {
                if !self.active_calls.is_empty()
                    || self.usage != Some(response.usage())
                    || self.usage_unavailable
                    || self.started_target.as_ref() != Some(response.selected_target())
                {
                    return Err(Self::reject());
                }
                response.require_supported_semantics()?;
                self.terminal = Some(ModelStreamState::Completed);
            }
            ModelStreamEvent::ProviderError { kind } => {
                if !self.usage_result_seen()
                    || matches!(kind, ModelStreamProviderErrorKind::TimeoutBeforeOutput)
                        && self.semantic_output_observed
                    || matches!(kind, ModelStreamProviderErrorKind::TimeoutAfterOutput)
                        && !self.semantic_output_observed
                    || matches!(kind, ModelStreamProviderErrorKind::TransientUnavailable)
                        && self.semantic_output_observed
                {
                    return Err(Self::reject());
                }
                self.terminal = Some(match kind {
                    ModelStreamProviderErrorKind::DefiniteFailure
                    | ModelStreamProviderErrorKind::TransientUnavailable => {
                        ModelStreamState::DefiniteProviderError
                    }
                    ModelStreamProviderErrorKind::Cancelled => ModelStreamState::Cancelled,
                    ModelStreamProviderErrorKind::OutcomeUnknown => {
                        ModelStreamState::AmbiguousTransportLoss
                    }
                    ModelStreamProviderErrorKind::TimeoutBeforeOutput => {
                        ModelStreamState::TimeoutBeforeOutput
                    }
                    ModelStreamProviderErrorKind::TimeoutAfterOutput => {
                        ModelStreamState::TimeoutAfterOutput
                    }
                    ModelStreamProviderErrorKind::ProtocolFailure => {
                        ModelStreamState::ProtocolFailure
                    }
                });
            }
            ModelStreamEvent::ResponseStarted { .. } => unreachable!("duplicate start rejected"),
            _ => {}
        }
        self.semantic_output_observed |= event.is_semantic_output();
        Ok(())
    }

    fn state(&self) -> ModelStreamState {
        self.terminal.unwrap_or(if self.semantic_output_observed {
            ModelStreamState::SemanticOutputObserved
        } else {
            ModelStreamState::NoSemanticOutput
        })
    }
}

/// Validates provider-neutral stream ordering without reordering or dropping events.
pub fn validate_model_stream(
    events: &[ModelStreamEvent],
) -> Result<ModelStreamState, ModelContractError> {
    let mut validator = ModelStreamValidator::new();
    for event in events {
        validator.observe(event)?;
    }
    if validator.started_target.is_none() {
        return Err(ModelStreamValidator::reject());
    }
    Ok(validator.state())
}

fn require_parts(parts: &[ModelTextPart]) -> Result<(), ModelContractError> {
    if parts.is_empty() {
        Err(ModelContractError::new(ModelContractErrorKind::InvalidText))
    } else {
        Ok(())
    }
}

fn text_values(parts: &[ModelTextPart]) -> Vec<&str> {
    parts.iter().map(ModelTextPart::as_str).collect()
}

fn text_part_bytes(parts: &[ModelTextPart]) -> usize {
    parts.iter().map(|part| part.as_str().len()).sum()
}

fn json_bytes(value: &Value) -> usize {
    canonical_json_bytes(value).len()
}

fn bounded_json(value: Value) -> Result<Value, ModelContractError> {
    let value = canonicalize_json(value);
    if canonical_json_bytes(&value).len() > MAX_MODEL_COMPONENT_BYTES {
        return Err(ModelContractError::new(
            ModelContractErrorKind::InvalidStructuredData,
        ));
    }
    Ok(value)
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.into_iter().collect();
            keys.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                keys.into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        scalar => scalar,
    }
}

fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&canonicalize_json(value.clone()))
        .expect("provider-neutral canonical JSON value must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ModelCapabilitySnapshotInput, ProviderModelReference};

    const V7_A: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";
    const V7_B: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0e";

    fn target() -> ModelTarget {
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            context_window_tokens: TokenCount::try_new(128_000).unwrap(),
            max_output_tokens: TokenCount::try_new(16_384).unwrap(),
        });
        ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new("primary").unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new("fixture-primary-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("fixture-account").unwrap(),
            requested_output_tokens: TokenCount::try_new(8_192).unwrap(),
            estimator: TokenEstimatorIdentity::try_new("conservative_v1", 1).unwrap(),
            provider_native_options: ProviderNativeOptions::new(true),
        })
        .unwrap()
    }

    fn text(value: &str) -> ModelTextPart {
        ModelTextPart::try_new(value).unwrap()
    }

    fn request_with(items: Vec<ModelInputItem>, instructions: Vec<ModelTextPart>) -> ModelRequest {
        ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: V7_A.parse().unwrap(),
            target: target(),
            ordered_input_items: items,
            instructions,
            tool_definitions: vec![],
            requested_output_limit: TokenCount::try_new(256).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::Automatic,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: V7_B.parse().unwrap(),
        })
        .unwrap()
    }

    fn usage() -> ModelUsage {
        ModelUsage::try_new(10, 2, 5, 1, 15).unwrap()
    }

    fn response(items: Vec<ModelOutputItem>) -> Result<ModelResponse, ModelContractError> {
        let has_tool = items
            .iter()
            .any(|item| matches!(item, ModelOutputItem::ToolCall(_)));
        let has_refusal = items
            .iter()
            .any(|item| matches!(item, ModelOutputItem::Refusal { .. }));
        let stop_reason = match (has_tool, has_refusal) {
            (true, true) => ModelStopReason::ProviderFailure,
            (true, false) => ModelStopReason::ToolContinuation,
            (false, true) => ModelStopReason::Refusal,
            (false, false) => ModelStopReason::Completed,
        };
        ModelResponse::try_new(ModelResponseInput {
            selected_target: target().identity(),
            output_items: items,
            stop_reason,
            usage: usage(),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        })
    }

    #[test]
    fn canonical_request_hash_is_stable_and_order_sensitive() {
        let first = request_with(
            vec![ModelInputItem::message(ModelInputRole::User, vec![text("a")]).unwrap()],
            vec![text("one"), text("two")],
        );
        let same = first.clone();
        assert_eq!(first.canonical_bytes(), same.canonical_bytes());
        assert_eq!(first.canonical_sha256(), same.canonical_sha256());
        let reordered = request_with(
            vec![ModelInputItem::message(ModelInputRole::User, vec![text("a")]).unwrap()],
            vec![text("two"), text("one")],
        );
        assert_ne!(first.canonical_sha256(), reordered.canonical_sha256());
        assert!(
            String::from_utf8(first.canonical_bytes())
                .unwrap()
                .contains("\"parallel_tool_calls\":false")
        );
    }

    #[test]
    fn request_preserves_input_order_target_policy_and_output_limit() {
        let request = request_with(
            vec![
                ModelInputItem::message(ModelInputRole::System, vec![text("system")]).unwrap(),
                ModelInputItem::message(ModelInputRole::User, vec![text("user")]).unwrap(),
            ],
            vec![],
        );
        assert_eq!(request.ordered_input_items().len(), 2);
        assert_eq!(
            request.target().identity().model_target_id().as_str(),
            "primary"
        );
        assert_eq!(
            request.tool_choice_policy(),
            ModelToolChoicePolicy::Automatic
        );
        assert_eq!(request.requested_output_limit().get(), 256);
        assert!(!request.parallel_tool_calls());
    }

    #[test]
    fn every_input_variant_is_provider_neutral_and_ordered() {
        let call_id = ModelToolCallId::try_new("call-1").unwrap();
        let opaque = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "continuation-v1",
            "opaque",
        )
        .unwrap();
        let items = vec![
            ModelInputItem::message(ModelInputRole::Developer, vec![text("developer")]).unwrap(),
            ModelInputItem::prior_assistant(vec![text("assistant")]).unwrap(),
            ModelInputItem::ToolCall(
                CanonicalModelToolCall::try_new(call_id.clone(), "read_file", "{}").unwrap(),
            ),
            ModelInputItem::tool_result(call_id, json!({"ok": true})).unwrap(),
            ModelInputItem::historical_refusal(vec![text("refused")]).unwrap(),
            ModelInputItem::structured_data(json!({"b": 2, "a": 1})).unwrap(),
            ModelInputItem::synthetic_runtime_status("ready", json!({"generation": 1})).unwrap(),
            ModelInputItem::ProviderOpaqueContinuation(opaque),
        ];
        let request = request_with(items, vec![]);
        let json = String::from_utf8(request.canonical_bytes()).unwrap();
        let positions = [
            "\"kind\":\"message\"",
            "\"kind\":\"prior_assistant\"",
            "\"kind\":\"tool_call\"",
            "\"kind\":\"tool_result\"",
            "\"kind\":\"historical_refusal\"",
            "\"kind\":\"structured_data\"",
            "\"kind\":\"synthetic_runtime_status\"",
            "\"kind\":\"provider_opaque_continuation\"",
        ]
        .map(|needle| json.find(needle).unwrap());
        assert!(positions.windows(2).all(|window| window[0] < window[1]));
    }

    #[test]
    fn tool_definition_is_stable_and_duplicate_names_are_rejected() {
        let definition = ModelToolDefinition::try_new(
            ToolName::try_new("read_file").unwrap(),
            ToolVersion::try_new("1.0.0").unwrap(),
            SchemaVersion::try_new(1).unwrap(),
            "Read a file",
            json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        )
        .unwrap();
        let result = ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: V7_A.parse().unwrap(),
            target: target(),
            ordered_input_items: vec![
                ModelInputItem::message(ModelInputRole::User, vec![text("x")]).unwrap(),
            ],
            instructions: vec![],
            tool_definitions: vec![definition.clone(), definition],
            requested_output_limit: TokenCount::try_new(1).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::Automatic,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: V7_B.parse().unwrap(),
        });
        assert_eq!(
            result.unwrap_err().kind(),
            ModelContractErrorKind::InvalidRequest
        );
    }

    #[test]
    fn request_rejects_output_limit_above_target_and_no_tool_policy_with_tools() {
        let oversized = ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: V7_A.parse().unwrap(),
            target: target(),
            ordered_input_items: vec![
                ModelInputItem::message(ModelInputRole::User, vec![text("x")]).unwrap(),
            ],
            instructions: vec![],
            tool_definitions: vec![],
            requested_output_limit: TokenCount::try_new(16_385).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::Automatic,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: V7_B.parse().unwrap(),
        });
        assert_eq!(
            oversized.unwrap_err().kind(),
            ModelContractErrorKind::InvalidRequest
        );
        let definition = ModelToolDefinition::try_new(
            ToolName::try_new("read_file").unwrap(),
            ToolVersion::try_new("1.0.0").unwrap(),
            SchemaVersion::try_new(1).unwrap(),
            "Read a file",
            json!({"type": "object"}),
        )
        .unwrap();
        let no_tools = ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: V7_A.parse().unwrap(),
            target: target(),
            ordered_input_items: vec![
                ModelInputItem::message(ModelInputRole::User, vec![text("x")]).unwrap(),
            ],
            instructions: vec![],
            tool_definitions: vec![definition],
            requested_output_limit: TokenCount::try_new(1).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::None,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: V7_B.parse().unwrap(),
        });
        assert_eq!(
            no_tools.unwrap_err().kind(),
            ModelContractErrorKind::InvalidRequest
        );
    }

    #[test]
    fn output_variants_and_text_parts_preserve_exact_order() {
        let opaque = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "opaque-v1",
            "opaque",
        )
        .unwrap();
        let items = vec![
            ModelOutputItem::text(vec![text("a"), text("b")]).unwrap(),
            ModelOutputItem::structured_data(json!({"answer": 42})).unwrap(),
            ModelOutputItem::reasoning_summary(vec![text("summary")]).unwrap(),
            ModelOutputItem::ProviderOpaque(opaque.clone()),
        ];
        let response = response(items).unwrap();
        assert_eq!(response.output_items().len(), 4);
        let ModelOutputItem::Text { content_parts } = &response.output_items()[0] else {
            panic!()
        };
        assert_eq!(
            content_parts
                .iter()
                .map(ModelTextPart::as_str)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(response.require_supported_semantics().is_ok());
        assert!(matches!(
            ModelOutputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    ModelToolCallId::try_new("call-1").unwrap(),
                    "read_file",
                    "{}",
                )
                .unwrap()
            ),
            ModelOutputItem::ToolCall(_)
        ));
        assert!(matches!(
            ModelOutputItem::refusal(vec![text("no")]).unwrap(),
            ModelOutputItem::Refusal { .. }
        ));
        assert!(matches!(
            ModelOutputItem::UnknownProviderItem(opaque),
            ModelOutputItem::UnknownProviderItem(_)
        ));
    }

    #[test]
    fn output_item_limit_is_inclusive_without_truncation() {
        let sixty_four = (0..64)
            .map(|index| ModelOutputItem::text(vec![text(&format!("part-{index}"))]).unwrap())
            .collect();
        assert_eq!(response(sixty_four).unwrap().output_items().len(), 64);
        let sixty_five = (0..65)
            .map(|index| ModelOutputItem::text(vec![text(&format!("part-{index}"))]).unwrap())
            .collect();
        assert_eq!(
            response(sixty_five).unwrap_err().kind(),
            ModelContractErrorKind::TooManyOutputItems
        );
    }

    #[test]
    fn actual_serialized_response_bytes_enforce_the_envelope_limit() {
        let items = (0..5)
            .map(|_| ModelOutputItem::text(vec![text(&"x".repeat(60_000))]).unwrap())
            .collect();
        assert_eq!(
            response(items).unwrap_err().kind(),
            ModelContractErrorKind::NormalizedOutputTooLarge
        );
    }

    #[test]
    fn tool_call_preserves_raw_bytes_parses_json_and_reports_malformed_truthfully() {
        let raw = "{ \"b\": 2, \"a\": 1 }";
        let call = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("call-1").unwrap(),
            "run_shell",
            raw,
        )
        .unwrap();
        assert_eq!(call.raw_arguments(), raw);
        assert_eq!(call.parsed_arguments().unwrap(), &json!({"a": 1, "b": 2}));
        let malformed = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("call-2").unwrap(),
            "run_shell",
            "{",
        )
        .unwrap();
        assert_eq!(malformed.raw_arguments(), "{");
        assert!(!malformed.arguments_are_valid_json());
        assert_eq!(
            malformed.require_valid_arguments().unwrap_err().kind(),
            ModelContractErrorKind::InvalidToolArguments
        );
        let empty = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("call-3").unwrap(),
            "run_shell",
            "",
        )
        .unwrap();
        assert_eq!(empty.raw_arguments(), "");
        assert!(!empty.arguments_are_valid_json());
    }

    #[test]
    fn tool_argument_and_name_boundaries_are_exact() {
        let accepted = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("call-1").unwrap(),
            "read_file",
            "x".repeat(MAX_MODEL_TOOL_ARGUMENT_BYTES),
        )
        .unwrap();
        assert_eq!(
            accepted.raw_arguments().len(),
            MAX_MODEL_TOOL_ARGUMENT_BYTES
        );
        assert_eq!(
            CanonicalModelToolCall::try_new(
                ModelToolCallId::try_new("call-2").unwrap(),
                "read_file",
                "x".repeat(MAX_MODEL_TOOL_ARGUMENT_BYTES + 1),
            )
            .unwrap_err()
            .kind(),
            ModelContractErrorKind::ToolArgumentsTooLarge
        );
        assert_eq!(
            CanonicalModelToolCall::try_new(
                ModelToolCallId::try_new("call-3").unwrap(),
                "Bad Tool",
                "{}",
            )
            .unwrap_err()
            .kind(),
            ModelContractErrorKind::InvalidToolName
        );
    }

    #[test]
    fn duplicate_tool_call_ids_are_rejected_without_reordering() {
        let make = || {
            ModelOutputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    ModelToolCallId::try_new("same").unwrap(),
                    "read_file",
                    "{}",
                )
                .unwrap(),
            )
        };
        assert_eq!(
            response(vec![make(), make()]).unwrap_err().kind(),
            ModelContractErrorKind::DuplicateToolCallId
        );
    }

    #[test]
    fn response_stop_reason_and_empty_terminal_semantics_fail_closed() {
        let empty = ModelResponse::try_new(ModelResponseInput {
            selected_target: target().identity(),
            output_items: vec![],
            stop_reason: ModelStopReason::Completed,
            usage: usage(),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        });
        assert_eq!(
            empty.unwrap_err().kind(),
            ModelContractErrorKind::InvalidTerminalSemantics
        );
        let call = ModelOutputItem::ToolCall(
            CanonicalModelToolCall::try_new(
                ModelToolCallId::try_new("call-1").unwrap(),
                "read_file",
                "{}",
            )
            .unwrap(),
        );
        let wrong_stop = ModelResponse::try_new(ModelResponseInput {
            selected_target: target().identity(),
            output_items: vec![call],
            stop_reason: ModelStopReason::Completed,
            usage: usage(),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        });
        assert_eq!(
            wrong_stop.unwrap_err().kind(),
            ModelContractErrorKind::InvalidTerminalSemantics
        );
    }

    #[test]
    fn response_terminal_consistency_matrix_fails_closed() {
        #[derive(Clone, Copy, Debug)]
        enum SemanticClass {
            TextAnswer,
            ExecutableToolCall,
            StructuredData,
            Refusal,
            ReasoningSummary,
            ProviderOpaqueContinuation,
            UnknownProviderItem,
        }

        const STOP_REASONS: [ModelStopReason; 6] = [
            ModelStopReason::Completed,
            ModelStopReason::ToolContinuation,
            ModelStopReason::Refusal,
            ModelStopReason::IncompleteProviderLimit,
            ModelStopReason::Cancelled,
            ModelStopReason::ProviderFailure,
        ];
        const SEMANTIC_CLASSES: [SemanticClass; 7] = [
            SemanticClass::TextAnswer,
            SemanticClass::ExecutableToolCall,
            SemanticClass::StructuredData,
            SemanticClass::Refusal,
            SemanticClass::ReasoningSummary,
            SemanticClass::ProviderOpaqueContinuation,
            SemanticClass::UnknownProviderItem,
        ];
        // Independently declared test oracle. Rows follow STOP_REASONS and columns follow
        // SEMANTIC_CLASSES; this table deliberately shares no production classifier or mask.
        const EXPECTED_VALIDITY: [[bool; 7]; 6] = [
            [true, false, true, false, false, false, false],
            [false, true, false, false, false, false, false],
            [false, false, false, true, false, false, false],
            [true, false, true, false, true, true, false],
            [false, false, false, false, false, true, false],
            [false, false, false, false, false, true, true],
        ];

        let item = |semantic_class| match semantic_class {
            SemanticClass::TextAnswer => ModelOutputItem::text(vec![text("answer")]).unwrap(),
            SemanticClass::ExecutableToolCall => ModelOutputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    ModelToolCallId::try_new("call-1").unwrap(),
                    "read_file",
                    "{}",
                )
                .unwrap(),
            ),
            SemanticClass::StructuredData => {
                ModelOutputItem::structured_data(json!({"answer": 1})).unwrap()
            }
            SemanticClass::Refusal => ModelOutputItem::refusal(vec![text("cannot")]).unwrap(),
            SemanticClass::ReasoningSummary => {
                ModelOutputItem::reasoning_summary(vec![text("provider-exposed summary")]).unwrap()
            }
            SemanticClass::ProviderOpaqueContinuation => ModelOutputItem::ProviderOpaque(
                ProviderOpaqueEvidence::try_new(
                    ProviderId::try_new("fixture").unwrap(),
                    "continuation-v1",
                    "opaque",
                )
                .unwrap(),
            ),
            SemanticClass::UnknownProviderItem => ModelOutputItem::UnknownProviderItem(
                ProviderOpaqueEvidence::try_new(
                    ProviderId::try_new("fixture").unwrap(),
                    "future-v1",
                    "unknown",
                )
                .unwrap(),
            ),
        };

        let mut observed_cases = 0;
        for (stop_index, stop_reason) in STOP_REASONS.into_iter().enumerate() {
            for (semantic_index, semantic_class) in SEMANTIC_CLASSES.into_iter().enumerate() {
                let expected_validity = EXPECTED_VALIDITY[stop_index][semantic_index];
                let result = ModelResponse::try_new(ModelResponseInput {
                    selected_target: target().identity(),
                    output_items: vec![item(semantic_class)],
                    stop_reason,
                    usage: usage(),
                    provider_request_id: None,
                    provider_response_id: None,
                    provider_continuation: None,
                    provider_metadata: ProviderMetadata::default(),
                });
                let observed_validity = result.is_ok();
                assert_eq!(
                    observed_validity, expected_validity,
                    "terminal matrix mismatch: stop_reason={stop_reason:?}, semantic_class={semantic_class:?}, expected_validity={expected_validity}, observed_validity={observed_validity}",
                );
                if matches!(semantic_class, SemanticClass::UnknownProviderItem)
                    && let Ok(response) = result
                {
                    assert_eq!(
                        response.require_supported_semantics().unwrap_err().kind(),
                        ModelContractErrorKind::UnknownSemanticItem,
                        "unknown semantic support mismatch: stop_reason={stop_reason:?}, semantic_class={semantic_class:?}",
                    );
                }
                observed_cases += 1;
            }
        }
        assert_eq!(observed_cases, 42);
    }

    #[test]
    fn response_terminal_mixed_combinations_make_contradictions_explicit() {
        #[derive(Clone, Copy, Debug)]
        enum SemanticClass {
            TextAnswer,
            ExecutableToolCall(&'static str),
            StructuredData,
            Refusal,
            ReasoningSummary,
            UnknownProviderItem,
        }

        let item = |semantic_class| match semantic_class {
            SemanticClass::TextAnswer => ModelOutputItem::text(vec![text("answer")]).unwrap(),
            SemanticClass::ExecutableToolCall(call_id) => ModelOutputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    ModelToolCallId::try_new(call_id).unwrap(),
                    "read_file",
                    "{}",
                )
                .unwrap(),
            ),
            SemanticClass::StructuredData => {
                ModelOutputItem::structured_data(json!({"answer": 1})).unwrap()
            }
            SemanticClass::Refusal => ModelOutputItem::refusal(vec![text("cannot")]).unwrap(),
            SemanticClass::ReasoningSummary => {
                ModelOutputItem::reasoning_summary(vec![text("provider-exposed summary")]).unwrap()
            }
            SemanticClass::UnknownProviderItem => ModelOutputItem::UnknownProviderItem(
                ProviderOpaqueEvidence::try_new(
                    ProviderId::try_new("fixture").unwrap(),
                    "future-v1",
                    "unknown",
                )
                .unwrap(),
            ),
        };
        let cases = [
            (
                "refusal + tool call",
                ModelStopReason::Refusal,
                vec![
                    SemanticClass::Refusal,
                    SemanticClass::ExecutableToolCall("call-1"),
                ],
                false,
            ),
            (
                "refusal + text",
                ModelStopReason::Refusal,
                vec![SemanticClass::Refusal, SemanticClass::TextAnswer],
                false,
            ),
            (
                "refusal + reasoning summary",
                ModelStopReason::Refusal,
                vec![SemanticClass::Refusal, SemanticClass::ReasoningSummary],
                false,
            ),
            (
                "provider failure + tool",
                ModelStopReason::ProviderFailure,
                vec![SemanticClass::ExecutableToolCall("call-1")],
                false,
            ),
            (
                "provider failure + text",
                ModelStopReason::ProviderFailure,
                vec![SemanticClass::TextAnswer],
                false,
            ),
            (
                "provider failure + reasoning summary",
                ModelStopReason::ProviderFailure,
                vec![SemanticClass::ReasoningSummary],
                false,
            ),
            (
                "cancelled + tool",
                ModelStopReason::Cancelled,
                vec![SemanticClass::ExecutableToolCall("call-1")],
                false,
            ),
            (
                "cancelled + text",
                ModelStopReason::Cancelled,
                vec![SemanticClass::TextAnswer],
                false,
            ),
            (
                "incomplete + tool",
                ModelStopReason::IncompleteProviderLimit,
                vec![SemanticClass::ExecutableToolCall("call-1")],
                false,
            ),
            (
                "completed text + reasoning summary",
                ModelStopReason::Completed,
                vec![SemanticClass::TextAnswer, SemanticClass::ReasoningSummary],
                true,
            ),
            (
                "tool continuation text + tool call",
                ModelStopReason::ToolContinuation,
                vec![
                    SemanticClass::TextAnswer,
                    SemanticClass::ExecutableToolCall("call-1"),
                ],
                true,
            ),
            (
                "tool continuation two complete tool calls",
                ModelStopReason::ToolContinuation,
                vec![
                    SemanticClass::ExecutableToolCall("call-1"),
                    SemanticClass::ExecutableToolCall("call-2"),
                ],
                true,
            ),
            (
                "completed structured + reasoning summary",
                ModelStopReason::Completed,
                vec![
                    SemanticClass::StructuredData,
                    SemanticClass::ReasoningSummary,
                ],
                true,
            ),
            (
                "unknown + otherwise valid completed output",
                ModelStopReason::Completed,
                vec![
                    SemanticClass::TextAnswer,
                    SemanticClass::UnknownProviderItem,
                ],
                false,
            ),
        ];

        for (name, stop_reason, semantic_classes, expected_validity) in cases {
            let output_items = semantic_classes.into_iter().map(&item).collect();
            let result = ModelResponse::try_new(ModelResponseInput {
                selected_target: target().identity(),
                output_items,
                stop_reason,
                usage: usage(),
                provider_request_id: None,
                provider_response_id: None,
                provider_continuation: None,
                provider_metadata: ProviderMetadata::default(),
            });
            let observed_validity = result.is_ok();
            assert_eq!(
                observed_validity, expected_validity,
                "mixed terminal mismatch: case={name}, stop_reason={stop_reason:?}, expected_validity={expected_validity}, observed_validity={observed_validity}",
            );
        }
    }

    #[test]
    fn usage_accepts_boundaries_and_rejects_every_contradiction_or_overflow() {
        assert_eq!(
            ModelUsage::try_new(0, 0, 0, 0, 0).unwrap().total_tokens(),
            0
        );
        assert!(ModelUsage::try_new(5, 5, 7, 7, 12).is_ok());
        assert_eq!(
            ModelUsage::try_new(5, 6, 7, 0, 12).unwrap_err().kind(),
            ModelContractErrorKind::InvalidUsage
        );
        assert_eq!(
            ModelUsage::try_new(5, 0, 7, 8, 12).unwrap_err().kind(),
            ModelContractErrorKind::InvalidUsage
        );
        assert_eq!(
            ModelUsage::try_new(5, 0, 7, 0, 11).unwrap_err().kind(),
            ModelContractErrorKind::InvalidUsage
        );
        assert!(ModelUsage::try_new(i64::MAX as u64, 0, 0, 0, i64::MAX as u64).is_ok());
        assert_eq!(
            ModelUsage::try_new(i64::MAX as u64 + 1, 0, 0, 0, i64::MAX as u64 + 1)
                .unwrap_err()
                .kind(),
            ModelContractErrorKind::InvalidUsage
        );
    }

    #[test]
    fn provider_ids_metadata_and_opaque_evidence_are_bounded_and_hashed() {
        let id = ProviderEvidenceId::try_new("req-1").unwrap();
        assert_eq!(id.as_str(), "req-1");
        assert!(ProviderEvidenceId::try_new("x".repeat(129)).is_err());
        let opaque = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "continuation-v1",
            "secret-free-opaque-fixture",
        )
        .unwrap();
        assert_eq!(
            opaque.sha256(),
            Sha256Digest::hash_bytes(opaque.opaque().as_bytes())
        );
        let metadata = ProviderMetadata::try_new([(
            "cache_hit".to_owned(),
            ProviderMetadataValue::Boolean(true),
        )])
        .unwrap();
        assert_eq!(metadata.entries().len(), 1);
    }

    #[test]
    fn stream_order_semantic_observation_and_terminal_state_are_explicit() {
        let started = ModelStreamEvent::ResponseStarted {
            target: target().identity(),
            provider_request_id: None,
            provider_response_id: None,
        };
        assert_eq!(
            validate_model_stream(std::slice::from_ref(&started)).unwrap(),
            ModelStreamState::NoSemanticOutput
        );
        let events = vec![
            started.clone(),
            ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text("a"),
            },
        ];
        assert_eq!(
            validate_model_stream(&events).unwrap(),
            ModelStreamState::SemanticOutputObserved
        );
        let completed = vec![
            started,
            ModelStreamEvent::Usage(usage()),
            ModelStreamEvent::Completed(
                response(vec![ModelOutputItem::text(vec![text("a")]).unwrap()]).unwrap(),
            ),
        ];
        assert_eq!(
            validate_model_stream(&completed).unwrap(),
            ModelStreamState::Completed
        );
    }

    #[test]
    fn invalid_stream_start_duplicate_start_and_post_terminal_event_fail_closed() {
        let delta = ModelStreamEvent::TextDelta {
            item_ordinal: 0,
            delta: text("a"),
        };
        assert_eq!(
            validate_model_stream(&[delta]).unwrap_err().kind(),
            ModelContractErrorKind::InvalidStreamOrdering
        );
        let started = ModelStreamEvent::ResponseStarted {
            target: target().identity(),
            provider_request_id: None,
            provider_response_id: None,
        };
        assert_eq!(
            validate_model_stream(&[started.clone(), started.clone()])
                .unwrap_err()
                .kind(),
            ModelContractErrorKind::InvalidStreamOrdering
        );
        assert_eq!(
            validate_model_stream(&[
                started,
                ModelStreamEvent::ProviderError {
                    kind: ModelStreamProviderErrorKind::TransientUnavailable,
                },
                ModelStreamEvent::Usage(usage())
            ])
            .unwrap_err()
            .kind(),
            ModelContractErrorKind::InvalidStreamOrdering
        );
    }

    #[test]
    fn stream_requires_exactly_one_usage_result_before_exactly_one_terminal() {
        let started = ModelStreamEvent::ResponseStarted {
            target: target().identity(),
            provider_request_id: None,
            provider_response_id: None,
        };
        let completed = || {
            ModelStreamEvent::Completed(
                response(vec![ModelOutputItem::text(vec![text("done")]).unwrap()]).unwrap(),
            )
        };
        let invalid = [
            vec![started.clone(), completed()],
            vec![
                started.clone(),
                ModelStreamEvent::Usage(usage()),
                ModelStreamEvent::Usage(usage()),
                completed(),
            ],
            vec![
                started.clone(),
                ModelStreamEvent::UsageUnavailable,
                completed(),
            ],
            vec![
                started.clone(),
                ModelStreamEvent::Usage(usage()),
                ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text("late"),
                },
                completed(),
            ],
            vec![
                started.clone(),
                ModelStreamEvent::ProviderError {
                    kind: ModelStreamProviderErrorKind::DefiniteFailure,
                },
            ],
        ];
        for events in invalid {
            assert_eq!(
                validate_model_stream(&events).unwrap_err().kind(),
                ModelContractErrorKind::InvalidStreamOrdering
            );
        }
        assert_eq!(
            validate_model_stream(&[started, ModelStreamEvent::Usage(usage()), completed(),])
                .unwrap(),
            ModelStreamState::Completed
        );
    }

    #[test]
    fn provider_error_stream_terminals_preserve_canonical_certainty() {
        let cases = [
            (
                ModelStreamProviderErrorKind::DefiniteFailure,
                ModelStreamState::DefiniteProviderError,
            ),
            (
                ModelStreamProviderErrorKind::TransientUnavailable,
                ModelStreamState::DefiniteProviderError,
            ),
            (
                ModelStreamProviderErrorKind::Cancelled,
                ModelStreamState::Cancelled,
            ),
            (
                ModelStreamProviderErrorKind::OutcomeUnknown,
                ModelStreamState::AmbiguousTransportLoss,
            ),
            (
                ModelStreamProviderErrorKind::TimeoutBeforeOutput,
                ModelStreamState::TimeoutBeforeOutput,
            ),
            (
                ModelStreamProviderErrorKind::ProtocolFailure,
                ModelStreamState::ProtocolFailure,
            ),
        ];
        for (kind, expected) in cases {
            let events = [
                ModelStreamEvent::ResponseStarted {
                    target: target().identity(),
                    provider_request_id: None,
                    provider_response_id: None,
                },
                ModelStreamEvent::UsageUnavailable,
                ModelStreamEvent::ProviderError { kind },
            ];
            assert_eq!(
                validate_model_stream(&events).unwrap(),
                expected,
                "{kind:?}"
            );
        }

        let after_output = [
            ModelStreamEvent::ResponseStarted {
                target: target().identity(),
                provider_request_id: None,
                provider_response_id: None,
            },
            ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text("partial"),
            },
            ModelStreamEvent::UsageUnavailable,
            ModelStreamEvent::ProviderError {
                kind: ModelStreamProviderErrorKind::TimeoutAfterOutput,
            },
        ];
        assert_eq!(
            validate_model_stream(&after_output).unwrap(),
            ModelStreamState::TimeoutAfterOutput
        );
        for contradictory in [
            vec![
                ModelStreamEvent::ResponseStarted {
                    target: target().identity(),
                    provider_request_id: None,
                    provider_response_id: None,
                },
                ModelStreamEvent::UsageUnavailable,
                ModelStreamEvent::ProviderError {
                    kind: ModelStreamProviderErrorKind::TimeoutAfterOutput,
                },
            ],
            vec![
                ModelStreamEvent::ResponseStarted {
                    target: target().identity(),
                    provider_request_id: None,
                    provider_response_id: None,
                },
                ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text("partial"),
                },
                ModelStreamEvent::UsageUnavailable,
                ModelStreamEvent::ProviderError {
                    kind: ModelStreamProviderErrorKind::TimeoutBeforeOutput,
                },
            ],
        ] {
            assert_eq!(
                validate_model_stream(&contradictory).unwrap_err().kind(),
                ModelContractErrorKind::InvalidStreamOrdering
            );
        }
    }

    #[test]
    fn stream_tool_argument_accumulator_enforces_start_identity_exact_bytes_and_limit() {
        let started = ModelStreamEvent::ResponseStarted {
            target: target().identity(),
            provider_request_id: None,
            provider_response_id: None,
        };
        let call_id = ModelToolCallId::try_new("call-1").unwrap();
        let complete =
            CanonicalModelToolCall::try_new(call_id.clone(), "read_file", "{\"path\":\"a\"}")
                .unwrap();
        let events = vec![
            started.clone(),
            ModelStreamEvent::ToolCallStarted {
                item_ordinal: 0,
                call_id: call_id.clone(),
                name: ToolName::try_new("read_file").unwrap(),
            },
            ModelStreamEvent::tool_argument_delta(0, call_id.clone(), "{\"path\":").unwrap(),
            ModelStreamEvent::tool_argument_delta(0, call_id.clone(), "\"a\"}").unwrap(),
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 0,
                call: complete.clone(),
            },
        ];
        assert_eq!(
            validate_model_stream(&events).unwrap(),
            ModelStreamState::SemanticOutputObserved
        );
        let mismatch = vec![
            started,
            ModelStreamEvent::ToolCallStarted {
                item_ordinal: 0,
                call_id: call_id.clone(),
                name: ToolName::try_new("read_file").unwrap(),
            },
            ModelStreamEvent::tool_argument_delta(0, call_id, "{}").unwrap(),
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 0,
                call: complete,
            },
        ];
        assert_eq!(
            validate_model_stream(&mismatch).unwrap_err().kind(),
            ModelContractErrorKind::InvalidStreamOrdering
        );
    }

    #[test]
    fn debug_redacts_text_tool_arguments_and_opaque_payloads() {
        let text_canary = "TEXT_CANARY";
        let args_canary = "ARGUMENT_CANARY";
        let opaque_canary = "OPAQUE_CANARY";
        let text_debug = format!("{:?}", text(text_canary));
        let call_debug = format!(
            "{:?}",
            CanonicalModelToolCall::try_new(
                ModelToolCallId::try_new("call").unwrap(),
                "read_file",
                args_canary
            )
            .unwrap()
        );
        let opaque_debug = format!(
            "{:?}",
            ProviderOpaqueEvidence::try_new(
                ProviderId::try_new("fixture").unwrap(),
                "v1",
                opaque_canary
            )
            .unwrap()
        );
        let structured_canary = "STRUCTURED_CANARY";
        let request_debug = format!(
            "{:?}",
            request_with(
                vec![ModelInputItem::structured_data(json!({"value": structured_canary})).unwrap()],
                vec![text(text_canary)],
            )
        );
        let response_debug = format!(
            "{:?}",
            response(vec![
                ModelOutputItem::structured_data(json!({"value": structured_canary})).unwrap()
            ])
            .unwrap()
        );
        let stream_debug = format!(
            "{:?}",
            ModelStreamEvent::ToolArgumentDelta {
                item_ordinal: 0,
                call_id: ModelToolCallId::try_new("call").unwrap(),
                delta: args_canary.to_owned(),
            }
        );
        assert!(!text_debug.contains(text_canary));
        assert!(!call_debug.contains(args_canary));
        assert!(!opaque_debug.contains(opaque_canary));
        assert!(!request_debug.contains(text_canary));
        assert!(!request_debug.contains(structured_canary));
        assert!(!response_debug.contains(structured_canary));
        assert!(!stream_debug.contains(args_canary));
    }

    #[test]
    fn required_capabilities_cover_every_canonical_flag_and_output_budget() {
        let available = target().reference().capabilities().clone();
        let base = RequiredModelCapabilities {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            required_output_tokens: TokenCount::try_new(16_384).unwrap(),
        };
        assert!(base.satisfied_by(&available));
        let too_large = RequiredModelCapabilities {
            required_output_tokens: TokenCount::try_new(16_385).unwrap(),
            ..base
        };
        assert!(!too_large.satisfied_by(&available));
        assert!(
            !String::from_utf8(canonical_json_bytes(&base.semantic_value()))
                .unwrap()
                .contains("context_window_tokens")
        );
    }
}
