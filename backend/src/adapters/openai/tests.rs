use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap as AxumHeaderMap, Response, StatusCode};
use axum::routing::post;
use serde_json::{Value, json};

use super::*;
use crate::domain::{
    ContextManifestId, LogicalInvocationId, ModelCapabilitySnapshotInput, ModelConfigReference,
    ModelInputRole, ModelRequestInput, ModelTargetId, ModelTargetInput, ModelToolDefinition,
    ProviderModelId, ProviderModelReference, ProviderNativeOptions, SchemaVersion,
    TargetConfigurationVersion, TokenCount, ToolVersion,
};
use crate::ports::clock::{MonotonicInstant, TestClock};
use crate::ports::model_provider::{
    ModelInvocationControl, ProviderAttempt, ProviderCancellationToken,
};

const SENTINEL: &str = "stage19-secret-sentinel-never-log";

#[test]
fn fragmented_multiline_sse_is_reassembled_with_crlf_support() {
    let mut decoder = SseDecoder::new();
    decoder.push(b"event: response.created\r\nda").unwrap();
    assert!(!decoder.has_event());
    decoder
        .push(b"ta: {\"type\":\r\ndata: \"response.created\"}\r\n\r\n")
        .unwrap();
    let event = decoder.pop_event().unwrap().unwrap();
    assert_eq!(event.event.as_deref(), Some("response.created"));
    assert_eq!(event.data, "{\"type\":\n\"response.created\"}");
}

#[test]
fn sse_decoder_rejects_invalid_utf8_and_total_overflow() {
    let mut invalid = SseDecoder::new();
    assert_eq!(
        invalid.push(b"data: \xff\n\n").unwrap_err().kind(),
        ProviderErrorKind::MalformedResponse
    );
    let mut oversized = SseDecoder::new();
    oversized.total_bytes = MAX_STREAM_BYTES;
    assert_eq!(
        oversized.push(b"x").unwrap_err().kind(),
        ProviderErrorKind::OutputTooLarge
    );
}

#[test]
fn request_translation_is_stateless_ordered_and_custom_tool_only() {
    let request = request("http://127.0.0.1:9/v1", true, rich_input());
    let body: Value = serde_json::from_slice(&encode_request(&request).unwrap()).unwrap();
    assert_eq!(body["model"], "fixture-openai-model");
    assert_eq!(body["store"], false);
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["truncation"], "disabled");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_output_tokens"], 4096);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert!(body.get("conversation").is_none());
    assert!(body.get("previous_response_id").is_none());
    assert!(body.get("background").is_none());
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"][0]["strict"], true);
    assert!(body["tools"][0].get("parameters").is_some());
    assert_eq!(body["input"].as_array().unwrap().len(), 7);
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][1]["role"], "assistant");
    assert_eq!(body["input"][2]["type"], "function_call");
    assert_eq!(body["input"][3]["type"], "function_call_output");
    assert_eq!(body["input"][6]["type"], "reasoning");
}

#[tokio::test]
async fn authenticated_local_stream_preserves_text_usage_and_provider_ids() {
    let server = FixtureServer::start(StatusCode::OK, text_response_sse()).await;
    let (provider, clock) = provider();
    let request = request(&server.endpoint(), false, vec![user("hello")]);
    let events = invoke_all(&provider, request, control(&clock, Duration::from_secs(1)))
        .await
        .unwrap();
    assert_eq!(server.calls(), 1);
    let capture = server.capture();
    assert_eq!(capture.authorization, format!("Bearer {SENTINEL}"));
    assert_eq!(capture.body["store"], false);
    assert_eq!(capture.body["parallel_tool_calls"], false);
    assert!(!format!("{provider:?}").contains(SENTINEL));
    assert!(matches!(events[0], ModelStreamEvent::ResponseStarted { .. }));
    assert!(matches!(events[1], ModelStreamEvent::TextDelta { .. }));
    assert!(matches!(events[2], ModelStreamEvent::TextDelta { .. }));
    let ModelStreamEvent::Usage(usage) = events[3] else {
        panic!("usage must precede completion")
    };
    assert_eq!(usage.input_tokens(), 11);
    assert_eq!(usage.cached_input_tokens(), 3);
    assert_eq!(usage.output_tokens(), 7);
    assert_eq!(usage.reasoning_tokens(), 2);
    let ModelStreamEvent::Completed(completed) = &events[4] else {
        panic!("complete response")
    };
    assert_eq!(completed.stop_reason(), ModelStopReason::Completed);
    assert_eq!(
        completed.provider_request_id().unwrap().as_str(),
        "request_fixture_1"
    );
    assert_eq!(
        completed.provider_response_id().unwrap().as_str(),
        "resp_fixture_1"
    );
    assert_eq!(completed.output_items().len(), 1);
    server.stop();
}

