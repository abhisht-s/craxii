//! Secret-safe OpenAI Responses API adapter.
//!
//! Provider wire values are deliberately confined to this module. The application receives only
//! the provider-neutral stream and response contracts from `ports::model_provider`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use serde_json::{Map, Value, json};

use crate::bootstrap::secret::SecretString;
use crate::domain::model::ModelUsage;
use crate::domain::{
    CanonicalModelToolCall, MAX_MODEL_COMPONENT_BYTES, MAX_MODEL_OUTPUT_ITEMS,
    MAX_MODEL_TOOL_ARGUMENT_BYTES, ModelCapabilitySnapshot, ModelConfigReference, ModelInputItem,
    ModelOutputItem, ModelRequest, ModelResponse, ModelResponseInput, ModelStopReason,
    ModelStreamEvent, ModelTarget, ModelTextPart, ModelToolCallId, ModelToolChoicePolicy,
    ProviderEvidenceId, ProviderId, ProviderMetadata, ProviderMetadataValue,
    ProviderOpaqueEvidence, TokenEstimatorIdentity, ToolName,
};
use crate::ports::clock::Clock;
use crate::ports::model_provider::{
    ConservativeTokenEstimate, ModelProvider, ModelProviderFuture, ModelProviderInvocation,
    ModelProviderStream, ProviderError, ProviderErrorKind, ProviderOutcomeCertainty,
    TokenEstimateUnit, TokenEstimator,
};

mod sse;
mod wire;
#[cfg(test)]
use sse::MAX_STREAM_BYTES;
use sse::{SseDecoder, SseEvent};

const OPENAI_PROVIDER_ID: &str = "openai";
const RESPONSES_PATH: &str = "responses";
const MAX_WIRE_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Only secret-bearing object in the live provider composition.
pub struct OpenAiProvider {
    provider_id: ProviderId,
    credentials: BTreeMap<String, SecretString>,
    client: reqwest::Client,
    clock: Arc<dyn Clock>,
}

impl OpenAiProvider {
    pub fn try_new(
        credentials: BTreeMap<String, SecretString>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        if credentials.is_empty() {
            return Err(not_sent(ProviderErrorKind::Authentication));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .retry(reqwest::retry::never())
            .connect_timeout(CONNECT_TIMEOUT)
            .connection_verbose(false)
            .http1_only()
            .user_agent("craxii/0.0.1")
            .build()
            .map_err(|_| not_sent(ProviderErrorKind::InternalProviderError))?;
        Ok(Self {
            provider_id: ProviderId::try_new(OPENAI_PROVIDER_ID)
                .map_err(|_| not_sent(ProviderErrorKind::InternalProviderError))?,
            credentials,
            client,
            clock,
        })
    }

    fn credential_for(&self, target: &ModelTarget) -> Result<&SecretString, ProviderError> {
        self.credentials
            .get(target.account_reference().as_str())
            .ok_or_else(|| not_sent(ProviderErrorKind::Authentication))
    }
}

impl Debug for OpenAiProvider {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiProvider")
            .field("provider_id", &self.provider_id)
            .field("credential_count", &self.credentials.len())
            .finish_non_exhaustive()
    }
}

impl ModelProvider for OpenAiProvider {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        if target.reference().provider_id() != &self.provider_id
            || !target.enabled()
            || target.reference().capabilities().structured_output()
        {
            return Err(not_sent(ProviderErrorKind::InvalidRequest));
        }
        Ok(target.reference().capabilities().clone())
    }

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            let target = invocation.request.target();
            self.capabilities(target)?;
            if invocation.control.cancellation().is_cancelled() {
                return Err(not_sent(ProviderErrorKind::Cancelled));
            }
            let endpoint = responses_endpoint(target.endpoint_reference().as_str())?;
            let body = encode_request(&invocation.request)?;
            if body.len() > MAX_WIRE_REQUEST_BYTES {
                return Err(not_sent(ProviderErrorKind::InvalidRequest));
            }
            let credential = self.credential_for(target)?;
            let request = self
                .client
                .post(endpoint)
                .header(ACCEPT, "text/event-stream")
                .header(CONTENT_TYPE, "application/json")
                .header(
                    "x-client-request-id",
                    invocation.request.logical_invocation_id().to_string(),
                )
                .bearer_auth(credential.expose_secret())
                .body(body);

            let remaining = invocation
                .control
                .absolute_deadline()
                .checked_duration_since(self.clock.monotonic_now())
                .ok_or_else(|| not_sent(ProviderErrorKind::TimeoutBeforeOutput))?;
            let response = tokio::select! {
                biased;
                _ = invocation.control.cancellation().cancelled() => {
                    return Err(unknown(ProviderErrorKind::Cancelled));
                }
                _ = tokio::time::sleep(remaining) => {
                    return Err(unknown(ProviderErrorKind::TimeoutBeforeOutput));
                }
                response = request.send() => response.map_err(classify_transport_error)?,
            };
            if !response.status().is_success() {
                return Err(classify_http_response(
                    response,
                    &invocation.control,
                    self.clock.as_ref(),
                )
                .await);
            }
            let content_type_ok = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
            if !content_type_ok {
                return Err(ProviderError::new(
                    ProviderErrorKind::MalformedResponse,
                    ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                ));
            }
            let provider_request_id = provider_request_id(response.headers())?;
            Ok(Box::new(OpenAiStream::new(
                response,
                invocation.request,
                invocation.control,
                Arc::clone(&self.clock),
                self.provider_id.clone(),
                provider_request_id,
            )) as Box<dyn ModelProviderStream>)
        })
    }
}

/// Documented conservative V0 estimator. A UTF-8 token cannot contain less than one byte; the
/// canonical request bytes already include framing, and the fixed allowance covers wire framing.
#[derive(Clone, Debug)]
pub struct OpenAiConservativeEstimator {
    identity: TokenEstimatorIdentity,
}

impl OpenAiConservativeEstimator {
    #[must_use]
    pub const fn new(identity: TokenEstimatorIdentity) -> Self {
        Self { identity }
    }
}

impl TokenEstimator for OpenAiConservativeEstimator {
    fn identity(&self) -> &TokenEstimatorIdentity {
        &self.identity
    }

    fn estimate(
        &self,
        target: &ModelTarget,
        units: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError> {
        if target.estimator() != &self.identity {
            return Err(not_sent(ProviderErrorKind::InvalidRequest));
        }
        let bytes = units.iter().try_fold(0_u64, |total, unit| {
            let value = match unit {
                TokenEstimateUnit::TextBytes(value)
                | TokenEstimateUnit::StructuredBytes(value)
                | TokenEstimateUnit::ToolDefinitionBytes(value)
                | TokenEstimateUnit::ProviderOpaqueBytes(value) => *value,
            };
            total.checked_add(value)
        });
        let tokens = bytes
            .and_then(|value| value.checked_add(256))
            .ok_or_else(|| not_sent(ProviderErrorKind::InvalidRequest))?;
        ConservativeTokenEstimate::try_new(self.identity.clone(), tokens)
    }
}

fn responses_endpoint(base: &str) -> Result<reqwest::Url, ProviderError> {
    let mut url =
        reqwest::Url::parse(base).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
    let allowed_scheme = url.scheme() == "https" || cfg!(test) && url.scheme() == "http";
    if !allowed_scheme
        || url.cannot_be_a_base()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    }
    let path = format!("{}/{}", url.path().trim_end_matches('/'), RESPONSES_PATH);
    url.set_path(&path);
    Ok(url)
}

