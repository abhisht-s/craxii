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
use crate::domain::{
    CanonicalModelToolCall, ModelCapabilitySnapshot, ModelConfigReference, ModelInputItem,
    ModelOutputItem, ModelRequest, ModelResponse, ModelResponseInput, ModelStopReason,
    ModelStreamEvent, ModelTarget, ModelTextPart, ModelToolCallId, ModelToolChoicePolicy,
    ProviderEvidenceId, ProviderId, ProviderMetadata, ProviderMetadataValue,
    ProviderOpaqueEvidence, TokenEstimatorIdentity, ToolName, MAX_MODEL_COMPONENT_BYTES,
    MAX_MODEL_OUTPUT_ITEMS, MAX_MODEL_TOOL_ARGUMENT_BYTES,
};
use crate::ports::clock::Clock;
use crate::domain::model::ModelUsage;
use crate::ports::model_provider::{
    ConservativeTokenEstimate, ModelProvider, ModelProviderFuture, ModelProviderInvocation,
    ModelProviderStream, ProviderError, ProviderErrorKind, ProviderOutcomeCertainty,
    TokenEstimateUnit, TokenEstimator,
};

const OPENAI_PROVIDER_ID: &str = "openai";
const RESPONSES_PATH: &str = "responses";
const MAX_WIRE_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 512 * 1024;
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
            .connect_timeout(CONNECT_TIMEOUT)
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
        if target.reference().provider_id() != &self.provider_id || !target.enabled() {
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
                return Err(
                    classify_http_response(response, &invocation.control, self.clock.as_ref())
                        .await,
                );
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
    let mut url = reqwest::Url::parse(base)
        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?;
    if !matches!(url.scheme(), "https" | "http") || url.cannot_be_a_base() {
        return Err(not_sent(ProviderErrorKind::InvalidRequest));
    }
    let path = format!("{}/{}", url.path().trim_end_matches('/'), RESPONSES_PATH);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
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
        .map(|definition| {
            json!({
                "type": "function",
                "name": definition.name().as_str(),
                "description": definition.description().as_str(),
                "parameters": definition.input_schema(),
                "strict": true,
            })
        })
        .collect::<Vec<_>>();
    let mut body = Map::new();
    body.insert(
        "model".to_owned(),
        request
            .target()
            .reference()
            .provider_model_id()
            .as_str()
            .into(),
    );
    body.insert("instructions".to_owned(), instructions.into());
    body.insert("input".to_owned(), Value::Array(input));
    body.insert("tools".to_owned(), Value::Array(tools));
    body.insert(
        "tool_choice".to_owned(),
        match request.tool_choice_policy() {
            ModelToolChoicePolicy::Automatic => "auto",
            ModelToolChoicePolicy::None => "none",
        }
        .into(),
    );
    body.insert("parallel_tool_calls".to_owned(), false.into());
    body.insert("store".to_owned(), false.into());
    body.insert("stream".to_owned(), true.into());
    body.insert("truncation".to_owned(), "disabled".into());
    body.insert(
        "max_output_tokens".to_owned(),
        u64::try_from(request.requested_output_limit().get())
            .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?
            .into(),
    );
    if request
        .provider_native_options()
        .reasoning_continuation()
    {
        body.insert(
            "include".to_owned(),
            json!(["reasoning.encrypted_content"]),
        );
    }
    serde_json::to_vec(&Value::Object(body))
        .map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))
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
            output.push(message_item("assistant", "output_text", "text", content_parts));
        }
        ModelInputItem::StructuredData { data } => output.push(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": serde_json::to_string(data).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?}],
        })),
        ModelInputItem::SyntheticRuntimeStatus { status, details } => output.push(json!({
            "role": "developer",
            "content": [{"type": "input_text", "text": serde_json::to_string(&json!({"status": status.as_str(), "details": details})).map_err(|_| not_sent(ProviderErrorKind::InvalidRequest))?}],
        })),
        ModelInputItem::ProviderOpaqueContinuation(evidence) => {
            if evidence.provider_id() != target.reference().provider_id()
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
                if item.get("type").and_then(Value::as_str) != Some("reasoning")
                    || item.get("encrypted_content").and_then(Value::as_str).is_none()
                {
                    return Err(not_sent(ProviderErrorKind::InvalidRequest));
                }
                output.push(item.clone());
            }
        }
    }
    Ok(())
}

