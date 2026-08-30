use std::collections::HashSet;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::domain::{
    ArtifactId, AuthorityDecision, AuthorityDecisionSnapshot, ConversationId, JournalEventId,
    JournalOffset, ModelCapabilitySnapshot, ModelCapabilitySnapshotInput, NormalizedError,
    ProviderId, ProviderModelReference, Sha256Digest, TokenCount, ToolName, ToolResultClass,
};
use crate::ports::state_store::{
    ContextTransformKind, ModelUsage, NormalizedModelOutput, NormalizedModelOutputItem,
    PreparedContextManifest, ProviderOption, ProviderOptionValue, RequiredModelCapabilities,
    ToolOutputPolicy, ToolResultEvidence,
};

use super::codec::{
    PersistedNormalizedError, decode_attempt_normalized_error, encode_attempt_normalized_error,
};
use super::error::{SqliteAdapterError, SqliteFailureKind};

const MAX_CAPABILITIES_BYTES: usize = 16_384;
const MAX_CONTEXT_JSON_BYTES: usize = 65_536;
const MAX_OUTPUT_JSON_BYTES: usize = 262_144;

fn invalid() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InternalInvariant)
}

fn corrupt() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

fn to_bounded_json<T: Serialize>(value: &T, maximum: usize) -> Result<String, SqliteAdapterError> {
    let json = serde_json::to_string(value).map_err(|_| invalid())?;
    if json.len() <= maximum {
        Ok(json)
    } else {
        Err(invalid())
    }
}