fn encode_request(request: &ModelRequest) -> Result<Vec<u8>, ProviderError> {
    let mut input = Vec::new();
    for item in request.ordered_input_items() {
        translate_input_item(item, request.target(), &mut input)?;
    }
    let instructions = request
        .instructions()
        .iter()
        .map(ModelTextPart::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let tools = request
        .tool_definitions()
        .iter()
        .map(|definition| wire::FunctionTool {
            kind: "function",
            name: definition.name().as_str(),
            description: definition.description().as_str(),
            parameters: definition.input_schema(),
            strict: false,
        })
        .collect::<Vec<_>>();
    let body = wire::ResponsesRequest {
        model: request.target().reference().provider_model_id().as_str(),
        instructions,
        input,
        tools,
        tool_choice: match request.tool_choice_policy() {
            ModelToolChoicePolicy::Automatic => "auto",
            ModelToolChoicePolicy::None => "none",
        },
        parallel_tool_calls: false,
        store: false,
        stream: true,
        truncation: "disabled",
        max_output_tokens: u64::try_from(request.requested_output_limit().get())
            .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
    };
    serde_json::to_vec(&body).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))
}

fn translate_input_item(
    item: &ModelInputItem,
    target: &ModelTarget,
    output: &mut Vec<Value>,
) -> Result<(), ProviderError> {
    match item {
        ModelInputItem::Message {
            role,
            content_parts,
        } => output.push(message_item(role.as_str(), "input_text", "text", content_parts)),
        ModelInputItem::PriorAssistant { content_parts } => {
            output.push(message_item("assistant", "output_text", "text", content_parts));
        }
        ModelInputItem::ToolCall(call) => output.push(json!({
            "type": "function_call",
            "call_id": call.call_id().as_str(),
            "name": call.name().as_str(),
            "arguments": call.raw_arguments(),
        })),
        ModelInputItem::ToolResult { call_id, result } => output.push(json!({
            "type": "function_call_output",
            "call_id": call_id.as_str(),
            "output": serde_json::to_string(result).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        })),
        ModelInputItem::HistoricalRefusal { content_parts } => {
            output.push(message_item("assistant", "refusal", "refusal", content_parts));
        }
        ModelInputItem::HistoricalReasoningSummary { content_parts } => {
            output.push(evidence_message(json!({
                "craxii_evidence_type": "historical_reasoning_summary",
                "content_parts": content_parts.iter().map(ModelTextPart::as_str).collect::<Vec<_>>()
            }))?);
        }
        ModelInputItem::StructuredData { data } => output.push(evidence_message(json!({
            "craxii_evidence_type": "structured_data",
            "data": data,
        }))?),
        ModelInputItem::SyntheticRuntimeStatus { status, details } => output.push(evidence_message(json!({
            "craxii_evidence_type": "synthetic_runtime_status",
            "status": status.as_str(),
            "details": details,
        }))?),
        ModelInputItem::ProviderOpaqueContinuation(evidence) => {
            if !target.provider_native_options().reasoning_continuation()
                || evidence.provider_id() != target.reference().provider_id()
                || evidence.type_label() != "openai.reasoning_items.v1"
            {
                return Err(not_sent(ProviderErrorKind::InvalidRequest));
            }
            let items: Value = serde_json::from_str(evidence.opaque())
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
            let items = items
                .as_array()
                .ok_or_else(|| not_sent(ProviderErrorKind::InvalidRequest))?;
            if items.is_empty() {
                return Err(not_sent(ProviderErrorKind::InvalidRequest));
            }
            for item in items {
                validate_reasoning_continuation_item(item)?;
                output.push(item.clone());
            }
        }
    }
    Ok(())
}

fn evidence_message(value: Value) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": serde_json::to_string(&value)
                .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?,
        }],
    }))
}

fn validate_reasoning_continuation_item(item: &Value) -> Result<(), ProviderError> {
    let Some(object) = item.as_object() else {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type" | "id" | "encrypted_content" | "summary"
        )
    }) || object.get("type").and_then(Value::as_str) != Some("reasoning")
        || object
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || object
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    }
    let Some(summary) = object.get("summary").and_then(Value::as_array) else {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    };
    if summary.iter().any(|part| {
        let Some(part) = part.as_object() else {
            return true;
        };
        part.len() != 2
            || part.get("type").and_then(Value::as_str) != Some("summary_text")
            || part.get("text").and_then(Value::as_str).is_none()
    }) {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    }
    Ok(())
}