fn message_item(
    role: &str,
    content_type: &str,
    value_key: &str,
    parts: &[ModelTextPart],
) -> Value {
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
    loop {
        let Some(remaining) = control
            .absolute_deadline()
            .checked_duration_since(clock.monotonic_now())
        else {
            break;
        };
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
    classify_http_error(status, &headers, error_code.as_deref())
}

fn classify_http_error(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    error_code: Option<&str>,
) -> ProviderError {
    let kind = match error_code {
        Some("invalid_api_key") | Some("authentication_error") => ProviderErrorKind::Authentication,
        Some("permission_denied") | Some("authorization_error") => {
            ProviderErrorKind::Authorization
        }
        Some("model_not_found") => ProviderErrorKind::UnknownModel,
        Some("context_length_exceeded") => ProviderErrorKind::ContextError,
        Some("rate_limit_exceeded") | Some("insufficient_quota") => {
            ProviderErrorKind::RateLimited
        }
        Some("server_error") | Some("service_unavailable") => {
            ProviderErrorKind::TemporarilyUnavailable
        }
        _ => match status.as_u16() {
        400 | 405 | 406 | 409 | 415 | 422 => ProviderErrorKind::InvalidRequest,
        401 => ProviderErrorKind::Authentication,
        403 => ProviderErrorKind::Authorization,
        404 => ProviderErrorKind::UnknownModel,
        408 => ProviderErrorKind::TimeoutBeforeOutput,
        413 => ProviderErrorKind::OutputTooLarge,
        429 => ProviderErrorKind::RateLimited,
        500..=599 => ProviderErrorKind::TemporarilyUnavailable,
        _ => ProviderErrorKind::InternalProviderError,
        },
    };
    let retry_after = headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    retry_after.map_or_else(
        || {
            ProviderError::new(
                kind,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
            )
        },
        |delay| {
            ProviderError::with_retry_after(
                kind,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
                delay,
            )
        },
    )
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
    semantic_output: bool,
    item_kinds: BTreeMap<u32, String>,
    item_ids: BTreeMap<u32, String>,
    completed_items: BTreeSet<u32>,
    tool_calls: BTreeMap<u32, ToolAccumulator>,
    text: BTreeMap<u32, String>,
    refusal: BTreeMap<u32, String>,
    reasoning: BTreeMap<u32, String>,
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
            semantic_output: false,
            item_kinds: BTreeMap::new(),
            item_ids: BTreeMap::new(),
            completed_items: BTreeSet::new(),
            tool_calls: BTreeMap::new(),
            text: BTreeMap::new(),
            refusal: BTreeMap::new(),
            reasoning: BTreeMap::new(),
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
            if self.terminal {
                return Ok(None);
            }
            if let Some(event) = self
                .decoder
                .pop_event()
                .map_err(|error| self.stream_error(error.kind()))?
            {
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
                    return Err(self.stream_error(ProviderErrorKind::TransportAfterPossibleProcessing));
                }
            }
        }
    }

    fn process_sse(&mut self, event: SseEvent) -> Result<(), ProviderError> {
        if event.data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let event_type = required_str(&value, "type", self)?;
        if let Some(label) = event.event.as_deref()
            && label != event_type
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let sequence = required_u64(&value, "sequence_number", self)?;
        match self.expected_sequence {
            None if sequence == 0 => self.expected_sequence = Some(1),
            Some(expected) if sequence == expected => {
                self.expected_sequence = expected.checked_add(1);
            }
            _ => return Err(self.stream_error(ProviderErrorKind::MalformedResponse)),
        }
        match event_type {
            "response.created" => self.response_created(&value),
            "response.queued" | "response.in_progress" => {
                if !self.started {
                    Err(self.stream_error(ProviderErrorKind::MalformedResponse))
                } else {
                    Ok(())
                }
            }
            "response.output_item.added" => self.output_item_added(&value),
            "response.output_text.delta" => self.text_delta(&value),
            "response.output_text.done" => self.text_done(&value),
            "response.refusal.delta" => self.refusal_delta(&value),
            "response.refusal.done" => self.refusal_done(&value),
            "response.reasoning_summary_text.delta" => self.reasoning_delta(&value),
            "response.reasoning_summary_text.done" => self.reasoning_done(&value),
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done" => self.reasoning_part(&value),
            "response.content_part.added" | "response.content_part.done" => {
                self.content_part(&value)
            }
            "response.function_call_arguments.delta" => self.tool_delta(&value),
            "response.function_call_arguments.done" => self.tool_done(&value),
            "response.output_item.done" => self.output_item_done(&value),
            "response.completed" => self.response_terminal(&value, ModelStopReason::Completed),
            "response.incomplete" => {
                self.response_terminal(&value, ModelStopReason::IncompleteProviderLimit)
            }
            "response.failed" | "error" => {
                self.pending.push_back(ModelStreamEvent::ProviderError {
                    kind: crate::domain::ModelStreamProviderErrorKind::DefiniteFailure,
                });
                self.terminal = true;
                Ok(())
            }
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
        let response_id = ProviderEvidenceId::try_new(required_str(response, "id", self)?.to_owned())
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        self.provider_response_id = Some(response_id.clone());
        self.started = true;
        self.pending.push_back(ModelStreamEvent::ResponseStarted {
            target: self.request.target().identity(),
            provider_request_id: self.provider_request_id.clone(),
            provider_response_id: Some(response_id),
        });
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
                let call_id = ModelToolCallId::try_new(required_str(item, "call_id", self)?.to_owned())
                    .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                let name = ToolName::try_new(required_str(item, "name", self)?.to_owned())
                    .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
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
        let delta = required_str(event, "delta", self)?;
        if delta.is_empty()
            || self
                .text
                .get(&ordinal)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.text.entry(ordinal).or_default().push_str(delta);
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
        if self.text.get(&ordinal).map(String::as_str)
            != Some(required_str(event, "text", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn refusal_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let delta = required_str(event, "delta", self)?;
        if delta.is_empty()
            || self
                .refusal
                .get(&ordinal)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.refusal.entry(ordinal).or_default().push_str(delta);
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
        let refusal = required_str(event, "refusal", self)?;
        if self.refusal.get(&ordinal).map(String::as_str) != Some(refusal) {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        self.pending
            .push_back(ModelStreamEvent::RefusalCompleted { item_ordinal: ordinal });
        Ok(())
    }

    fn reasoning_delta(&mut self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        let delta = required_str(event, "delta", self)?;
        if delta.is_empty()
            || self
                .reasoning
                .get(&ordinal)
                .map_or(0, String::len)
                .saturating_add(delta.len())
                > MAX_MODEL_COMPONENT_BYTES
        {
            return Err(self.stream_error(ProviderErrorKind::OutputTooLarge));
        }
        self.reasoning.entry(ordinal).or_default().push_str(delta);
        self.pending.push_back(ModelStreamEvent::ReasoningSummaryDelta {
            item_ordinal: ordinal,
            delta: ModelTextPart::try_new(delta.to_owned())
                .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?,
        });
        Ok(())
    }

    fn reasoning_done(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        if self.reasoning.get(&ordinal).map(String::as_str)
            != Some(required_str(event, "text", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        Ok(())
    }

    fn reasoning_part(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "reasoning")?;
        self.require_item_id(event, ordinal)?;
        let part = event
            .get("part")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if required_str(part, "type", self)? != "summary_text" {
            return Err(self.unsupported_item("reasoning_summary_part"));
        }
        Ok(())
    }

    fn content_part(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "message")?;
        self.require_item_id(event, ordinal)?;
        let part = event
            .get("part")
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        match required_str(part, "type", self)? {
            "output_text" | "refusal" => Ok(()),
            unknown => Err(self.unsupported_item(unknown)),
        }
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
        self.pending.push_back(ModelStreamEvent::tool_argument_delta(
            ordinal,
            accumulator.call_id.clone(),
            delta.to_owned(),
        ).map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?);
        Ok(())
    }

    fn tool_done(&self, event: &Value) -> Result<(), ProviderError> {
        let ordinal = self.require_kind(event, "function_call")?;
        self.require_item_id(event, ordinal)?;
        let accumulator = self
            .tool_calls
            .get(&ordinal)
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        if accumulator.arguments != required_str(event, "arguments", self)? {
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
        if self.item_ids.get(&ordinal).map(String::as_str) != Some(required_str(item, "id", self)?) {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        if !self.completed_items.insert(ordinal) {
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
        if self.provider_response_id.as_ref().map(ProviderEvidenceId::as_str)
            != Some(required_str(response, "id", self)?)
        {
            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
        }
        let (output_items, continuation) = self.normalize_output(response)?;
        let usage = parse_usage(response, self)?;
        match usage {
            Some(usage) => self.pending.push_back(ModelStreamEvent::Usage(usage)),
            None => self.pending.push_back(ModelStreamEvent::UsageUnavailable),
        }
        let usage = usage.unwrap_or(ModelUsage::try_new(0, 0, 0, 0, 0)
            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?);
        let stop_reason = if requested_stop == ModelStopReason::IncompleteProviderLimit {
            ModelStopReason::IncompleteProviderLimit
        } else if output_items.iter().any(|item| item.tool_call().is_some()) {
            ModelStopReason::ToolContinuation
        } else if output_items.iter().any(|item| matches!(item, ModelOutputItem::Refusal { .. })) {
            ModelStopReason::Refusal
        } else {
            ModelStopReason::Completed
        };
        let response_id = self
            .provider_response_id
            .clone()
            .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
        let metadata = response_metadata(response, self)?;
        let normalized = ModelResponse::try_new(ModelResponseInput {
            selected_target: self.request.target().identity(),
            output_items,
            stop_reason,
            usage,
            provider_request_id: self.provider_request_id.clone(),
            provider_response_id: Some(response_id),
            provider_continuation: continuation,
            provider_metadata: metadata,
        })
        .map_err(|error| {
            self.stream_error(match error.kind() {
                crate::domain::ModelContractErrorKind::ToolArgumentsTooLarge
                | crate::domain::ModelContractErrorKind::NormalizedOutputTooLarge => {
                    ProviderErrorKind::OutputTooLarge
                }
                crate::domain::ModelContractErrorKind::InvalidToolArguments => {
                    ProviderErrorKind::MalformedCompletedToolArguments
                }
                _ => ProviderErrorKind::MalformedResponse,
            })
        })?;
        self.pending.push_back(ModelStreamEvent::Completed(normalized));
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
            {
                return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
            }
            match required_str(item, "type", self)? {
                "message" => {
                    let content = item
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or_else(|| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    let mut text = Vec::new();
                    let mut refusal = Vec::new();
                    for part in content {
                        match required_str(part, "type", self)? {
                            "output_text" => text.push(ModelTextPart::try_new(
                                required_str(part, "text", self)?.to_owned(),
                            ).map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?),
                            "refusal" => refusal.push(ModelTextPart::try_new(
                                required_str(part, "refusal", self)?.to_owned(),
                            ).map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?),
                            unknown => return Err(self.unsupported_item(unknown)),
                        }
                    }
                    if !text.is_empty() && !refusal.is_empty() {
                        return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                    }
                    if !text.is_empty() {
                        verify_delta(&self.text, ordinal, &text, self)?;
                        normalized.push(ModelOutputItem::text(text)
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?);
                    } else if !refusal.is_empty() {
                        verify_delta(&self.refusal, ordinal, &refusal, self)?;
                        normalized.push(ModelOutputItem::refusal(refusal)
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?);
                    } else {
                        return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                    }
                }
                "function_call" => {
                    let call_id = ModelToolCallId::try_new(required_str(item, "call_id", self)?.to_owned())
                        .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
                    let call = CanonicalModelToolCall::try_new(
                        call_id,
                        required_str(item, "name", self)?,
                        required_str(item, "arguments", self)?,
                    )
                    .map_err(|_| self.stream_error(ProviderErrorKind::MalformedCompletedToolArguments))?;
                    let observed = self.tool_calls.get(&ordinal)
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
                    for part in summary {
                        if required_str(part, "type", self)? != "summary_text" {
                            return Err(self.unsupported_item("reasoning_summary"));
                        }
                        parts.push(ModelTextPart::try_new(required_str(part, "text", self)?.to_owned())
                            .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?);
                    }
                    if !parts.is_empty() {
                        verify_delta(&self.reasoning, ordinal, &parts, self)?;
                        normalized.push(ModelOutputItem::reasoning_summary(parts)
                            .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?);
                    }
                    if let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) {
                        if encrypted.is_empty() {
                            return Err(self.stream_error(ProviderErrorKind::MalformedResponse));
                        }
                        reasoning_wire.push(json!({
                            "type": "reasoning",
                            "id": required_str(item, "id", self)?,
                            "encrypted_content": encrypted,
                            "summary": item.get("summary").cloned().unwrap_or_else(|| json!([])),
                        }));
                    }
                }
                unknown => return Err(self.unsupported_item(unknown)),
            }
        }
        let continuation = if reasoning_wire.is_empty() {
            None
        } else {
            let opaque = serde_json::to_string(&reasoning_wire)
                .map_err(|_| self.stream_error(ProviderErrorKind::MalformedResponse))?;
            Some(ProviderOpaqueEvidence::try_new(
                self.provider_id.clone(),
                "openai.reasoning_items.v1",
                opaque,
            )
            .map_err(|_| self.stream_error(ProviderErrorKind::OutputTooLarge))?)
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
        self.pending.push_back(ModelStreamEvent::ProviderError {
            kind: crate::domain::ModelStreamProviderErrorKind::ProtocolFailure,
        });
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

fn ordinal(event: &Value, stream: &OpenAiStream) -> Result<u32, ProviderError> {
    u32::try_from(required_u64(event, "output_index", stream)?)
        .map_err(|_| stream.stream_error(ProviderErrorKind::OutputTooLarge))
}

fn verify_delta(
    observed: &BTreeMap<u32, String>,
    ordinal: u32,
    parts: &[ModelTextPart],
    stream: &OpenAiStream,
) -> Result<(), ProviderError> {
    let terminal = parts.iter().map(ModelTextPart::as_str).collect::<String>();
    if observed.get(&ordinal) != Some(&terminal) {
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
    let input = required_u64(usage, "input_tokens", stream)?;
    let output = required_u64(usage, "output_tokens", stream)?;
    let total = required_u64(usage, "total_tokens", stream)?;
    let cached = usage
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = usage
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    ModelUsage::try_new(input, cached, output, reasoning, total)
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

#[derive(Debug)]
struct SseEvent {
    event: Option<String>,
    data: String,
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    total_bytes: usize,
    event_name: Option<String>,
    data_lines: Vec<String>,
    event_bytes: usize,
    ready: VecDeque<SseEvent>,
}

impl SseDecoder {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| unknown(ProviderErrorKind::OutputTooLarge))?;
        if self.total_bytes > MAX_STREAM_BYTES {
            return Err(unknown(ProviderErrorKind::OutputTooLarge));
        }
        self.buffer.extend_from_slice(bytes);
        self.consume_lines(false)?;
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(unknown(ProviderErrorKind::OutputTooLarge));
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ProviderError> {
        self.consume_lines(true)?;
        if self.event_name.is_some() || !self.data_lines.is_empty() {
            self.dispatch()?;
        }
        Ok(())
    }

    fn consume_lines(&mut self, eof: bool) -> Result<(), ProviderError> {
        loop {
            let newline = self.buffer.iter().position(|byte| *byte == b'\n');
            let take = match (newline, eof && !self.buffer.is_empty()) {
                (Some(index), _) => index + 1,
                (None, true) => self.buffer.len(),
                (None, false) => break,
            };
            let mut line = self.buffer.drain(..take).collect::<Vec<_>>();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| unknown(ProviderErrorKind::MalformedResponse))?;
            if line.is_empty() {
                if self.event_name.is_some() || !self.data_lines.is_empty() {
                    self.dispatch()?;
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            self.event_bytes = self.event_bytes.saturating_add(line.len());
            if self.event_bytes > MAX_SSE_EVENT_BYTES {
                return Err(unknown(ProviderErrorKind::OutputTooLarge));
            }
            if let Some(value) = line.strip_prefix("event:") {
                if self.event_name.is_some() {
                    return Err(unknown(ProviderErrorKind::MalformedResponse));
                }
                self.event_name = Some(value.strip_prefix(' ').unwrap_or(value).to_owned());
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data_lines
                    .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            }
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Result<(), ProviderError> {
        if self.data_lines.is_empty() {
            return Err(unknown(ProviderErrorKind::MalformedResponse));
        }
        self.ready.push_back(SseEvent {
            event: self.event_name.take(),
            data: std::mem::take(&mut self.data_lines).join("\n"),
        });
        self.event_bytes = 0;
        Ok(())
    }

    fn pop_event(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        Ok(self.ready.pop_front())
    }

    fn has_event(&self) -> bool {
        !self.ready.is_empty()
    }
}

#[cfg(test)]
mod tests;