fn from_bounded_json<T: DeserializeOwned + Serialize>(
    json: &str,
    maximum: usize,
) -> Result<T, SqliteAdapterError> {
    if json.len() > maximum {
        return Err(corrupt());
    }
    let value: T = serde_json::from_str(json).map_err(|_| corrupt())?;
    if serde_json::to_string(&value).map_err(|_| corrupt())? == json {
        Ok(value)
    } else {
        Err(corrupt())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredModelCapabilitiesV1 {
    version: u8,
    text_input: bool,
    text_output: bool,
    custom_tool_calling: bool,
    streaming: bool,
    ordered_output_items: bool,
    structured_output: bool,
    reasoning_continuation: bool,
    context_window_tokens: i64,
    max_output_tokens: i64,
}

pub(super) fn encode_model_capabilities(
    model: &ProviderModelReference,
) -> Result<String, SqliteAdapterError> {
    let capabilities = model.capabilities();
    if capabilities.max_output_tokens() > capabilities.context_window_tokens() {
        return Err(invalid());
    }
    to_bounded_json(
        &StoredModelCapabilitiesV1 {
            version: 1,
            text_input: capabilities.text_input(),
            text_output: capabilities.text_output(),
            custom_tool_calling: capabilities.custom_tool_calling(),
            streaming: capabilities.streaming(),
            ordered_output_items: capabilities.ordered_output_items(),
            structured_output: capabilities.structured_output(),
            reasoning_continuation: capabilities.reasoning_continuation(),
            context_window_tokens: capabilities.context_window_tokens().get(),
            max_output_tokens: capabilities.max_output_tokens().get(),
        },
        MAX_CAPABILITIES_BYTES,
    )
}

pub(super) fn decode_model_capabilities(
    json: &str,
) -> Result<ModelCapabilitySnapshot, SqliteAdapterError> {
    let value: StoredModelCapabilitiesV1 = from_bounded_json(json, MAX_CAPABILITIES_BYTES)?;
    if value.version != 1 {
        return Err(corrupt());
    }
    let context_window_tokens =
        TokenCount::try_new(value.context_window_tokens).map_err(|_| corrupt())?;
    let max_output_tokens = TokenCount::try_new(value.max_output_tokens).map_err(|_| corrupt())?;
    if max_output_tokens > context_window_tokens {
        return Err(corrupt());
    }
    Ok(ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
        text_input: value.text_input,
        text_output: value.text_output,
        custom_tool_calling: value.custom_tool_calling,
        streaming: value.streaming,
        ordered_output_items: value.ordered_output_items,
        structured_output: value.structured_output,
        reasoning_continuation: value.reasoning_continuation,
        context_window_tokens,
        max_output_tokens,
    }))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEligibilityCutoffV1 {
    version: u8,
    conversation_id: String,
    active_work_ordinal: i64,
    highest_prior_terminal_work_ordinal: Option<i64>,
    input_event_ids: Vec<String>,
    active_output_record_ids: Vec<String>,
    maximum_journal_offset: i64,
}

pub(super) fn encode_eligibility_cutoff(
    manifest: &PreparedContextManifest,
) -> Result<String, SqliteAdapterError> {
    let mut input_event_ids = HashSet::with_capacity(manifest.input_event_ids.len());
    let mut active_output_record_ids =
        HashSet::with_capacity(manifest.active_output_record_ids.len());
    if manifest.active_work_ordinal <= 0
        || manifest
            .highest_prior_terminal_work_ordinal
            .is_some_and(|value| value <= 0 || value >= manifest.active_work_ordinal)
        || manifest
            .input_event_ids
            .iter()
            .any(|value| !input_event_ids.insert(*value))
        || manifest.active_output_record_ids.iter().any(|value| {
            value.is_empty()
                || value.len() > 255
                || value.trim() != value
                || value.chars().any(char::is_control)
                || !active_output_record_ids.insert(value.as_str())
        })
    {
        return Err(invalid());
    }
    to_bounded_json(
        &StoredEligibilityCutoffV1 {
            version: 1,
            conversation_id: manifest.eligibility_conversation_id.to_string(),
            active_work_ordinal: manifest.active_work_ordinal,
            highest_prior_terminal_work_ordinal: manifest.highest_prior_terminal_work_ordinal,
            input_event_ids: manifest
                .input_event_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            active_output_record_ids: manifest.active_output_record_ids.clone(),
            maximum_journal_offset: manifest.maximum_journal_offset.get(),
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredOmissionsV1 {
    version: u8,
    omitted_source_count: u64,
    transformed_source_count: u64,
}

pub(super) fn encode_omissions(
    manifest: &PreparedContextManifest,
) -> Result<String, SqliteAdapterError> {
    to_bounded_json(
        &StoredOmissionsV1 {
            version: 1,
            omitted_source_count: manifest.omitted_source_count,
            transformed_source_count: manifest.transformed_source_count,
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTransformV1 {
    version: u8,
    kind: String,
    transformed: bool,
}

pub(super) fn encode_transform(
    kind: ContextTransformKind,
    transformed: bool,
) -> Result<String, SqliteAdapterError> {
    let kind = match kind {
        ContextTransformKind::Identity => "identity",
        ContextTransformKind::InlineProjection => "inline_projection",
        ContextTransformKind::SyntheticStatus => "synthetic_status",
        ContextTransformKind::ProviderContinuation => "provider_continuation",
    };
    if transformed != (kind != "identity") {
        return Err(invalid());
    }
    to_bounded_json(
        &StoredTransformV1 {
            version: 1,
            kind: kind.to_owned(),
            transformed,
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRequiredCapabilitiesV1 {
    version: u8,
    text_input: bool,
    text_output: bool,
    custom_tool_calling: bool,
    streaming: bool,
    ordered_output_items: bool,
    structured_output: bool,
    reasoning_continuation: bool,
}

pub(super) fn encode_required_capabilities(
    value: RequiredModelCapabilities,
) -> Result<String, SqliteAdapterError> {
    to_bounded_json(
        &StoredRequiredCapabilitiesV1 {
            version: 1,
            text_input: value.text_input,
            text_output: value.text_output,
            custom_tool_calling: value.custom_tool_calling,
            streaming: value.streaming,
            ordered_output_items: value.ordered_output_items,
            structured_output: value.structured_output,
            reasoning_continuation: value.reasoning_continuation,
        },
        MAX_CAPABILITIES_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "value",
    rename_all = "snake_case"
)]
enum StoredProviderOptionValueV1 {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProviderOptionV1 {
    key: String,
    value: StoredProviderOptionValueV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProviderOptionsV1 {
    version: u8,
    options: Vec<StoredProviderOptionV1>,
}

pub(super) fn encode_provider_options(
    values: &[ProviderOption],
) -> Result<String, SqliteAdapterError> {
    let mut prior: Option<&str> = None;
    let mut options = Vec::with_capacity(values.len());
    for option in values {
        if !valid_provider_option_key(&option.key)
            || prior.is_some_and(|value| value >= option.key.as_str())
        {
            return Err(invalid());
        }
        prior = Some(&option.key);
        let value = match &option.value {
            ProviderOptionValue::Boolean(value) => StoredProviderOptionValueV1::Boolean(*value),
            ProviderOptionValue::Integer(value) => StoredProviderOptionValueV1::Integer(*value),
            ProviderOptionValue::Text(value)
                if !value.is_empty()
                    && value.len() <= 1_024
                    && !value.chars().any(char::is_control) =>
            {
                StoredProviderOptionValueV1::Text(value.clone())
            }
            ProviderOptionValue::Text(_) => return Err(invalid()),
        };
        options.push(StoredProviderOptionV1 {
            key: option.key.clone(),
            value,
        });
    }
    to_bounded_json(
        &StoredProviderOptionsV1 {
            version: 1,
            options,
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum StoredNormalizedOutputItemV1 {
    Text {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
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
        provider_id: String,
        item_type: String,
        sha256: Sha256Digest,
        artifact_id: String,
    },
    UnknownProviderItem {
        item_type: String,
        sha256: Sha256Digest,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredNormalizedOutputV1 {
    version: u8,
    items: Vec<StoredNormalizedOutputItemV1>,
}

pub(super) fn encode_normalized_output(
    output: &NormalizedModelOutput,
) -> Result<String, SqliteAdapterError> {
    if output.items.len() > 1_024 {
        return Err(invalid());
    }
    let mut items = Vec::with_capacity(output.items.len());
    for item in &output.items {
        let stored = match item {
            NormalizedModelOutputItem::Text { text } => StoredNormalizedOutputItemV1::Text {
                text: bounded_text(text, 65_536)?.to_owned(),
            },
            NormalizedModelOutputItem::ToolCall {
                call_id,
                tool_name,
                arguments_json,
            } => {
                validate_identifier(call_id, 255)?;
                validate_canonical_json(arguments_json)?;
                StoredNormalizedOutputItemV1::ToolCall {
                    call_id: call_id.clone(),
                    tool_name: tool_name.as_str().to_owned(),
                    arguments_json: arguments_json.clone(),
                }
            }
            NormalizedModelOutputItem::StructuredData { canonical_json } => {
                validate_canonical_json(canonical_json)?;
                StoredNormalizedOutputItemV1::StructuredData {
                    canonical_json: canonical_json.clone(),
                }
            }
            NormalizedModelOutputItem::Refusal { text } => StoredNormalizedOutputItemV1::Refusal {
                text: bounded_text(text, 65_536)?.to_owned(),
            },
            NormalizedModelOutputItem::ReasoningSummary { text } => {
                StoredNormalizedOutputItemV1::ReasoningSummary {
                    text: bounded_text(text, 65_536)?.to_owned(),
                }
            }
            NormalizedModelOutputItem::ProviderOpaque {
                provider_id,
                item_type,
                sha256,
                artifact_id,
            } => {
                validate_identifier(item_type, 128)?;
                StoredNormalizedOutputItemV1::ProviderOpaque {
                    provider_id: provider_id.as_str().to_owned(),
                    item_type: item_type.clone(),
                    sha256: *sha256,
                    artifact_id: artifact_id.to_string(),
                }
            }
            NormalizedModelOutputItem::UnknownProviderItem { item_type, sha256 } => {
                validate_identifier(item_type, 128)?;
                StoredNormalizedOutputItemV1::UnknownProviderItem {
                    item_type: item_type.clone(),
                    sha256: *sha256,
                }
            }
        };
        items.push(stored);
    }
    to_bounded_json(
        &StoredNormalizedOutputV1 { version: 1, items },
        MAX_OUTPUT_JSON_BYTES,
    )
}

pub(super) fn validate_normalized_output(
    json: &str,
) -> Result<NormalizedModelOutput, SqliteAdapterError> {
    let value: StoredNormalizedOutputV1 = from_bounded_json(json, MAX_OUTPUT_JSON_BYTES)?;
    if value.version != 1 || value.items.len() > 1_024 {
        return Err(corrupt());
    }
    let mut items = Vec::with_capacity(value.items.len());
    for item in value.items {
        items.push(match item {
            StoredNormalizedOutputItemV1::Text { text } => NormalizedModelOutputItem::Text {
                text: bounded_text(&text, 65_536)
                    .map_err(|_| corrupt())?
                    .to_owned(),
            },
            StoredNormalizedOutputItemV1::ToolCall {
                call_id,
                tool_name,
                arguments_json,
            } => {
                validate_identifier(&call_id, 255).map_err(|_| corrupt())?;
                let tool_name = ToolName::try_new(tool_name).map_err(|_| corrupt())?;
                validate_canonical_json(&arguments_json).map_err(|_| corrupt())?;
                NormalizedModelOutputItem::ToolCall {
                    call_id,
                    tool_name,
                    arguments_json,
                }
            }
            StoredNormalizedOutputItemV1::StructuredData { canonical_json } => {
                validate_canonical_json(&canonical_json).map_err(|_| corrupt())?;
                NormalizedModelOutputItem::StructuredData { canonical_json }
            }
            StoredNormalizedOutputItemV1::Refusal { text } => NormalizedModelOutputItem::Refusal {
                text: bounded_text(&text, 65_536)
                    .map_err(|_| corrupt())?
                    .to_owned(),
            },
            StoredNormalizedOutputItemV1::ReasoningSummary { text } => {
                NormalizedModelOutputItem::ReasoningSummary {
                    text: bounded_text(&text, 65_536)
                        .map_err(|_| corrupt())?
                        .to_owned(),
                }
            }
            StoredNormalizedOutputItemV1::ProviderOpaque {
                provider_id,
                item_type,
                sha256,
                artifact_id,
            } => {
                let provider_id = ProviderId::try_new(provider_id).map_err(|_| corrupt())?;
                validate_identifier(&item_type, 128).map_err(|_| corrupt())?;
                let artifact_id =
                    ArtifactId::parse_canonical(&artifact_id).map_err(|_| corrupt())?;
                NormalizedModelOutputItem::ProviderOpaque {
                    provider_id,
                    item_type,
                    sha256,
                    artifact_id,
                }
            }
            StoredNormalizedOutputItemV1::UnknownProviderItem { item_type, sha256 } => {
                validate_identifier(&item_type, 128).map_err(|_| corrupt())?;
                NormalizedModelOutputItem::UnknownProviderItem { item_type, sha256 }
            }
        });
    }
    Ok(NormalizedModelOutput { items })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredOutputPolicyV1 {
    version: u8,
    stdout_capture_limit: u64,
    stderr_capture_limit: u64,
    combined_inline_limit: u64,
    per_stream_inline_limit: u64,
}

pub(super) fn encode_output_policy(value: ToolOutputPolicy) -> Result<String, SqliteAdapterError> {
    if value.per_stream_inline_limit > value.combined_inline_limit
        || value.per_stream_inline_limit
            > value.stdout_capture_limit.max(value.stderr_capture_limit)
    {
        return Err(invalid());
    }
    to_bounded_json(
        &StoredOutputPolicyV1 {
            version: 1,
            stdout_capture_limit: value.stdout_capture_limit.get(),
            stderr_capture_limit: value.stderr_capture_limit.get(),
            combined_inline_limit: value.combined_inline_limit.get(),
            per_stream_inline_limit: value.per_stream_inline_limit.get(),
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityDecisionV1 {
    version: u8,
    decision: String,
    effective_privilege: String,
    policy: String,
    reason_code: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityFactsV1 {
    administrative_allowed: bool,
    authority_widening_attempt: bool,
    malformed_arguments: bool,
    requested_stderr_bytes: u64,
    requested_stdout_bytes: u64,
    requested_timeout_ms: Option<u64>,
    tool_allowed: bool,
    work_cancelled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityCapabilitiesV1 {
    cancel_execution: bool,
    filesystem_read: bool,
    foreground_execute: bool,
    inspect_execution: bool,
    max_execution_timeout_ms: u64,
    max_stderr_bytes: u64,
    max_stdout_bytes: u64,
    privilege_administrative: bool,
    privilege_user: bool,
    workspace_present: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPreparedCwdEvidenceV1 {
    device: u64,
    inode: u64,
    object_type: String,
    requested_cwd: String,
    resolved_cwd: String,
    version: u8,
    workspace_id: String,
    workstation_generation: i64,
    workstation_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityEvidenceV1 {
    arguments_sha256: String,
    authority_facts: StoredAuthorityFactsV1,
    canonical_argument_bytes: usize,
    capabilities: StoredAuthorityCapabilitiesV1,
    craxii_id: String,
    decision: String,
    effective_privilege: String,
    policy: String,
    prepared_cwd: Option<StoredPreparedCwdEvidenceV1>,
    reason_code: String,
    requested_privilege: String,
    required_capability: Option<String>,
    runtime_instance_id: String,
    schema_version: Option<i64>,
    tool_name: String,
    tool_version: Option<String>,
    version: u8,
    work_id: String,
    workspace_id: String,
    workstation_generation: i64,
    workstation_id: String,
}

pub(super) fn encode_authority(
    value: &AuthorityDecisionSnapshot,
) -> Result<String, SqliteAdapterError> {
    let decision = match value.decision() {
        AuthorityDecision::Allow => "allow",
        AuthorityDecision::Deny => "deny",
    };
    let effective_privilege = match value.effective_privilege() {
        crate::domain::PrivilegeMode::User => "user",
        crate::domain::PrivilegeMode::Administrative => "administrative",
    };
    if (value.decision() == AuthorityDecision::Deny
        && value.effective_privilege() != crate::domain::PrivilegeMode::User)
        || !valid_authority_reason_code(value.reason_code().as_str())
    {
        return Err(invalid());
    }
    to_bounded_json(
        &StoredAuthorityDecisionV1 {
            version: 1,
            decision: decision.to_owned(),
            effective_privilege: effective_privilege.to_owned(),
            policy: value.policy_version().as_str().to_owned(),
            reason_code: value.reason_code().as_str().to_owned(),
        },
        MAX_CONTEXT_JSON_BYTES,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToolResultV1 {
    version: u8,
    result_kind: String,
    summary: String,
    fields: Vec<(String, String)>,
}

pub(super) fn encode_tool_result(value: &ToolResultEvidence) -> Result<String, SqliteAdapterError> {
    let mut prior: Option<&str> = None;
    for (key, value) in &value.fields {
        if !valid_tool_result_field_key(key) {
            return Err(invalid());
        }
        bounded_text(value, 4_096)?;
        if prior.is_some_and(|previous| previous >= key.as_str()) {
            return Err(invalid());
        }
        prior = Some(key);
    }
    to_bounded_json(
        &StoredToolResultV1 {
            version: 1,
            result_kind: value.result_kind.as_str().to_owned(),
            summary: bounded_text(&value.summary, 8_192)?.to_owned(),
            fields: value.fields.clone(),
        },
        MAX_OUTPUT_JSON_BYTES,
    )
}

pub(super) fn encode_model_usage(value: ModelUsage) -> Result<[i64; 5], SqliteAdapterError> {
    let values = [
        value.input_tokens,
        value.cached_input_tokens,
        value.output_tokens,
        value.reasoning_tokens,
        value.total_tokens,
    ];
    if value.cached_input_tokens > value.input_tokens
        || value.reasoning_tokens > value.output_tokens
        || value.total_tokens != value.input_tokens.saturating_add(value.output_tokens)
        || values.iter().any(|value| *value > i64::MAX as u64)
    {
        return Err(invalid());
    }
    Ok(values.map(|value| value as i64))
}

pub(super) fn encode_attempt_error(
    value: &NormalizedError,
    allow_outcome_unknown: bool,
) -> Result<String, SqliteAdapterError> {
    encode_attempt_normalized_error(value, allow_outcome_unknown)
}

pub(super) fn validate_attempt_error(
    json: &str,
    allow_outcome_unknown: bool,
) -> Result<PersistedNormalizedError, SqliteAdapterError> {
    decode_attempt_normalized_error(json, allow_outcome_unknown)
}

pub(super) struct DecodedEligibilityCutoffV1 {
    pub conversation_id: ConversationId,
    pub active_work_ordinal: i64,
    pub highest_prior_terminal_work_ordinal: Option<i64>,
    pub input_event_ids: Vec<JournalEventId>,
    pub maximum_journal_offset: JournalOffset,
}

pub(super) fn validate_eligibility_cutoff(
    json: &str,
) -> Result<DecodedEligibilityCutoffV1, SqliteAdapterError> {
    let value: StoredEligibilityCutoffV1 = from_bounded_json(json, MAX_CONTEXT_JSON_BYTES)?;
    if value.version != 1
        || value.active_work_ordinal <= 0
        || value.maximum_journal_offset <= 0
        || value
            .highest_prior_terminal_work_ordinal
            .is_some_and(|ordinal| ordinal <= 0 || ordinal >= value.active_work_ordinal)
        || value.active_output_record_ids.iter().any(|id| {
            id.is_empty() || id.len() > 255 || id.trim() != id || id.chars().any(char::is_control)
        })
    {
        return Err(corrupt());
    }
    let conversation_id =
        ConversationId::parse_canonical(&value.conversation_id).map_err(|_| corrupt())?;
    let maximum_journal_offset =
        JournalOffset::try_new(value.maximum_journal_offset).map_err(|_| corrupt())?;
    let mut seen_inputs = HashSet::with_capacity(value.input_event_ids.len());
    let mut input_event_ids = Vec::with_capacity(value.input_event_ids.len());
    for input in value.input_event_ids {
        let id = JournalEventId::parse_canonical(&input).map_err(|_| corrupt())?;
        if !seen_inputs.insert(id) {
            return Err(corrupt());
        }
        input_event_ids.push(id);
    }
    let mut seen_outputs = HashSet::with_capacity(value.active_output_record_ids.len());
    if value
        .active_output_record_ids
        .iter()
        .any(|id| !seen_outputs.insert(id.as_str()))
    {
        return Err(corrupt());
    }
    Ok(DecodedEligibilityCutoffV1 {
        conversation_id,
        active_work_ordinal: value.active_work_ordinal,
        highest_prior_terminal_work_ordinal: value.highest_prior_terminal_work_ordinal,
        input_event_ids,
        maximum_journal_offset,
    })
}

pub(super) fn validate_omissions(json: &str) -> Result<(), SqliteAdapterError> {
    let value: StoredOmissionsV1 = from_bounded_json(json, MAX_CONTEXT_JSON_BYTES)?;
    if value.version == 1 {
        Ok(())
    } else {
        Err(corrupt())
    }
}

pub(super) fn validate_transform(json: &str) -> Result<(), SqliteAdapterError> {
    let value: StoredTransformV1 = from_bounded_json(json, MAX_CONTEXT_JSON_BYTES)?;
    if value.version == 1
        && matches!(
            value.kind.as_str(),
            "identity" | "inline_projection" | "synthetic_status" | "provider_continuation"
        )
        && value.transformed == (value.kind != "identity")
    {
        Ok(())
    } else {
        Err(corrupt())
    }
}

pub(super) fn validate_required_capabilities(json: &str) -> Result<(), SqliteAdapterError> {
    let value: StoredRequiredCapabilitiesV1 = from_bounded_json(json, MAX_CAPABILITIES_BYTES)?;
    if value.version == 1 {
        Ok(())
    } else {
        Err(corrupt())
    }
}

pub(super) fn validate_provider_options(json: &str) -> Result<(), SqliteAdapterError> {
    let value: StoredProviderOptionsV1 = from_bounded_json(json, MAX_CONTEXT_JSON_BYTES)?;
    let mut prior: Option<&str> = None;
    for option in &value.options {
        if option.key.is_empty()
            || option.key.len() > 64
            || !valid_provider_option_key(&option.key)
            || prior.is_some_and(|key| key >= option.key.as_str())
            || matches!(&option.value, StoredProviderOptionValueV1::Text(text) if text.is_empty() || text.len() > 1_024 || text.chars().any(char::is_control))
        {
            return Err(corrupt());
        }
        prior = Some(&option.key);
    }
    if value.version == 1 {
        Ok(())
    } else {
        Err(corrupt())
    }
}

fn valid_provider_option_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase() || byte.is_ascii_digit()
            } else {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            }
        })
}

pub(super) fn validate_output_policy(json: &str) -> Result<(), SqliteAdapterError> {
    let value: StoredOutputPolicyV1 = from_bounded_json(json, MAX_CONTEXT_JSON_BYTES)?;
    if value.version == 1
        && value.per_stream_inline_limit <= value.combined_inline_limit
        && value.per_stream_inline_limit
            <= value.stdout_capture_limit.max(value.stderr_capture_limit)
        && [
            value.stdout_capture_limit,
            value.stderr_capture_limit,
            value.combined_inline_limit,
            value.per_stream_inline_limit,
        ]
        .iter()
        .all(|count| *count <= i64::MAX as u64)
    {
        Ok(())
    } else {
        Err(corrupt())
    }
}

pub(super) fn validate_authority(
    json: &str,
    require_allow: bool,
) -> Result<(), SqliteAdapterError> {
    if let Ok(value) = from_bounded_json::<StoredAuthorityDecisionV1>(json, MAX_CAPABILITIES_BYTES)
    {
        return if value.version == 1
            && matches!(value.decision.as_str(), "allow" | "deny")
            && (!require_allow || value.decision == "allow")
            && matches!(
                value.effective_privilege.as_str(),
                "user" | "administrative"
            )
            && (value.decision != "deny" || value.effective_privilege == "user")
            && value.policy == "v0-development-workstation"
            && valid_authority_reason_code(&value.reason_code)
        {
            Ok(())
        } else {
            Err(corrupt())
        };
    }
    let value: StoredAuthorityEvidenceV1 = from_bounded_json(json, MAX_CAPABILITIES_BYTES)?;
    if (!require_allow || value.decision == "allow") && valid_full_authority_evidence(&value) {
        Ok(())
    } else {
        Err(corrupt())
    }
}

fn valid_authority_reason_code(value: &str) -> bool {
    matches!(
        value,
        "allowed"
            | "registered_tool"
            | "policy_denied"
            | "malformed_request"
            | "work_cancelled"
            | "scope_denied"
            | "unregistered_tool"
            | "malformed_arguments"
            | "cancelled_work"
            | "authority_widening"
            | "explicit_constraint_denial"
            | "wrong_workstation"
            | "stale_generation"
            | "wrong_workspace"
            | "unsupported_capability"
            | "administrative_unavailable"
            | "limit_exceeded"
    )
}

pub(super) fn validate_authority_evidence(
    json: &str,
    expected: &AuthorityDecisionSnapshot,
) -> Result<(), SqliteAdapterError> {
    let value: StoredAuthorityEvidenceV1 = from_bounded_json(json, MAX_CAPABILITIES_BYTES)?;
    let canonical = serde_json::to_string(
        &serde_json::from_str::<serde_json::Value>(json).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    let expected_decision = match expected.decision() {
        AuthorityDecision::Allow => "allow",
        AuthorityDecision::Deny => "deny",
    };
    let expected_privilege = match expected.effective_privilege() {
        crate::domain::PrivilegeMode::User => "user",
        crate::domain::PrivilegeMode::Administrative => "administrative",
    };
    if canonical != json
        || !matches!(value.version, 1 | 2)
        || value.decision != expected_decision
        || value.effective_privilege != expected_privilege
        || value.policy != expected.policy_version().as_str()
        || value.reason_code != expected.reason_code().as_str()
        || !valid_full_authority_evidence(&value)
    {
        return Err(invalid());
    }
    Ok(())
}

fn valid_full_authority_evidence(value: &StoredAuthorityEvidenceV1) -> bool {
    let facts = &value.authority_facts;
    let capabilities = &value.capabilities;
    ((value.version == 1 && value.prepared_cwd.is_none())
        || (value.version == 2
            && value
                .prepared_cwd
                .as_ref()
                .is_some_and(valid_prepared_cwd_evidence)))
        && value.decision == "allow"
        && value.reason_code == "allowed"
        && matches!(
            value.effective_privilege.as_str(),
            "user" | "administrative"
        )
        && matches!(
            value.requested_privilege.as_str(),
            "user" | "administrative"
        )
        && value.policy == "v0-development-workstation"
        && valid_authority_reason_code(&value.reason_code)
        && crate::domain::Sha256Digest::parse_canonical(&value.arguments_sha256).is_ok()
        && value.canonical_argument_bytes <= 65_536
        && value.workstation_generation > 0
        && value.schema_version.is_some_and(|version| version > 0)
        && value
            .tool_version
            .as_ref()
            .is_some_and(|version| !version.is_empty())
        && value
            .required_capability
            .as_deref()
            .is_some_and(|capability| {
                matches!(capability, "filesystem_read" | "foreground_execute")
            })
        && !value.craxii_id.is_empty()
        && !value.runtime_instance_id.is_empty()
        && !value.tool_name.is_empty()
        && !value.work_id.is_empty()
        && !value.workspace_id.is_empty()
        && !value.workstation_id.is_empty()
        && facts.requested_timeout_ms.is_none_or(|timeout| timeout > 0)
        && facts.requested_stdout_bytes <= capabilities.max_stdout_bytes
        && facts.requested_stderr_bytes <= capabilities.max_stderr_bytes
        && facts
            .requested_timeout_ms
            .is_none_or(|timeout| timeout <= capabilities.max_execution_timeout_ms)
        && facts.tool_allowed
        && !facts.authority_widening_attempt
        && !facts.malformed_arguments
        && !facts.work_cancelled
        && capabilities.privilege_user
        && capabilities.workspace_present
        && (value.required_capability.as_deref() != Some("filesystem_read")
            || capabilities.filesystem_read)
        && (value.required_capability.as_deref() != Some("foreground_execute")
            || capabilities.foreground_execute)
        && (value.effective_privilege != "administrative"
            || (facts.administrative_allowed && capabilities.privilege_administrative))
}

fn valid_prepared_cwd_evidence(value: &StoredPreparedCwdEvidenceV1) -> bool {
    value.version == 1
        && value.object_type == "directory"
        && value.inode > 0
        && value.workstation_generation > 0
        && !value.workstation_id.is_empty()
        && !value.workspace_id.is_empty()
        && !value.requested_cwd.is_empty()
        && value.requested_cwd.len() <= crate::domain::MAX_LOGICAL_PATH_BYTES
        && !value.requested_cwd.contains(['\0', '\\'])
        && value.resolved_cwd.starts_with('/')
        && value.resolved_cwd.len() <= crate::domain::MAX_LOGICAL_PATH_BYTES
        && !value.resolved_cwd.contains('\0')
}

pub(super) fn validate_dispatch_evidence(
    json: &str,
    expected_authority: &AuthorityDecisionSnapshot,
    expected_cwd: &crate::ports::workstation_preparation::PreparedCwdEvidence,
) -> Result<(), SqliteAdapterError> {
    validate_authority_evidence(json, expected_authority)?;
    let value: StoredAuthorityEvidenceV1 = from_bounded_json(json, MAX_CAPABILITIES_BYTES)?;
    let prepared = value.prepared_cwd.ok_or_else(invalid)?;
    let resolved = expected_cwd.resolved_cwd();
    let identity = expected_cwd.object_identity();
    if value.version != 2
        || !valid_prepared_cwd_evidence(&prepared)
        || prepared.workstation_id != resolved.workstation_id().to_string()
        || prepared.workstation_generation != resolved.workstation_generation().get()
        || prepared.workspace_id != resolved.workspace_id().to_string()
        || prepared.requested_cwd != resolved.requested_path().canonical()
        || prepared.resolved_cwd != resolved.resolved_absolute_path()
        || prepared.device != identity.device()
        || prepared.inode != identity.inode()
        || !matches!(
            identity.object_type(),
            crate::ports::workstation_preparation::PreparedCwdObjectType::Directory
        )
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn validate_tool_result(json: &str) -> Result<ToolResultClass, SqliteAdapterError> {
    decode_tool_result(json).map(|value| value.result_class)
}

pub(super) struct DecodedToolResult {
    pub result_class: ToolResultClass,
    pub fields: Vec<(String, String)>,
}

pub(super) fn decode_tool_result(json: &str) -> Result<DecodedToolResult, SqliteAdapterError> {
    let value: StoredToolResultV1 = from_bounded_json(json, MAX_OUTPUT_JSON_BYTES)?;
    let mut prior: Option<&str> = None;
    for (key, field) in &value.fields {
        if key.is_empty()
            || key.len() > 64
            || !valid_tool_result_field_key(key)
            || field.is_empty()
            || field.len() > 4_096
            || field.contains('\0')
            || prior.is_some_and(|previous| previous >= key.as_str())
        {
            return Err(corrupt());
        }
        prior = Some(key);
    }
    if value.version == 1
        && !value.summary.is_empty()
        && value.summary.len() <= 8_192
        && !value.summary.contains('\0')
    {
        Ok(DecodedToolResult {
            result_class: decode_tool_result_class(&value.result_kind)?,
            fields: value.fields,
        })
    } else {
        Err(corrupt())
    }
}

fn valid_tool_result_field_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase() || byte.is_ascii_digit()
            } else {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            }
        })
}

fn decode_tool_result_class(value: &str) -> Result<ToolResultClass, SqliteAdapterError> {
    match value {
        "success" => Ok(ToolResultClass::Success),
        "validation_rejection" => Ok(ToolResultClass::ValidationRejection),
        "unknown_tool" => Ok(ToolResultClass::UnknownTool),
        "authority_denial" => Ok(ToolResultClass::AuthorityDenial),
        "file_error" => Ok(ToolResultClass::FileError),
        "process_exit" => Ok(ToolResultClass::ProcessExit),
        "signal_termination" => Ok(ToolResultClass::SignalTermination),
        "timeout" => Ok(ToolResultClass::Timeout),
        "cancellation" => Ok(ToolResultClass::Cancellation),
        "spawn_failure" => Ok(ToolResultClass::SpawnFailure),
        "cleanup_failure" => Ok(ToolResultClass::CleanupFailure),
        _ => Err(corrupt()),
    }
}

fn bounded_text(value: &str, maximum: usize) -> Result<&str, SqliteAdapterError> {
    if !value.is_empty() && value.len() <= maximum && !value.contains('\0') {
        Ok(value)
    } else {
        Err(invalid())
    }
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), SqliteAdapterError> {
    if !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn validate_canonical_json(value: &str) -> Result<(), SqliteAdapterError> {
    if value.len() > MAX_CONTEXT_JSON_BYTES {
        return Err(invalid());
    }
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| invalid())?;
    let encoded = serde_json::to_string(&parsed).map_err(|_| invalid())?;
    if encoded == value {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[allow(dead_code)]
fn _capability_shape_is_dependency_neutral(_: &ModelCapabilitySnapshot) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn json<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    fn valid_cutoff() -> StoredEligibilityCutoffV1 {
        StoredEligibilityCutoffV1 {
            version: 1,
            conversation_id: ConversationId::generate().to_string(),
            active_work_ordinal: 2,
            highest_prior_terminal_work_ordinal: Some(1),
            input_event_ids: vec![JournalEventId::generate().to_string()],
            active_output_record_ids: vec!["model:1".to_owned()],
            maximum_journal_offset: 9,
        }
    }

    #[test]
    fn eligibility_cutoff_decode_reconstructs_ids_and_rejects_corrupt_relationships() {
        assert!(validate_eligibility_cutoff(&json(&valid_cutoff())).is_ok());
        let mut invalid = valid_cutoff();
        invalid.conversation_id = "not-a-uuid".to_owned();
        assert!(validate_eligibility_cutoff(&json(&invalid)).is_err());
        let mut invalid = valid_cutoff();
        invalid.input_event_ids = vec!["not-a-uuid".to_owned()];
        assert!(validate_eligibility_cutoff(&json(&invalid)).is_err());
        let mut invalid = valid_cutoff();
        invalid
            .input_event_ids
            .push(invalid.input_event_ids[0].clone());
        assert!(validate_eligibility_cutoff(&json(&invalid)).is_err());
        let mut invalid = valid_cutoff();
        invalid.highest_prior_terminal_work_ordinal = Some(2);
        assert!(validate_eligibility_cutoff(&json(&invalid)).is_err());
        let mut invalid = valid_cutoff();
        invalid.active_output_record_ids.push("model:1".to_owned());
        assert!(validate_eligibility_cutoff(&json(&invalid)).is_err());
        assert!(
            validate_eligibility_cutoff(
                "{\"version\":1,\"conversation_id\":\"x\",\"active_work_ordinal\":1,\"highest_prior_terminal_work_ordinal\":null,\"input_event_ids\":[],\"active_output_record_ids\":[],\"maximum_journal_offset\":1,\"unknown\":true}"
            )
            .is_err()
        );
    }

    fn options(key: String, value: StoredProviderOptionValueV1) -> StoredProviderOptionsV1 {
        StoredProviderOptionsV1 {
            version: 1,
            options: vec![StoredProviderOptionV1 { key, value }],
        }
    }

    #[test]
    fn provider_options_decode_uses_the_encoder_key_and_value_grammar() {
        assert!(
            validate_provider_options(&json(&options(
                "temperature.v1".to_owned(),
                StoredProviderOptionValueV1::Integer(0),
            )))
            .is_ok()
        );
        for key in ["Upper", "-leading", "has/slash", "white space"] {
            assert!(
                validate_provider_options(&json(&options(
                    key.to_owned(),
                    StoredProviderOptionValueV1::Boolean(true),
                )))
                .is_err()
            );
        }
        assert!(
            validate_provider_options(&json(&options(
                "a".repeat(65),
                StoredProviderOptionValueV1::Boolean(true),
            )))
            .is_err()
        );
        assert!(
            validate_provider_options(&json(&options(
                "valid".to_owned(),
                StoredProviderOptionValueV1::Text("x".repeat(1_025)),
            )))
            .is_err()
        );
        let mut wrong_version = options(
            "valid".to_owned(),
            StoredProviderOptionValueV1::Boolean(true),
        );
        wrong_version.version = 2;
        assert!(validate_provider_options(&json(&wrong_version)).is_err());
    }

    #[test]
    fn authority_decode_uses_closed_reason_codes_and_decision_privilege_shape() {
        let valid = StoredAuthorityDecisionV1 {
            version: 1,
            decision: "allow".to_owned(),
            effective_privilege: "administrative".to_owned(),
            policy: "v0-development-workstation".to_owned(),
            reason_code: "registered_tool".to_owned(),
        };
        assert!(validate_authority(&json(&valid), true).is_ok());
        let invalid_reason = StoredAuthorityDecisionV1 {
            reason_code: "arbitrary_reason".to_owned(),
            ..valid
        };
        assert!(validate_authority(&json(&invalid_reason), false).is_err());
        let invalid_deny = StoredAuthorityDecisionV1 {
            version: 1,
            decision: "deny".to_owned(),
            effective_privilege: "administrative".to_owned(),
            policy: "v0-development-workstation".to_owned(),
            reason_code: "policy_denied".to_owned(),
        };
        assert!(validate_authority(&json(&invalid_deny), false).is_err());
    }

    #[test]
    fn tool_result_decode_rejects_unknown_kinds_and_unsafe_field_shapes() {
        let valid = StoredToolResultV1 {
            version: 1,
            result_kind: "process_exit".to_owned(),
            summary: "process exited".to_owned(),
            fields: vec![("status".to_owned(), "nonzero".to_owned())],
        };
        assert_eq!(
            validate_tool_result(&json(&valid)).unwrap(),
            ToolResultClass::ProcessExit
        );
        let unknown = StoredToolResultV1 {
            result_kind: "whatever".to_owned(),
            ..valid
        };
        assert!(validate_tool_result(&json(&unknown)).is_err());
        let invalid_field = StoredToolResultV1 {
            version: 1,
            result_kind: "success".to_owned(),
            summary: "ok".to_owned(),
            fields: vec![("bad/key".to_owned(), "value".to_owned())],
        };
        assert!(validate_tool_result(&json(&invalid_field)).is_err());
    }

    fn output(item: StoredNormalizedOutputItemV1) -> String {
        json(&StoredNormalizedOutputV1 {
            version: 1,
            items: vec![item],
        })
    }

    #[test]
    fn normalized_output_decode_reconstructs_every_nested_canonical_type() {
        assert!(
            validate_normalized_output(&output(StoredNormalizedOutputItemV1::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "run-shell".to_owned(),
                arguments_json: "{\"command\":\"true\"}".to_owned(),
            }))
            .is_ok()
        );
        assert!(
            validate_normalized_output(&output(StoredNormalizedOutputItemV1::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "INVALID".to_owned(),
                arguments_json: "{}".to_owned(),
            }))
            .is_err()
        );
        assert!(
            validate_normalized_output(&output(StoredNormalizedOutputItemV1::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "run-shell".to_owned(),
                arguments_json: "{not-json}".to_owned(),
            }))
            .is_err()
        );
        assert!(
            validate_normalized_output(&output(StoredNormalizedOutputItemV1::ProviderOpaque {
                provider_id: "test-provider".to_owned(),
                item_type: "opaque".to_owned(),
                sha256: Sha256Digest::hash_bytes(b"opaque"),
                artifact_id: "not-an-artifact-id".to_owned(),
            }))
            .is_err()
        );
        let oversized = StoredNormalizedOutputV1 {
            version: 1,
            items: (0..1_025)
                .map(|_| StoredNormalizedOutputItemV1::Text {
                    text: "x".to_owned(),
                })
                .collect(),
        };
        assert!(validate_normalized_output(&json(&oversized)).is_err());
        assert!(
            validate_normalized_output("{\"version\":1,\"items\":[{\"kind\":\"mystery\"}]}")
                .is_err()
        );
        assert!(
            validate_normalized_output(
                "{\"version\":1,\"items\":[{\"kind\":\"text\",\"text\":\"x\",\"unknown\":true}]}"
            )
            .is_err()
        );
    }

    #[test]
    fn model_capability_decode_enforces_version_and_token_relationship() {
        let valid = StoredModelCapabilitiesV1 {
            version: 1,
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            context_window_tokens: 1_000,
            max_output_tokens: 100,
        };
        assert!(decode_model_capabilities(&json(&valid)).is_ok());
        let invalid_relation = StoredModelCapabilitiesV1 {
            max_output_tokens: 1_001,
            ..valid.clone()
        };
        assert!(decode_model_capabilities(&json(&invalid_relation)).is_err());
        let invalid_version = StoredModelCapabilitiesV1 {
            version: 2,
            ..valid
        };
        assert!(decode_model_capabilities(&json(&invalid_version)).is_err());
    }

    #[test]
    fn every_remaining_stage8_dto_decoder_rejects_invalid_versions_or_relations() {
        assert!(
            validate_omissions(
                "{\"version\":2,\"omitted_source_count\":0,\"transformed_source_count\":0}"
            )
            .is_err()
        );
        assert!(
            validate_transform("{\"version\":1,\"kind\":\"identity\",\"transformed\":true}")
                .is_err()
        );
        assert!(validate_required_capabilities("{\"version\":2,\"text_input\":true,\"text_output\":true,\"custom_tool_calling\":false,\"streaming\":false,\"ordered_output_items\":false,\"structured_output\":false,\"reasoning_continuation\":false}").is_err());
        assert!(validate_output_policy("{\"version\":1,\"stdout_capture_limit\":1,\"stderr_capture_limit\":1,\"combined_inline_limit\":1,\"per_stream_inline_limit\":2}").is_err());
    }

    #[test]
    fn corruption_errors_are_redacted_without_parser_or_json_content() {
        let error = validate_normalized_output("{secret}").unwrap_err();
        assert_eq!(error.to_string(), "SQLite schema is inconsistent");
        assert_eq!(format!("{error:?}"), "SQLite schema is inconsistent");
        assert!(std::error::Error::source(&error).is_none());
    }
}