fn message_item(role: &str, content_type: &str, value_key: &str, parts: &[ModelTextPart]) -> Value {
    let content = parts
        .iter()
        .map(|part| {
            let mut value = Map::new();
            value.insert("type".to_owned(), content_type.into());
            value.insert(value_key.to_owned(), part.as_str().into());
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    json!({"role": role, "content": content})
}

fn provider_request_id(headers: &HeaderMap) -> Result<Option<ProviderEvidenceId>, ProviderError> {
    for name in ["x-request-id", "openai-request-id"] {
        if let Some(value) = headers.get(name) {
            let value = value
                .to_str()
                .map_err(|_| unknown(ProviderErrorKind::MalformedResponse))?;
            return ProviderEvidenceId::try_new(value.to_owned())
                .map(Some)
                .map_err(|_| unknown(ProviderErrorKind::MalformedResponse));
        }
    }
    Ok(None)
}

fn classify_transport_error(error: reqwest::Error) -> ProviderError {
    if error.is_connect() || error.is_builder() {
        not_sent(ProviderErrorKind::TransportBeforeResponse)
    } else if error.is_timeout() {
        unknown(ProviderErrorKind::TimeoutBeforeOutput)
    } else {
        unknown(ProviderErrorKind::TransportAfterPossibleProcessing)
    }
}

async fn classify_http_response(
    mut response: reqwest::Response,
    control: &crate::ports::model_provider::ModelInvocationControl,
    clock: &dyn Clock,
) -> ProviderError {
    const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(remaining) = control
        .absolute_deadline()
        .checked_duration_since(clock.monotonic_now())
    {
        let chunk = tokio::select! {
            biased;
            _ = control.cancellation().cancelled() => break,
            _ = tokio::time::sleep(remaining.min(control.idle_timeout())) => break,
            chunk = response.chunk() => match chunk {
                Ok(chunk) => chunk,
                Err(_) => break,
            },
        };
        let Some(chunk) = chunk else {
            break;
        };
        if body.len().saturating_add(chunk.len()) > MAX_ERROR_BODY_BYTES {
            body.clear();
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let error_code = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/code")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    value
                        .pointer("/error/type")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        });
    classify_http_error(
        status,
        &headers,
        error_code.as_deref(),
        clock.utc_now().ok(),
    )
}

fn classify_http_error(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    error_code: Option<&str>,
    now: Option<time::OffsetDateTime>,
) -> ProviderError {
    let status_kind = match status.as_u16() {
        400 | 405 | 406 | 409 | 415 | 422 => ProviderErrorKind::InvalidRequest,
        401 => ProviderErrorKind::Authentication,
        403 => ProviderErrorKind::Authorization,
        404 => ProviderErrorKind::UnknownModel,
        408 => ProviderErrorKind::TimeoutBeforeOutput,
        413 => ProviderErrorKind::OutputTooLarge,
        429 => ProviderErrorKind::RateLimited,
        500..=599 => ProviderErrorKind::TemporarilyUnavailable,
        _ => ProviderErrorKind::InternalProviderError,
    };
    let kind = match status.as_u16() {
        // Authentication and authorization status is authoritative even if a gateway or
        // provider returns a contradictory generic error `type` in the bounded body.
        401 | 403 => status_kind,
        // These codes carry more precise documented classifications than their status.
        400 | 404 | 422 | 429 => error_code
            .map(|code| classify_provider_error_code(Some(code)))
            .filter(|kind| *kind != ProviderErrorKind::InternalProviderError)
            .unwrap_or(status_kind),
        // A provider-returned server failure stays a definite transient provider failure.
        500..=599 => status_kind,
        _ => status_kind,
    };
    let retry_after = parse_retry_after(headers, now);
    retry_after.map_or_else(
        || ProviderError::new(kind, ProviderOutcomeCertainty::DefiniteProviderFailure),
        |delay| {
            ProviderError::with_retry_after(
                kind,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
                delay,
            )
        },
    )
}

fn classify_provider_error_code(code: Option<&str>) -> ProviderErrorKind {
    match code {
        Some("invalid_api_key") | Some("authentication_error") => ProviderErrorKind::Authentication,
        Some("permission_denied") | Some("authorization_error") => ProviderErrorKind::Authorization,
        Some(
            "insufficient_quota"
            | "credit_balance_exhausted"
            | "organization_spend_limit_exceeded"
            | "project_spend_limit_exceeded"
            | "organization_usage_limit_exceeded",
        ) => ProviderErrorKind::Authorization,
        Some("model_not_found") => ProviderErrorKind::UnknownModel,
        Some("context_length_exceeded") => ProviderErrorKind::ContextError,
        Some("rate_limit_exceeded") => ProviderErrorKind::RateLimited,
        Some("server_error" | "service_unavailable" | "temporarily_unavailable") => {
            ProviderErrorKind::TemporarilyUnavailable
        }
        Some("invalid_request_error" | "invalid_request") => ProviderErrorKind::InvalidRequest,
        _ => ProviderErrorKind::InternalProviderError,
    }
}

fn parse_retry_after(headers: &HeaderMap, now: Option<time::OffsetDateTime>) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let now = now?;
    let deadline =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822).ok()?;
    let seconds = (deadline - now).whole_seconds();
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

fn not_sent(kind: ProviderErrorKind) -> ProviderError {
    ProviderError::new(kind, ProviderOutcomeCertainty::DefinitelyNotSent)
}

fn unknown(kind: ProviderErrorKind) -> ProviderError {
    ProviderError::new(kind, ProviderOutcomeCertainty::ProviderOutcomeUnknown)
}

struct OpenAiStream {
    response: reqwest::Response,
    request: ModelRequest,
    control: crate::ports::model_provider::ModelInvocationControl,
    clock: Arc<dyn Clock>,
    provider_id: ProviderId,
    provider_request_id: Option<ProviderEvidenceId>,
    provider_response_id: Option<ProviderEvidenceId>,
    decoder: SseDecoder,
    pending: VecDeque<ModelStreamEvent>,
    expected_sequence: Option<u64>,
    started: bool,
    terminal: bool,
    stream_closed: bool,
    terminal_error_kind: Option<ProviderErrorKind>,
    terminal_error_is_provider_failure: bool,
    semantic_output: bool,
    item_kinds: BTreeMap<u32, String>,
    item_ids: BTreeMap<u32, String>,
    completed_items: BTreeSet<u32>,
    done_items: BTreeMap<u32, Value>,
    allowed_tools: BTreeSet<String>,
    tool_calls_allowed: bool,
    seen_call_ids: BTreeSet<String>,
    tool_calls: BTreeMap<u32, ToolAccumulator>,
    text: BTreeMap<(u32, u32), String>,
    refusal: BTreeMap<(u32, u32), String>,
    reasoning: BTreeMap<(u32, u32), String>,
    content_part_kinds: BTreeMap<(u32, u32), String>,
    completed_content_parts: BTreeSet<(u32, u32)>,
    reasoning_part_kinds: BTreeMap<(u32, u32), String>,
    completed_reasoning_parts: BTreeSet<(u32, u32)>,
}

struct ToolAccumulator {
    item_id: String,
    call_id: ModelToolCallId,
    name: ToolName,
    arguments: String,
    completed: bool,
}

impl OpenAiStream {
    fn new(
        response: reqwest::Response,
        request: ModelRequest,
        control: crate::ports::model_provider::ModelInvocationControl,
        clock: Arc<dyn Clock>,
        provider_id: ProviderId,
        provider_request_id: Option<ProviderEvidenceId>,
    ) -> Self {
        let allowed_tools = request
            .tool_definitions()
            .iter()
            .map(|definition| definition.name().as_str().to_owned())
            .collect();
        let tool_calls_allowed = request.tool_choice_policy() == ModelToolChoicePolicy::Automatic;
        Self {
            response,
            request,
            control,
            clock,
            provider_id,
            provider_request_id,
            provider_response_id: None,
            decoder: SseDecoder::new(),
            pending: VecDeque::new(),
            expected_sequence: None,
            started: false,
            terminal: false,
            stream_closed: false,
            terminal_error_kind: None,
            terminal_error_is_provider_failure: false,
            semantic_output: false,
            item_kinds: BTreeMap::new(),
            item_ids: BTreeMap::new(),
            completed_items: BTreeSet::new(),
            done_items: BTreeMap::new(),
            allowed_tools,
            tool_calls_allowed,
            seen_call_ids: BTreeSet::new(),
            tool_calls: BTreeMap::new(),
            text: BTreeMap::new(),
            refusal: BTreeMap::new(),
            reasoning: BTreeMap::new(),
            content_part_kinds: BTreeMap::new(),
            completed_content_parts: BTreeSet::new(),
            reasoning_part_kinds: BTreeMap::new(),
            completed_reasoning_parts: BTreeSet::new(),
        }
    }

    fn stream_error(&self, kind: ProviderErrorKind) -> ProviderError {
        ProviderError::new(
            kind,
            if self.semantic_output {
                ProviderOutcomeCertainty::SemanticOutputObserved
            } else {
                ProviderOutcomeCertainty::ProviderOutcomeUnknown
            },
        )
    }

    async fn next(&mut self) -> Result<Option<ModelStreamEvent>, ProviderError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                self.semantic_output |= event.is_semantic_output();
                return Ok(Some(event));
            }
            if let Some(kind) = self.terminal_error_kind.take() {
                return Err(if self.terminal_error_is_provider_failure {
                    ProviderError::new(
                        kind,
                        if self.semantic_output {
                            ProviderOutcomeCertainty::SemanticOutputObserved
                        } else {
                            ProviderOutcomeCertainty::DefiniteProviderFailure
                        },
                    )
                } else {
                    self.stream_error(kind)
                });
            }
            if self.stream_closed {
                return Ok(None);
            }
            if let Some(event) = self.decoder.pop_event() {
                self.process_sse(event)?;
                continue;
            }
            if self.control.cancellation().is_cancelled() {
                return Err(self.stream_error(ProviderErrorKind::Cancelled));
            }
            let remaining = self
                .control
                .absolute_deadline()
                .checked_duration_since(self.clock.monotonic_now())
                .ok_or_else(|| {
                    self.stream_error(if self.semantic_output {
                        ProviderErrorKind::TimeoutAfterOutput
                    } else {
                        ProviderErrorKind::TimeoutBeforeOutput
                    })
                })?;
            let wait = remaining.min(self.control.idle_timeout());
            let chunk = tokio::select! {
                biased;
                _ = self.control.cancellation().cancelled() => {
                    return Err(self.stream_error(ProviderErrorKind::Cancelled));
                }
                _ = tokio::time::sleep(wait) => {
                    return Err(self.stream_error(if self.semantic_output {
                        ProviderErrorKind::TimeoutAfterOutput
                    } else {
                        ProviderErrorKind::TimeoutBeforeOutput
                    }));
                }
                chunk = self.response.chunk() => chunk.map_err(|error| {
                    if error.is_timeout() {
                        self.stream_error(if self.semantic_output {
                            ProviderErrorKind::TimeoutAfterOutput
                        } else {
                            ProviderErrorKind::TimeoutBeforeOutput
                        })
                    } else {
                        self.stream_error(ProviderErrorKind::TransportAfterPossibleProcessing)
                    }
                })?,
            };
            match chunk {
                Some(bytes) => self
                    .decoder
                    .push(&bytes)
                    .map_err(|error| self.stream_error(error.kind()))?,
                None => {
                    self.decoder
                        .finish()
                        .map_err(|error| self.stream_error(error.kind()))?;
                    if self.decoder.has_event() {
                        continue;
                    }
                    if self.terminal {
                        self.stream_closed = true;
                        return Ok(None);
                    }
                    return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                }
            }
        }
    }

    fn process_sse(&mut self, event: SseEvent) -> Result<(), ProviderError> {
        if event.data == "[DONE]" {
            if !self.terminal {
                return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
            }
            self.stream_closed = true;
            return Ok(());
        }
        if self.terminal {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let envelope: wire::EventEnvelope = serde_json::from_str(&event.data)
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let event_type = envelope.kind.as_str();
        if let Some(label) = event.event.as_deref()
            && label != event_type
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let sequence = envelope.sequence_number;
        match self.expected_sequence {
            None if sequence == 0 => self.expected_sequence = Some(1),
            Some(expected) if sequence == expected => {
                self.expected_sequence = expected.checked_add(1);
            }
            _ => return Err(self.stream_error(ProviderErrorKind::MalformedResponse)),
        }
        match event_type {
            "response.created" => self.response_created(&value),
            "response.queued" => self.response_progress(&value, "queued"),
            "response.in_progress" => self.response_progress(&value, "in_progress"),
            "response.output_item.added" => self.output_item_added(&value),
            "response.output_text.delta" => self.text_delta(&value),
            "response.output_text.done" => self.text_done(&value),
            "response.refusal.delta" => self.refusal_delta(&value),
            "response.refusal.done" => self.refusal_done(&value),
            "response.reasoning_summary_text.delta" => self.reasoning_delta(&value),
            "response.reasoning_summary_text.done" => self.reasoning_done(&value),
            "response.reasoning_summary_part.added" => self.reasoning_part(&value, false),
            "response.reasoning_summary_part.done" => self.reasoning_part(&value, true),
            "response.content_part.added" => self.content_part(&value, false),
            "response.content_part.done" => self.content_part(&value, true),
            "response.output_text.annotation.added"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => self.unknown_event(event_type),
            "response.function_call_arguments.delta" => self.tool_delta(&value),
            "response.function_call_arguments.done" => self.tool_done(&value),
            "response.output_item.done" => self.output_item_done(&value),
            "response.completed" => self.response_terminal(&value, ModelStopReason::Completed),
            "response.incomplete" => {
                self.response_terminal(&value, ModelStopReason::IncompleteProviderLimit)
            }
            "response.failed" => self.response_failed(&value),
            "error" => self.error_event(&value),
            unknown_type => self.unknown_event(unknown_type),
        }
    }

    fn response_created(&mut self, event: &Value) -> Result<(), ProviderError> {
        if self.started {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let response = event
            .get("response")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let response_id =
            ProviderEvidenceId::try_new(required_str(response, "id", self)?.to_owned())
                .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if !matches!(
            response.get("status").and_then(Value::as_str),
            Some("queued" | "in_progress")
        ) || !response_echo_controls_match(response)
            || response
                .get("output")
                .is_some_and(|output| output.as_array().is_none_or(|items| !items.is_empty()))
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        self.provider_response_id = Some(response_id.clone());
        self.started = true;
        self.pending.push_back(ModelStreamEvent::ResponseStarted {
            target: self.request.target().identity(),
            provider_request_id: self.provider_request_id.clone(),
            provider_response_id: Some(response_id),
        });
        Ok(())
    }

    fn response_progress(&self, event: &Value, expected_status: &str) -> Result<(), ProviderError> {
        self.require_started()?;
        let response = event
            .get("response")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if response.get("status").and_then(Value::as_str) != Some(expected_status)
            || !response_echo_controls_match(response)
            || self
                .provider_response_id
                .as_ref()
                .map(ProviderEvidenceId::as_str)
                != response.get("id").and_then(Value::as_str)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn output_item_added(&mut self, event: &Value) -> Result<(), ProviderError> {
        self.require_started()?;
        let ordinal = ordinal(event, self)?;
        let item = event
            .get("item")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let kind = required_str(item, "type", self)?.to_owned();
        let item_id = required_str(item, "id", self)?.to_owned();
        if self.item_kinds.insert(ordinal, kind.clone()).is_some()
            || self.item_ids.insert(ordinal, item_id.clone()).is_some()
            || usize::try_from(ordinal).unwrap_or(usize::MAX) >= MAX_MODEL_OUTPUT_ITEMS
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        match kind.as_str() {
            "function_call" => {
                let call_id =
                    ModelToolCallId::try_new(required_str(item, "call_id", self)?.to_owned())
                        .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                let name = ToolName::try_new(required_str(item, "name", self)?.to_owned())
                    .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                if !self.tool_calls_allowed
                    || !self.allowed_tools.contains(name.as_str())
                    || !self.seen_call_ids.insert(call_id.as_str().to_owned())
                {
                    return Err(self.stream_error(ProviderErrorKind::UnsupportedResponseItem));
                }
                self.tool_calls.insert(
                    ordinal,
                    ToolAccumulator {
                        item_id,
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                        completed: false,
                    },
                );
                self.pending.push_back(ModelStreamEvent::ToolCallStarted {
                    item_ordinal: ordinal,
                    call_id,
                    name,
                });
            }
            "message" | "reasoning" => {}
            _ => return self.unknown_event(&format!("output_item.{kind}")),
        }
        Ok(())
    }

    fn text_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let content_index = subindex(event, "content_index", self)?;
        self.require_observed_part_kind(
            &self.content_part_kinds,
            (ordinal, content_index),
            "output_text",
        )?;
        let delta = required_str(event, "delta", self)?;
        let key = (ordinal, content_index);
        if delta.is_empty()
            || self
                .text
                .get(&key)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.text.entry(key).or_default().push_str(delta);
        self.pending.push_back(ModelStreamEvent::TextDelta {
            item_ordinal: ordinal,
            delta: ModelTextPart::try_new(delta.to_owned())
                .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
        });
        Ok(())
    }

    fn text_done(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let content_index = subindex(event, "content_index", self)?;
        self.require_observed_part_kind(
            &self.content_part_kinds,
            (ordinal, content_index),
            "output_text",
        )?;
        if self.text.get(&(ordinal, content_index)).map(String::as_str)
            != Some(required_str(event, "text", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn refusal_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let content_index = subindex(event, "content_index", self)?;
        self.require_observed_part_kind(
            &self.content_part_kinds,
            (ordinal, content_index),
            "refusal",
        )?;
        let delta = required_str(event, "delta", self)?;
        let key = (ordinal, content_index);
        if delta.is_empty()
            || self
                .refusal
                .get(&key)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.refusal.entry(key).or_default().push_str(delta);
        self.pending.push_back(ModelStreamEvent::RefusalDelta {
            item_ordinal: ordinal,
            delta: ModelTextPart::try_new(delta.to_owned())
                .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
        });
        Ok(())
    }

    fn refusal_done(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let content_index = subindex(event, "content_index", self)?;
        self.require_observed_part_kind(
            &self.content_part_kinds,
            (ordinal, content_index),
            "refusal",
        )?;
        let refusal = required_str(event, "refusal", self)?;
        if self
            .refusal
            .get(&(ordinal, content_index))
            .map(String::as_str)
            != Some(refusal)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        self.pending.push_back(ModelStreamEvent::RefusalCompleted {
            item_ordinal: ordinal,
        });
        Ok(())
    }

    fn reasoning_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        let summary_index = subindex(event, "summary_index", self)?;
        self.require_observed_part_kind(
            &self.reasoning_part_kinds,
            (ordinal, summary_index),
            "summary_text",
        )?;
        let delta = required_str(event, "delta", self)?;
        let key = (ordinal, summary_index);
        if delta.is_empty()
            || self
                .reasoning
                .get(&key)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.reasoning.entry(key).or_default().push_str(delta);
        self.pending
            .push_back(ModelStreamEvent::ReasoningSummaryDelta {
                item_ordinal: ordinal,
                delta: ModelTextPart::try_new(delta.to_owned())
                    .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
            });
        Ok(())
    }

    fn reasoning_done(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        let summary_index = subindex(event, "summary_index", self)?;
        self.require_observed_part_kind(
            &self.reasoning_part_kinds,
            (ordinal, summary_index),
            "summary_text",
        )?;
        if self
            .reasoning
            .get(&(ordinal, summary_index))
            .map(String::as_str)
            != Some(required_str(event, "text", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn reasoning_part(&mut self, event: &Value, done: bool) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        let summary_index = subindex(event, "summary_index", self)?;
        let part = event
            .get("part")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let kind = required_str(part, "type", self)?;
        if kind != "summary_text" {
            return Err(self.unsupported_item("reasoning_summary_part"));
        }
        observe_part(
            &mut self.reasoning_part_kinds,
            &mut self.completed_reasoning_parts,
            (ordinal, summary_index),
            kind,
            done,
        )
        .map_err(|kind| self.stream_error(kind))
    }

    fn content_part(&mut self, event: &Value, done: bool) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let content_index = subindex(event, "content_index", self)?;
        let part = event
            .get("part")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let kind = required_str(part, "type", self)?;
        if !matches!(kind, "output_text" | "refusal") {
            return Err(self.unsupported_item(kind));
        }
        observe_part(
            &mut self.content_part_kinds,
            &mut self.completed_content_parts,
            (ordinal, content_index),
            kind,
            done,
        )
        .map_err(|kind| self.stream_error(kind))
    }

    fn tool_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "function_call")?;
        self.require_item_id(event, ordinal)?;
        let item_id = required_str(event, "item_id", self)?;
        let delta = required_str(event, "delta", self)?;
        let missing = self.stream_error(ProviderErrorKind::MalformedResponse);
        let Some(accumulator) = self.tool_calls.get_mut(&ordinal) else {
            return Err(missing);
        };
        if accumulator.item_id != item_id || accumulator.completed {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if accumulator.arguments.len().saturating_add(delta.len()) > MAX_MODEL_TOOL_ARGUMENT_BYTES {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        accumulator.arguments.push_str(delta);
        self.pending.push_back(
            ModelStreamEvent::tool_argument_delta(
                ordinal,
                accumulator.call_id.clone(),
                delta.to_owned(),
            )
            .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
        );
        Ok(())
    }

    fn tool_done(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "function_call")?;
        self.require_item_id(event, ordinal)?;
        let accumulator = self
            .tool_calls
            .get(&ordinal)
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if accumulator.arguments != required_str(event, "arguments", self)?
            || accumulator.name.as_str() != required_str(event, "name", self)?
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn output_item_done(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = ordinal(event, self)?;
        let item = event
            .get("item")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let kind = required_str(item, "type", self)?;
        if self.item_kinds.get(&ordinal).map(String::as_str) != Some(kind) {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if self.item_ids.get(&ordinal).map(String::as_str) != Some(required_str(item, "id", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if !self.completed_items.insert(ordinal) {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if self.done_items.insert(ordinal, item.clone()).is_some() {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let item_status = required_str(item, "status", self)?;
        if !matches!(item_status, "completed" | "incomplete")
            || kind == "function_call" && item_status != "completed"
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if kind == "function_call" {
            let final_arguments = required_str(item, "arguments", self)?;
            let final_call_id = required_str(item, "call_id", self)?.to_owned();
            let final_name = required_str(item, "name", self)?.to_owned();
            let missing = self.stream_error(ProviderErrorKind::MalformedResponse);
            let Some(accumulator) = self.tool_calls.get_mut(&ordinal) else {
                return Err(missing);
            };
            if accumulator.completed
                || accumulator.arguments != final_arguments
                || accumulator.call_id.as_str() != final_call_id
                || accumulator.name.as_str() != final_name
            {
                return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
            }
            accumulator.completed = true;
            let call = CanonicalModelToolCall::try_new(
                accumulator.call_id.clone(),
                accumulator.name.as_str(),
                final_arguments,
            )
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedCompletedToolArguments))?;
            call.require_valid_arguments().map_err(|_| {
                self.stream_error(ProviderErrorKind::MalformedCompletedToolArguments)
            })?;
            self.pending.push_back(ModelStreamEvent::ToolCallCompleted {
                item_ordinal: ordinal,
                call,
            });
        }
        Ok(())
    }

    fn response_terminal(
        &mut self,
        event: &Value,
        requested_stop: ModelStopReason,
    ) -> Result<(), ProviderError> {
        self.require_started()?;
        if self.terminal {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let response = event
            .get("response")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let usage = self.observe_usage(response)?;
        let expected_status = if requested_stop == ModelStopReason::IncompleteProviderLimit {
            "incomplete"
        } else {
            "completed"
        };
        let response_id_matches = response
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                self.provider_response_id
                    .as_ref()
                    .is_some_and(|expected| expected.as_str() == id)
            });
        let controls_match = response.get("status").and_then(Value::as_str)
            == Some(expected_status)
            && response_echo_controls_match(response)
            && response.get("error").is_none_or(Value::is_null);
        if !response_id_matches || !controls_match {
            return self.defer_terminal_error(ProviderErrorKind::MalformedResponse, false);
        }
        if requested_stop == ModelStopReason::IncompleteProviderLimit {
            match response
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
            {
                Some("max_output_tokens") => {}
                Some("content_filter") => {
                    return self.defer_terminal_error(ProviderErrorKind::SafetyRefusal, true);
                }
                _ => {
                    return self
                        .defer_terminal_error(ProviderErrorKind::UnsupportedResponseItem, false);
                }
            }
        } else if !response
            .get("incomplete_details")
            .is_none_or(Value::is_null)
        {
            return self.defer_terminal_error(ProviderErrorKind::MalformedResponse, false);
        }
        let (output_items, continuation) = match self.normalize_output(response) {
            Ok(normalized) => normalized,
            Err(error) => return self.defer_terminal_error(error.kind(), false),
        };
        let stop_reason = if requested_stop == ModelStopReason::IncompleteProviderLimit {
            ModelStopReason::IncompleteProviderLimit
        } else if output_items.iter().any(|item| item.tool_call().is_some()) {
            ModelStopReason::ToolContinuation
        } else if output_items
            .iter()
            .any(|item| matches!(item, ModelOutputItem::Refusal { .. }))
        {
            ModelStopReason::Refusal
        } else {
            ModelStopReason::Completed
        };
        let Some(response_id) = self.provider_response_id.clone() else {
            return self.defer_terminal_error(ProviderErrorKind::MalformedResponse, false);
        };
        let metadata = match response_metadata(response, self) {
            Ok(metadata) => metadata,
            Err(error) => return self.defer_terminal_error(error.kind(), false),
        };
        let normalized = match ModelResponse::try_new(ModelResponseInput {
            selected_target: self.request.target().identity(),
            output_items,
            stop_reason,
            usage,
            provider_request_id: self.provider_request_id.clone(),
            provider_response_id: Some(response_id),
            provider_continuation: continuation,
            provider_metadata: metadata,
        }) {
            Ok(normalized) => normalized,
            Err(error) => {
                let kind = match error.kind() {
                    crate::domain::ModelContractErrorKind::ToolArgumentsTooLarge
                    | crate::domain::ModelContractErrorKind::NormalizedOutputTooLarge => {
                        ProviderErrorKind::OutputTooLarge
                    }
                    crate::domain::ModelContractErrorKind::InvalidToolArguments => {
                        ProviderErrorKind::MalformedCompletedToolArguments
                    }
                    _ => ProviderErrorKind::MalformedResponse,
                };
                return self.defer_terminal_error(kind, false);
            }
        };
        self.pending
            .push_back(ModelStreamEvent::Completed(Box::new(normalized)));
        self.terminal = true;
        Ok(())
    }

    fn response_failed(&mut self, event: &Value) -> Result<(), ProviderError> {
        self.require_started()?;
        let response = event
            .get("response")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        self.observe_usage(response)?;
        let response_id_matches = response
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| {
                self.provider_response_id
                    .as_ref()
                    .is_some_and(|expected| expected.as_str() == id)
            });
        if !response_id_matches || response.get("status").and_then(Value::as_str) != Some("failed")
        {
            return self.defer_terminal_error(ProviderErrorKind::MalformedResponse, false);
        }
        let code = response
            .pointer("/error/code")
            .and_then(Value::as_str)
            .or_else(|| response.pointer("/error/type").and_then(Value::as_str));
        if code.is_none() {
            return self.defer_terminal_error(ProviderErrorKind::MalformedResponse, false);
        }
        self.terminal_error_kind = Some(classify_provider_error_code(code));
        self.terminal_error_is_provider_failure = true;
        self.terminal = true;
        Ok(())
    }

    fn observe_usage(&mut self, response: &Value) -> Result<Option<ModelUsage>, ProviderError> {
        let usage = parse_usage(response, self)?;
        match usage {
            Some(usage) => self.pending.push_back(ModelStreamEvent::Usage(usage)),
            None => self.pending.push_back(ModelStreamEvent::UsageUnavailable),
        }
        Ok(usage)
    }

    fn defer_terminal_error(
        &mut self,
        kind: ProviderErrorKind,
        provider_failure: bool,
    ) -> Result<(), ProviderError> {
        self.terminal_error_kind = Some(kind);
        self.terminal_error_is_provider_failure = provider_failure;
        self.terminal = true;
        Ok(())
    }

    fn error_event(&mut self, event: &Value) -> Result<(), ProviderError> {
        let code = event
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| event.get("type").and_then(Value::as_str));
        self.terminal_error_kind = Some(classify_provider_error_code(code));
        self.terminal_error_is_provider_failure = true;
        self.terminal = true;
        Ok(())
    }

    fn normalize_output(
        &self,
        response: &Value,
    ) -> Result<(Vec<ModelOutputItem>, Option<ProviderOpaqueEvidence>), ProviderError> {
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if output.len() > MAX_MODEL_OUTPUT_ITEMS {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        if self.item_kinds.len() != output.len()
            || self.item_ids.len() != output.len()
            || self.completed_items.len() != output.len()
            || self.done_items.len() != output.len()
            || (0..output.len()).any(|index| {
                u32::try_from(index)
                    .ok()
                    .is_none_or(|ordinal| !self.completed_items.contains(&ordinal))
            })
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let mut normalized = Vec::new();
        let mut reasoning_wire = Vec::new();
        for (index, item) in output.iter().enumerate() {
            let ordinal = u32::try_from(index)
                .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?;
            if self.item_ids.get(&ordinal).map(String::as_str)
                != Some(required_str(item, "id", self)?)
                || self.done_items.get(&ordinal) != Some(item)
            {
                return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
            }
            let item_kind = required_str(item, "type", self)?;
            if self.item_kinds.get(&ordinal).map(String::as_str) != Some(item_kind) {
                return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
            }
            match item_kind {
                "message" => {
                    let content = item
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    let mut text = Vec::new();
                    let mut refusal = Vec::new();
                    let mut terminal_part_kinds = Vec::new();
                    for part in content {
                        let part_kind = required_str(part, "type", self)?;
                        terminal_part_kinds.push(part_kind.to_owned());
                        match part_kind {
                            "output_text" => {
                                if nonempty_array(part, "annotations")
                                    || nonempty_array(part, "logprobs")
                                {
                                    return Err(self.unsupported_item("output_text_metadata"));
                                }
                                text.push(
                                    ModelTextPart::try_new(
                                        required_str(part, "text", self)?.to_owned(),
                                    )
                                    .map_err(|_| {
                                        self.stream_error(ProviderErrorKind::OutputTooLarge)
                                    })?,
                                );
                            }
                            "refusal" => refusal.push(
                                ModelTextPart::try_new(
                                    required_str(part, "refusal", self)?.to_owned(),
                                )
                                .map_err(|_| {
                                    self.stream_error(ProviderErrorKind::OutputTooLarge)
                                })?,
                            ),
                            unknown => return Err(self.unsupported_item(unknown)),
                        }
                    }
                    verify_observed_parts(
                        &self.content_part_kinds,
                        &self.completed_content_parts,
                        ordinal,
                        &terminal_part_kinds,
                        self,
                    )?;
                    if !text.is_empty() && !refusal.is_empty() {
                        return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                    }
                    if !text.is_empty() {
                        verify_delta(&self.text, ordinal, &text, self)?;
                        normalized.push(ModelOutputItem::text(text).map_err(|_| {
                            self.stream_error(ProviderErrorKind::MalformedResponse)
                        })?);
                    } else if !refusal.is_empty() {
                        verify_delta(&self.refusal, ordinal, &refusal, self)?;
                        normalized.push(ModelOutputItem::refusal(refusal).map_err(|_| {
                            self.stream_error(ProviderErrorKind::MalformedResponse)
                        })?);
                    } else {
                        return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                    }
                }
                "function_call" => {
                    let call_id =
                        ModelToolCallId::try_new(required_str(item, "call_id", self)?.to_owned())
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    let call = CanonicalModelToolCall::try_new(
                        call_id,
                        required_str(item, "name", self)?,
                        required_str(item, "arguments", self)?,
                    )
                    .map_err(|_| {
                        self.stream_error(ProviderErrorKind::MalformedCompletedToolArguments)
                    })?;
                    call.require_valid_arguments().map_err(|_| {
                        self.stream_error(ProviderErrorKind::MalformedCompletedToolArguments)
                    })?;
                    let observed = self
                        .tool_calls
                        .get(&ordinal)
                        .filter(|value| value.completed)
                        .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    if observed.call_id != *call.call_id()
                        || observed.name != *call.name()
                        || observed.arguments != call.raw_arguments()
                    {
                        return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                    }
                    normalized.push(ModelOutputItem::ToolCall(call));
                }
                "reasoning" => {
                    let summary = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    let mut parts = Vec::new();
                    let mut terminal_part_kinds = Vec::new();
                    for part in summary {
                        let part_kind = required_str(part, "type", self)?;
                        if part_kind != "summary_text" {
                            return Err(self.unsupported_item("reasoning_summary"));
                        }
                        terminal_part_kinds.push(part_kind.to_owned());
                        parts.push(
                            ModelTextPart::try_new(required_str(part, "text", self)?.to_owned())
                                .map_err(|_| {
                                    self.stream_error(ProviderErrorKind::OutputTooLarge)
                                })?,
                        );
                    }
                    verify_observed_parts(
                        &self.reasoning_part_kinds,
                        &self.completed_reasoning_parts,
                        ordinal,
                        &terminal_part_kinds,
                        self,
                    )?;
                    if !parts.is_empty() {
                        verify_delta(&self.reasoning, ordinal, &parts, self)?;
                        normalized.push(ModelOutputItem::reasoning_summary(parts).map_err(
                            |_| self.stream_error(ProviderErrorKind::MalformedResponse),
                        )?);
                    }
                    if let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) {
                        if encrypted.is_empty() {
                            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                        }
                        let wire_item = json!({
                            "type": "reasoning",
                            "id": required_str(item, "id", self)?,
                            "encrypted_content": encrypted,
                            "summary": item.get("summary").cloned().unwrap_or_else(|| json!([])),
                        });
                        validate_reasoning_continuation_item(&wire_item)
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                        let opaque = serde_json::to_string(&wire_item)
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                        if !self
                            .request
                            .provider_native_options()
                            .reasoning_continuation()
                        {
                            normalized.push(ModelOutputItem::ProviderOpaque(
                                ProviderOpaqueEvidence::try_new(
                                    self.provider_id.clone(),
                                    "openai.reasoning_item.v1",
                                    opaque,
                                )
                                .map_err(|_| {
                                    self.stream_error(ProviderErrorKind::OutputTooLarge)
                                })?,
                            ));
                        }
                        reasoning_wire.push(wire_item);
                    }
                }
                unknown => return Err(self.unsupported_item(unknown)),
            }
            if normalized.len() > MAX_MODEL_OUTPUT_ITEMS {
                return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
            }
        }
        let continuation = if reasoning_wire.is_empty()
            || !self
                .request
                .provider_native_options()
                .reasoning_continuation()
        {
            None
        } else {
            let opaque = serde_json::to_string(&reasoning_wire)
                .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
            Some(
                ProviderOpaqueEvidence::try_new(
                    self.provider_id.clone(),
                    "openai.reasoning_items.v1",
                    opaque,
                )
                .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
            )
        };
        Ok((normalized, continuation))
    }

    fn require_started(&self) -> Result<(), ProviderError> {
        if self.started && !self.terminal {
            Ok(())
        } else {
            Err(self.stream_error(ProviderErrorKind::MalformedResponse))
        }
    }

    fn require_kind(&self, event: &Value, expected: &str) -> Result<u32, ProviderError> {
        self.require_started()?;
        let ordinal = ordinal(event, self)?;
        if self.item_kinds.get(&ordinal).map(String::as_str) != Some(expected) {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(ordinal)
    }

    fn require_item_id(&self, event: &Value, ordinal: u32) -> Result<(), ProviderError> {
        if self.item_ids.get(&ordinal).map(String::as_str)
            != Some(required_str(event, "item_id", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn require_observed_part_kind(
        &self,
        observed: &BTreeMap<(u32, u32), String>,
        key: (u32, u32),
        expected: &str,
    ) -> Result<(), ProviderError> {
        if observed
            .get(&key)
            .is_some_and(|kind| kind.as_str() != expected)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn unsupported_item(&self, _: &str) -> ProviderError {
        self.stream_error(ProviderErrorKind::UnsupportedResponseItem)
    }

    fn unknown_event(&mut self, event_type: &str) -> Result<(), ProviderError> {
        let evidence = ProviderOpaqueEvidence::try_new(
            self.provider_id.clone(),
            "openai.unknown_event_type.v1",
            event_type.to_owned(),
        )
        .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        self.pending
            .push_back(ModelStreamEvent::UnknownProviderEvent(evidence));
        self.terminal_error_kind = Some(ProviderErrorKind::UnsupportedResponseItem);
        self.terminal = true;
        Ok(())
    }
}

impl ModelProviderStream for OpenAiStream {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>> {
        Box::pin(self.next())
    }
}

fn required_str<'a>(
    value: &'a Value,
    key: &str,
    stream: &OpenAiStream,
) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| stream.stream_error(ProviderErrorKind::MalformedResponse))
}

fn required_u64(value: &Value, key: &str, stream: &OpenAiStream) -> Result<u64, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| stream.stream_error(ProviderErrorKind::MalformedResponse))
}

fn response_echo_controls_match(response: &Value) -> bool {
    response
        .get("store")
        .is_none_or(|value| value.as_bool() == Some(false))
        && response
            .get("parallel_tool_calls")
            .is_none_or(|value| value.as_bool() == Some(false))
        && response
            .get("previous_response_id")
            .is_none_or(Value::is_null)
        && response.get("conversation").is_none_or(Value::is_null)
}

fn ordinal(event: &Value, stream: &OpenAiStream) -> Result<u32, ProviderError> {
    u32::try_from(required_u64(event, "output_index", stream)?)
        .map_err(|_| stream.stream_error(ProviderErrorKind::OutputTooLarge))
}

fn subindex(event: &Value, key: &str, stream: &OpenAiStream) -> Result<u32, ProviderError> {
    u32::try_from(required_u64(event, key, stream)?)
        .map_err(|_| stream.stream_error(ProviderErrorKind::OutputTooLarge))
}

fn observe_part(
    kinds: &mut BTreeMap<(u32, u32), String>,
    completed: &mut BTreeSet<(u32, u32)>,
    key: (u32, u32),
    kind: &str,
    done: bool,
) -> Result<(), ProviderErrorKind> {
    if done {
        if kinds.get(&key).map(String::as_str) != Some(kind) || !completed.insert(key) {
            return Err(ProviderErrorKind::MalformedResponse);
        }
    } else if kinds.insert(key, kind.to_owned()).is_some() || completed.contains(&key) {
        return Err(ProviderErrorKind::MalformedResponse);
    }
    Ok(())
}

fn nonempty_array(value: &Value, key: &str) -> bool {
    match value.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Array(values)) => !values.is_empty(),
        Some(_) => true,
    }
}

fn verify_observed_parts(
    observed: &BTreeMap<(u32, u32), String>,
    completed: &BTreeSet<(u32, u32)>,
    ordinal: u32,
    terminal_kinds: &[String],
    stream: &OpenAiStream,
) -> Result<(), ProviderError> {
    let observed_count = observed
        .keys()
        .filter(|(item_ordinal, _)| *item_ordinal == ordinal)
        .count();
    if observed_count == 0 {
        return Ok(());
    }
    let completed_count = completed
        .iter()
        .filter(|(item_ordinal, _)| *item_ordinal == ordinal)
        .count();
    if observed_count != terminal_kinds.len()
        || completed_count != terminal_kinds.len()
        || terminal_kinds.iter().enumerate().any(|(index, kind)| {
            u32::try_from(index).ok().is_none_or(|index| {
                observed.get(&(ordinal, index)) != Some(kind)
                    || !completed.contains(&(ordinal, index))
            })
        })
    {
        return Err(stream.stream_error(ProviderErrorKind::MalformedResponse));
    }
    Ok(())
}

fn verify_delta(
    observed: &BTreeMap<(u32, u32), String>,
    ordinal: u32,
    parts: &[ModelTextPart],
    stream: &OpenAiStream,
) -> Result<(), ProviderError> {
    let observed_count = observed
        .keys()
        .filter(|(item_ordinal, _)| *item_ordinal == ordinal)
        .count();
    if observed_count != parts.len()
        || parts.iter().enumerate().any(|(index, part)| {
            u32::try_from(index).ok().is_none_or(|index| {
                observed.get(&(ordinal, index)).map(String::as_str) != Some(part.as_str())
            })
        })
    {
        return Err(stream.stream_error(ProviderErrorKind::MalformedResponse));
    }
    Ok(())
}

fn parse_usage(
    response: &Value,
    stream: &OpenAiStream,
) -> Result<Option<ModelUsage>, ProviderError> {
    let Some(usage) = response.get("usage").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let usage: wire::Usage = serde_json::from_value(usage.clone())
        .map_err(|_| stream.stream_error(ProviderErrorKind::MalformedResponse))?;
    ModelUsage::try_new(
        usage.input_tokens,
        usage.input_tokens_details.cached_tokens,
        usage.output_tokens,
        usage.output_tokens_details.reasoning_tokens,
        usage.total_tokens,
    )
    .map(Some)
    .map_err(|_| stream.stream_error(ProviderErrorKind::MalformedResponse))
}

fn response_metadata(
    response: &Value,
    stream: &OpenAiStream,
) -> Result<ProviderMetadata, ProviderError> {
    let mut entries = Vec::new();
    for (key, wire_key) in [
        ("served_model", "model"),
        ("service_tier", "service_tier"),
        ("status", "status"),
    ] {
        if let Some(value) = response.get(wire_key).and_then(Value::as_str) {
            entries.push((
                key.to_owned(),
                ProviderMetadataValue::Identifier(
                    ModelConfigReference::named(value.to_owned())
                        .map_err(|_| stream.stream_error(ProviderErrorKind::MalformedResponse))?,
                ),
            ));
        }
    }
    ProviderMetadata::try_new(entries)
        .map_err(|_| stream.stream_error(ProviderErrorKind::MalformedResponse))
}

#[cfg(test)]
mod tests;