#[tokio::test]
async fn complete_function_call_is_emitted_only_after_all_arguments_and_item_done() {
    let server = FixtureServer::start(StatusCode::OK, tool_response_sse()).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&server.endpoint(), false, vec![user("read")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(matches!(events[1], ModelStreamEvent::ToolCallStarted { .. }));
    assert!(matches!(events[2], ModelStreamEvent::ToolArgumentDelta { .. }));
    assert!(matches!(events[3], ModelStreamEvent::ToolArgumentDelta { .. }));
    let ModelStreamEvent::ToolCallCompleted { call, .. } = &events[4] else {
        panic!("completed call is emitted only at item.done")
    };
    assert_eq!(call.raw_arguments(), r#"{"path":"Cargo.toml"}"#);
    assert!(call.arguments_are_valid_json());
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal response")
    };
    assert_eq!(response.stop_reason(), ModelStopReason::ToolContinuation);
    server.stop();
}

#[tokio::test]
async fn encrypted_reasoning_continuation_round_trips_as_provider_guarded_input() {
    let server = FixtureServer::start(StatusCode::OK, reasoning_response_sse()).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&server.endpoint(), true, vec![user("think")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal response")
    };
    let continuation = response.provider_continuation().unwrap().clone();
    assert_eq!(continuation.provider_id().as_str(), OPENAI_PROVIDER_ID);
    assert_eq!(continuation.type_label(), "openai.reasoning_items.v1");
    assert!(!format!("{continuation:?}").contains("encrypted-fixture"));
    let replay = request(
        "http://127.0.0.1:9/v1",
        true,
        vec![ModelInputItem::ProviderOpaqueContinuation(continuation)],
    );
    let body: Value = serde_json::from_slice(&encode_request(&replay).unwrap()).unwrap();
    assert_eq!(body["input"][0]["type"], "reasoning");
    assert_eq!(body["input"][0]["id"], "rs_fixture_1");
    assert_eq!(
        body["input"][0]["encrypted_content"],
        "encrypted-fixture"
    );
    server.stop();
}

#[tokio::test]
async fn unknown_correctness_event_fails_closed_with_bounded_evidence() {
    let events = vec![
        created(0, "resp_unknown"),
        json!({"type":"response.future_semantic.delta","sequence_number":1,"delta":"x"}),
    ];
    let server = FixtureServer::start(StatusCode::OK, sse(events)).await;
    let (provider, clock) = provider();
    let observed = invoke_all(
        &provider,
        request(&server.endpoint(), false, vec![user("unknown")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(matches!(
        observed[1],
        ModelStreamEvent::UnknownProviderEvent(_)
    ));
    assert!(matches!(
        observed[2],
        ModelStreamEvent::ProviderError {
            kind: crate::domain::ModelStreamProviderErrorKind::ProtocolFailure
        }
    ));
    server.stop();
}

#[tokio::test]
async fn reqwest_layer_does_not_hide_retries_and_retry_after_is_bounded() {
    let server = FixtureServer::start(StatusCode::TOO_MANY_REQUESTS, "limited".to_owned()).await;
    let (provider, clock) = provider();
    let error = match provider
        .invoke_stream(invocation(
            request(&server.endpoint(), false, vec![user("once")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
    {
        Ok(_) => panic!("429 cannot produce a stream"),
        Err(error) => error,
    };
    assert_eq!(server.calls(), 1);
    assert_eq!(error.kind(), ProviderErrorKind::RateLimited);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::DefiniteProviderFailure
    );
    assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
    server.stop();
}

#[tokio::test]
async fn bounded_provider_error_body_maps_safe_code_without_retaining_message() {
    let server = FixtureServer::start(
        StatusCode::BAD_REQUEST,
        json!({
            "error": {
                "code": "context_length_exceeded",
                "message": "sensitive-provider-message-sentinel"
            }
        })
        .to_string(),
    )
    .await;
    let (provider, clock) = provider();
    let error = match provider
        .invoke_stream(invocation(
            request(&server.endpoint(), false, vec![user("context")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
    {
        Ok(_) => panic!("context error cannot produce a stream"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ProviderErrorKind::ContextError);
    assert_eq!(error.to_string(), "context_error");
    assert!(!format!("{error:?}").contains("sensitive-provider-message"));
    server.stop();
}

#[tokio::test]
async fn disconnect_before_and_after_semantic_output_preserves_certainty() {
    let before = FixtureServer::start(StatusCode::OK, sse(vec![created(0, "resp_before")])).await;
    let (provider, clock) = provider();
    let mut stream = provider
        .invoke_stream(invocation(
            request(&before.endpoint(), false, vec![user("before")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(stream.next_event().await.unwrap().is_some());
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.certainty(), ProviderOutcomeCertainty::ProviderOutcomeUnknown);
    before.stop();

    let after = FixtureServer::start(
        StatusCode::OK,
        sse(vec![
            created(0, "resp_after"),
            item_added(1, 0, json!({"id":"msg_after","type":"message","status":"in_progress","role":"assistant","content":[]})),
            json!({"type":"response.output_text.delta","sequence_number":2,"output_index":0,"item_id":"msg_after","content_index":0,"delta":"partial"}),
        ]),
    )
    .await;
    let mut stream = provider
        .invoke_stream(invocation(
            request(&after.endpoint(), false, vec![user("after")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(stream.next_event().await.unwrap().is_some());
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::TextDelta { .. })
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::SemanticOutputObserved
    );
    after.stop();
}

#[tokio::test]
async fn cancellation_before_send_is_definitely_not_sent() {
    let (provider, clock) = provider();
    let token = ProviderCancellationToken::new();
    token.cancel();
    let control = ModelInvocationControl::try_new(
        token,
        MonotonicInstant::from_elapsed(Duration::from_secs(10)),
        Duration::from_secs(1),
    )
    .unwrap();
    let error = match provider
        .invoke_stream(invocation(
            request("http://127.0.0.1:9/v1", false, vec![user("cancel")]),
            control,
        ))
        .await
    {
        Ok(_) => panic!("pre-cancelled invocation cannot produce a stream"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::DefinitelyNotSent
    );
    drop(clock);
}

#[test]
fn conservative_estimator_is_byte_upper_bound_and_rejects_identity_mismatch() {
    let target = target("http://127.0.0.1:9/v1", false);
    let estimator = OpenAiConservativeEstimator::new(target.estimator().clone());
    let estimate = estimator
        .estimate(
            &target,
            &[
                TokenEstimateUnit::TextBytes(10),
                TokenEstimateUnit::StructuredBytes(20),
                TokenEstimateUnit::ToolDefinitionBytes(30),
                TokenEstimateUnit::ProviderOpaqueBytes(40),
            ],
        )
        .unwrap();
    assert_eq!(estimate.tokens(), 356);
    let other = OpenAiConservativeEstimator::new(
        TokenEstimatorIdentity::try_new("other", 1).unwrap(),
    );
    assert_eq!(
        other.estimate(&target, &[]).unwrap_err().kind(),
        ProviderErrorKind::InvalidRequest
    );
}

fn provider() -> (OpenAiProvider, Arc<TestClock>) {
    let clock = Arc::new(TestClock::new(
        time::OffsetDateTime::UNIX_EPOCH,
        Duration::ZERO,
    ));
    let clock_port: Arc<dyn Clock> = clock.clone();
    let provider = OpenAiProvider::try_new(
        BTreeMap::from([(
            "openai_fixture".to_owned(),
            SecretString::new(SENTINEL.to_owned()),
        )]),
        clock_port,
    )
    .unwrap();
    (provider, clock)
}

fn control(clock: &TestClock, idle: Duration) -> ModelInvocationControl {
    ModelInvocationControl::try_new(
        ProviderCancellationToken::new(),
        clock
            .monotonic_now()
            .checked_add(Duration::from_secs(10))
            .unwrap(),
        idle,
    )
    .unwrap()
}

fn invocation(request: ModelRequest, control: ModelInvocationControl) -> ModelProviderInvocation {
    ModelProviderInvocation {
        request,
        attempt: ProviderAttempt::try_new(1).unwrap(),
        control,
        fixture_key: None,
    }
}

async fn invoke_all(
    provider: &OpenAiProvider,
    request: ModelRequest,
    control: ModelInvocationControl,
) -> Result<Vec<ModelStreamEvent>, ProviderError> {
    let mut stream = provider.invoke_stream(invocation(request, control)).await?;
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(events)
}

fn request(endpoint: &str, reasoning: bool, input: Vec<ModelInputItem>) -> ModelRequest {
    let target = target(endpoint, reasoning);
    ModelRequest::try_new(ModelRequestInput {
        logical_invocation_id: LogicalInvocationId::generate(),
        target,
        ordered_input_items: input,
        instructions: vec![
            ModelTextPart::try_new("system instruction").unwrap(),
            ModelTextPart::try_new("developer instruction").unwrap(),
        ],
        tool_definitions: vec![ModelToolDefinition::try_new(
            ToolName::try_new("read_file").unwrap(),
            ToolVersion::try_new("1.0.0").unwrap(),
            SchemaVersion::try_new(1).unwrap(),
            "Read one file",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            }),
        )
        .unwrap()],
        requested_output_limit: TokenCount::try_new(4096).unwrap(),
        tool_choice_policy: ModelToolChoicePolicy::Automatic,
        provider_native_options: ProviderNativeOptions::new(reasoning),
        context_manifest_id: ContextManifestId::generate(),
    })
    .unwrap()
}

fn target(endpoint: &str, reasoning: bool) -> ModelTarget {
    let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: reasoning,
        context_window_tokens: TokenCount::try_new(128_000).unwrap(),
        max_output_tokens: TokenCount::try_new(16_384).unwrap(),
    });
    ModelTarget::try_new(ModelTargetInput {
        reference: ProviderModelReference::new(
            ModelTargetId::try_new("fixture").unwrap(),
            ProviderId::try_new(OPENAI_PROVIDER_ID).unwrap(),
            ProviderModelId::try_new("fixture-openai-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities,
        ),
        enabled: true,
        endpoint_reference: ModelConfigReference::endpoint(endpoint).unwrap(),
        account_reference: ModelConfigReference::named("openai_fixture").unwrap(),
        requested_output_tokens: TokenCount::try_new(4096).unwrap(),
        estimator: TokenEstimatorIdentity::try_new("conservative_v1", 1).unwrap(),
        provider_native_options: ProviderNativeOptions::new(reasoning),
    })
    .unwrap()
}

fn user(text: &str) -> ModelInputItem {
    ModelInputItem::message(
        ModelInputRole::User,
        vec![ModelTextPart::try_new(text).unwrap()],
    )
    .unwrap()
}

fn rich_input() -> Vec<ModelInputItem> {
    let call_id = ModelToolCallId::try_new("call_prior").unwrap();
    let continuation = ProviderOpaqueEvidence::try_new(
        ProviderId::try_new(OPENAI_PROVIDER_ID).unwrap(),
        "openai.reasoning_items.v1",
        r#"[{"type":"reasoning","id":"rs_prior","encrypted_content":"opaque-prior","summary":[]}]"#,
    )
    .unwrap();
    vec![
        user("hello"),
        ModelInputItem::prior_assistant(vec![ModelTextPart::try_new("prior").unwrap()]).unwrap(),
        ModelInputItem::ToolCall(
            CanonicalModelToolCall::try_new(call_id.clone(), "read_file", r#"{"path":"a"}"#)
                .unwrap(),
        ),
        ModelInputItem::tool_result(call_id, json!({"ok": true})).unwrap(),
        ModelInputItem::structured_data(json!({"stable": true})).unwrap(),
        ModelInputItem::synthetic_runtime_status("interrupted", json!({"certainty":"definite"}))
            .unwrap(),
        ModelInputItem::ProviderOpaqueContinuation(continuation),
    ]
}

fn created(sequence: u64, response_id: &str) -> Value {
    json!({
        "type": "response.created",
        "sequence_number": sequence,
        "response": {"id": response_id, "status": "in_progress", "output": []}
    })
}

fn item_added(sequence: u64, output_index: u32, item: Value) -> Value {
    json!({
        "type": "response.output_item.added",
        "sequence_number": sequence,
        "output_index": output_index,
        "item": item
    })
}

fn text_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-text.sse").to_owned()
}

fn tool_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-tool-call.sse").to_owned()
}

fn reasoning_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-reasoning.sse").to_owned()
}

fn sse(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|event| format!("event: {}\ndata: {}\n\n", event["type"].as_str().unwrap(), event))
        .collect()
}

#[derive(Clone)]
struct FixtureState {
    status: StatusCode,
    response: Arc<String>,
    calls: Arc<AtomicUsize>,
    capture: Arc<Mutex<Option<Capture>>>,
}

#[derive(Clone)]
struct Capture {
    authorization: String,
    body: Value,
}

struct FixtureServer {
    authority: String,
    state: FixtureState,
    task: tokio::task::JoinHandle<()>,
}

impl FixtureServer {
    async fn start(status: StatusCode, response: String) -> Self {
        let state = FixtureState {
            status,
            response: Arc::new(response),
            calls: Arc::new(AtomicUsize::new(0)),
            capture: Arc::new(Mutex::new(None)),
        };
        let app = Router::new()
            .route("/v1/responses", post(fixture_handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            authority,
            state,
            task,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1", self.authority)
    }

    fn calls(&self) -> usize {
        self.state.calls.load(Ordering::Acquire)
    }

    fn capture(&self) -> Capture {
        self.state.capture.lock().unwrap().clone().unwrap()
    }

    fn stop(self) {
        self.task.abort();
    }
}

async fn fixture_handler(
    State(state): State<FixtureState>,
    headers: AxumHeaderMap,
    body: axum::body::Bytes,
) -> Response<Body> {
    state.calls.fetch_add(1, Ordering::AcqRel);
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = serde_json::from_slice(&body).unwrap();
    *state.capture.lock().unwrap() = Some(Capture {
        authorization,
        body,
    });
    let mut response = Response::builder()
        .status(state.status)
        .header("x-request-id", "request_fixture_1");
    if state.status.is_success() {
        response = response.header("content-type", "text/event-stream; charset=utf-8");
    } else {
        response = response.header("retry-after", "120");
    }
    response.body(Body::from(state.response.as_str().to_owned())).unwrap()
}
