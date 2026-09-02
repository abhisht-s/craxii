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
    let event = decoder.pop_event().unwrap();
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
    assert!(body.get("include").is_none());
    assert!(body.get("conversation").is_none());
    assert!(body.get("previous_response_id").is_none());
    assert!(body.get("background").is_none());
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"][0]["strict"], false);
    assert!(body["tools"][0].get("parameters").is_some());
    assert_eq!(body["input"].as_array().unwrap().len(), 11);
    assert_eq!(body["input"][0]["role"], "system");
    assert_eq!(body["input"][1]["role"], "developer");
    assert_eq!(body["input"][2]["role"], "user");
    assert_eq!(body["input"][3]["role"], "assistant");
    assert_eq!(body["input"][4]["type"], "function_call");
    assert_eq!(body["input"][5]["type"], "function_call_output");
    assert_eq!(body["input"][6]["content"][0]["type"], "refusal");
    assert_eq!(body["input"][7]["role"], "developer");
    assert_eq!(body["input"][10]["type"], "reasoning");
}

#[test]
fn request_translation_rejects_ineligible_opaque_state_before_network_io() {
    let continuation = ProviderOpaqueEvidence::try_new(
        ProviderId::try_new(OPENAI_PROVIDER_ID).unwrap(),
        "openai.reasoning_items.v1",
        r#"[{"type":"reasoning","id":"rs","encrypted_content":"opaque","summary":[]}]"#,
    )
    .unwrap();
    let disabled = request(
        "http://127.0.0.1:9/v1",
        false,
        vec![ModelInputItem::ProviderOpaqueContinuation(continuation)],
    );
    assert_eq!(
        encode_request(&disabled).unwrap_err().kind(),
        ProviderErrorKind::InvalidRequest
    );

    let user_json = request(
        "http://127.0.0.1:9/v1",
        false,
        vec![user(r#"{"looks":"structured"}"#)],
    );
    let body: Value = serde_json::from_slice(&encode_request(&user_json).unwrap()).unwrap();
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(
        body["input"][0]["content"][0]["text"],
        r#"{"looks":"structured"}"#
    );
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
    assert!(capture.authorization_present);
    assert!(capture.client_request_id_present);
    assert_eq!(capture.body["store"], false);
    assert_eq!(capture.body["parallel_tool_calls"], false);
    assert!(!format!("{provider:?}").contains(SENTINEL));
    assert!(matches!(
        events[0],
        ModelStreamEvent::ResponseStarted { .. }
    ));
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
async fn raw_http_transport_handles_fragmented_chunks_and_exact_request_shape() {
    let body = text_response_sse();
    let server = RawFixtureServer::start(body, vec![1, 3, 11, 2, 97]).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&server.endpoint(), false, vec![user("fragment")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(matches!(
        events.last(),
        Some(ModelStreamEvent::Completed(_))
    ));
    let capture = server.finish().await;
    assert_eq!(capture.method_and_path, "POST /v1/responses");
    assert!(capture.authorization_present);
    assert!(capture.client_request_id_present);
    assert_eq!(capture.body["store"], false);
    assert_eq!(capture.body["parallel_tool_calls"], false);
    assert!(capture.body.get("conversation").is_none());
    assert!(capture.body.get("previous_response_id").is_none());
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
    assert!(matches!(
        events[1],
        ModelStreamEvent::ToolCallStarted { .. }
    ));
    assert!(matches!(
        events[2],
        ModelStreamEvent::ToolArgumentDelta { .. }
    ));
    assert!(matches!(
        events[3],
        ModelStreamEvent::ToolArgumentDelta { .. }
    ));
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
async fn mixed_text_and_tool_preserve_provider_order() {
    let server = FixtureServer::start(StatusCode::OK, mixed_response_sse()).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&server.endpoint(), false, vec![user("mixed")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal response")
    };
    assert_eq!(response.stop_reason(), ModelStopReason::ToolContinuation);
    assert!(matches!(
        response.output_items()[0],
        ModelOutputItem::Text { .. }
    ));
    assert!(matches!(
        response.output_items()[1],
        ModelOutputItem::ToolCall(_)
    ));
    server.stop();
}

#[tokio::test]
async fn multipart_text_is_reconstructed_by_content_index_without_reordering() {
    let message = json!({
        "id":"msg_multipart_1",
        "type":"message",
        "status":"completed",
        "role":"assistant",
        "content":[
            {"type":"output_text","text":"first"},
            {"type":"output_text","text":"second"}
        ]
    });
    let response = sse(vec![
        created(0, "resp_multipart_1"),
        item_added(
            1,
            0,
            json!({"id":"msg_multipart_1","type":"message","status":"in_progress","role":"assistant","content":[]}),
        ),
        json!({"type":"response.output_text.delta","sequence_number":2,"output_index":0,"item_id":"msg_multipart_1","content_index":0,"delta":"first"}),
        json!({"type":"response.output_text.done","sequence_number":3,"output_index":0,"item_id":"msg_multipart_1","content_index":0,"text":"first"}),
        json!({"type":"response.output_text.delta","sequence_number":4,"output_index":0,"item_id":"msg_multipart_1","content_index":1,"delta":"second"}),
        json!({"type":"response.output_text.done","sequence_number":5,"output_index":0,"item_id":"msg_multipart_1","content_index":1,"text":"second"}),
        json!({"type":"response.output_item.done","sequence_number":6,"output_index":0,"item":message.clone()}),
        json!({"type":"response.completed","sequence_number":7,"response":{"id":"resp_multipart_1","status":"completed","model":"fixture-openai-model","output":[message],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}),
    ]);
    let server = FixtureServer::start(StatusCode::OK, response).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&server.endpoint(), false, vec![user("multipart")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal multipart response")
    };
    let ModelOutputItem::Text { content_parts } = &response.output_items()[0] else {
        panic!("multipart text output")
    };
    assert_eq!(content_parts[0].as_str(), "first");
    assert_eq!(content_parts[1].as_str(), "second");
    server.stop();
}

#[tokio::test]
async fn refusal_and_incomplete_are_normalized_without_inventing_usage() {
    let refusal_server = FixtureServer::start(StatusCode::OK, refusal_response_sse()).await;
    let (provider, clock) = provider();
    let events = invoke_all(
        &provider,
        request(&refusal_server.endpoint(), false, vec![user("refuse")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal refusal")
    };
    assert_eq!(response.stop_reason(), ModelStopReason::Refusal);
    assert!(matches!(
        response.output_items()[0],
        ModelOutputItem::Refusal { .. }
    ));
    refusal_server.stop();

    let incomplete_server = FixtureServer::start(StatusCode::OK, incomplete_response_sse()).await;
    let events = invoke_all(
        &provider,
        request(&incomplete_server.endpoint(), false, vec![user("long")]),
        control(&clock, Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(matches!(
        events[events.len() - 2],
        ModelStreamEvent::UsageUnavailable
    ));
    let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
        panic!("terminal incomplete")
    };
    assert_eq!(
        response.stop_reason(),
        ModelStopReason::IncompleteProviderLimit
    );
    assert_eq!(response.usage(), None);
    incomplete_server.stop();

    let filtered_server = FixtureServer::start(
        StatusCode::OK,
        incomplete_response_sse().replace("max_output_tokens", "content_filter"),
    )
    .await;
    let mut stream = provider
        .invoke_stream(invocation(
            request(&filtered_server.endpoint(), false, vec![user("filtered")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::ResponseStarted { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::TextDelta { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::UsageUnavailable)
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::SafetyRefusal);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::SemanticOutputObserved
    );
    filtered_server.stop();
}

#[tokio::test]
async fn contradictory_terminal_echo_retains_reported_usage_then_fails_closed() {
    let response = text_response_sse().replace(
        "\"service_tier\":\"default\",\"output\"",
        "\"store\":true,\"service_tier\":\"default\",\"output\"",
    );
    let server = FixtureServer::start(StatusCode::OK, response).await;
    let (provider, clock) = provider();
    let mut stream = provider
        .invoke_stream(invocation(
            request(&server.endpoint(), false, vec![user("contradict")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::ResponseStarted { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::TextDelta { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::TextDelta { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::Usage(_))
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::MalformedResponse);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::SemanticOutputObserved
    );
    server.stop();
}

#[tokio::test]
async fn failed_terminal_retains_usage_and_returns_a_classified_error() {
    let server = FixtureServer::start(StatusCode::OK, failed_response_sse()).await;
    let (provider, clock) = provider();
    let mut stream = provider
        .invoke_stream(invocation(
            request(&server.endpoint(), false, vec![user("fail")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::ResponseStarted { .. })
    ));
    let Some(ModelStreamEvent::Usage(usage)) = stream.next_event().await.unwrap() else {
        panic!("reported failure usage")
    };
    assert_eq!(usage.total_tokens(), 4);
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::TemporarilyUnavailable);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::DefiniteProviderFailure
    );
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
    assert!(
        response
            .output_items()
            .iter()
            .any(|item| { matches!(item, ModelOutputItem::ReasoningSummary { .. }) })
    );
    assert!(
        response
            .output_items()
            .iter()
            .all(|item| !matches!(item, ModelOutputItem::ProviderOpaque(_)))
    );
    let replay = request(
        "http://127.0.0.1:9/v1",
        true,
        vec![ModelInputItem::ProviderOpaqueContinuation(continuation)],
    );
    let body: Value = serde_json::from_slice(&encode_request(&replay).unwrap()).unwrap();
    assert_eq!(body["input"][0]["type"], "reasoning");
    assert_eq!(body["input"][0]["id"], "rs_fixture_1");
    assert_eq!(body["input"][0]["encrypted_content"], "encrypted-fixture");
    server.stop();
}

#[tokio::test]
async fn unknown_correctness_event_fails_closed_with_bounded_evidence() {
    let server = FixtureServer::start(
        StatusCode::OK,
        include_str!("../../../tests/fixtures/openai/unknown-event.sse").to_owned(),
    )
    .await;
    let (provider, clock) = provider();
    let mut stream = provider
        .invoke_stream(invocation(
            request(&server.endpoint(), false, vec![user("unknown")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::ResponseStarted { .. })
    ));
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::UnknownProviderEvent(_))
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::UnsupportedResponseItem);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::SemanticOutputObserved
    );
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

#[derive(serde::Deserialize)]
struct HttpErrorFixture {
    status: u16,
    code: String,
    expected: String,
    retry_after: Option<String>,
}

#[tokio::test]
async fn official_http_error_classes_and_quota_are_conservative() {
    let fixtures: Vec<HttpErrorFixture> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/openai/http-errors.json"
    ))
    .unwrap();
    let (provider, clock) = provider();
    for fixture in fixtures {
        let status = StatusCode::from_u16(fixture.status).unwrap();
        let server = FixtureServer::start_with_retry_after(
            status,
            json!({"error":{"code":fixture.code,"message":"redacted"}}).to_string(),
            fixture.retry_after.as_deref(),
        )
        .await;
        let error = match provider
            .invoke_stream(invocation(
                request(&server.endpoint(), false, vec![user("error")]),
                control(&clock, Duration::from_secs(1)),
            ))
            .await
        {
            Ok(_) => panic!("HTTP error cannot produce a stream"),
            Err(error) => error,
        };
        let expected = match fixture.expected.as_str() {
            "authentication" => ProviderErrorKind::Authentication,
            "authorization" => ProviderErrorKind::Authorization,
            "invalid_request" => ProviderErrorKind::InvalidRequest,
            "unknown_model" => ProviderErrorKind::UnknownModel,
            "rate_limited" => ProviderErrorKind::RateLimited,
            "temporarily_unavailable" => ProviderErrorKind::TemporarilyUnavailable,
            _ => panic!("unknown fixture classification"),
        };
        assert_eq!(error.kind(), expected);
        assert_eq!(
            error.certainty(),
            ProviderOutcomeCertainty::DefiniteProviderFailure
        );
        if expected == ProviderErrorKind::RateLimited {
            assert_eq!(error.retry_after(), Some(Duration::from_secs(2)));
        }
        assert_eq!(server.calls(), 1);
        server.stop();
    }
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
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::ProviderOutcomeUnknown
    );
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
async fn malformed_json_unsupported_items_and_premature_eof_fail_closed() {
    let (provider, clock) = provider();

    let malformed = FixtureServer::start(
        StatusCode::OK,
        include_str!("../../../tests/fixtures/openai/malformed-json.sse").to_owned(),
    )
    .await;
    let mut stream = provider
        .invoke_stream(invocation(
            request(&malformed.endpoint(), false, vec![user("malformed")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::MalformedResponse);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::ProviderOutcomeUnknown
    );
    malformed.stop();

    let premature = FixtureServer::start(
        StatusCode::OK,
        include_str!("../../../tests/fixtures/openai/premature-eof.sse").to_owned(),
    )
    .await;
    let mut stream = provider
        .invoke_stream(invocation(
            request(&premature.endpoint(), false, vec![user("eof")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::ResponseStarted { .. })
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::MalformedResponse);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::ProviderOutcomeUnknown
    );
    premature.stop();

    let unsupported = FixtureServer::start(
        StatusCode::OK,
        include_str!("../../../tests/fixtures/openai/unsupported-output-item.sse").to_owned(),
    )
    .await;
    let mut stream = provider
        .invoke_stream(invocation(
            request(&unsupported.endpoint(), false, vec![user("unsupported")]),
            control(&clock, Duration::from_secs(1)),
        ))
        .await
        .unwrap();
    assert!(stream.next_event().await.unwrap().is_some());
    assert!(matches!(
        stream.next_event().await.unwrap(),
        Some(ModelStreamEvent::UnknownProviderEvent(_))
    ));
    let error = stream.next_event().await.unwrap_err();
    assert_eq!(error.kind(), ProviderErrorKind::UnsupportedResponseItem);
    unsupported.stop();
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

#[tokio::test]
async fn cancellation_and_timeout_after_http_request_are_outcome_unknown_and_never_retried() {
    let (provider, clock) = provider();
    let provider = Arc::new(provider);

    let mut cancelled_server = HangingServer::start().await;
    let token = ProviderCancellationToken::new();
    let cancellation_control = ModelInvocationControl::try_new(
        token.clone(),
        clock
            .monotonic_now()
            .checked_add(Duration::from_secs(2))
            .unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    let cancelled_provider = Arc::clone(&provider);
    let cancelled_request = request(
        &cancelled_server.endpoint(),
        false,
        vec![user("cancel pending")],
    );
    let cancelled = tokio::spawn(async move {
        cancelled_provider
            .invoke_stream(invocation(cancelled_request, cancellation_control))
            .await
            .err()
            .unwrap()
    });
    cancelled_server.wait_received().await;
    token.cancel();
    let error = cancelled.await.unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::Cancelled);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::ProviderOutcomeUnknown
    );
    assert_eq!(cancelled_server.calls(), 1);
    cancelled_server.stop();

    let mut timeout_server = HangingServer::start().await;
    let timeout_control = ModelInvocationControl::try_new(
        ProviderCancellationToken::new(),
        clock
            .monotonic_now()
            .checked_add(Duration::from_millis(30))
            .unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    let timeout_provider = Arc::clone(&provider);
    let timeout_request = request(
        &timeout_server.endpoint(),
        false,
        vec![user("timeout pending")],
    );
    let timed_out = tokio::spawn(async move {
        timeout_provider
            .invoke_stream(invocation(timeout_request, timeout_control))
            .await
            .err()
            .unwrap()
    });
    timeout_server.wait_received().await;
    let error = timed_out.await.unwrap();
    assert_eq!(error.kind(), ProviderErrorKind::TimeoutBeforeOutput);
    assert_eq!(
        error.certainty(),
        ProviderOutcomeCertainty::ProviderOutcomeUnknown
    );
    assert_eq!(timeout_server.calls(), 1);
    timeout_server.stop();
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
    let other =
        OpenAiConservativeEstimator::new(TokenEstimatorIdentity::try_new("other", 1).unwrap());
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
        tool_definitions: vec![
            ModelToolDefinition::try_new(
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
            .unwrap(),
        ],
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
        structured_output: false,
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
        ModelInputItem::message(
            ModelInputRole::System,
            vec![ModelTextPart::try_new("system history").unwrap()],
        )
        .unwrap(),
        ModelInputItem::message(
            ModelInputRole::Developer,
            vec![ModelTextPart::try_new("developer history").unwrap()],
        )
        .unwrap(),
        user("hello"),
        ModelInputItem::prior_assistant(vec![ModelTextPart::try_new("prior").unwrap()]).unwrap(),
        ModelInputItem::ToolCall(
            CanonicalModelToolCall::try_new(call_id.clone(), "read_file", r#"{"path":"a"}"#)
                .unwrap(),
        ),
        ModelInputItem::tool_result(call_id, json!({"ok": true})).unwrap(),
        ModelInputItem::historical_refusal(vec![
            ModelTextPart::try_new("historical refusal").unwrap(),
        ])
        .unwrap(),
        ModelInputItem::historical_reasoning_summary(vec![
            ModelTextPart::try_new("historical summary").unwrap(),
        ])
        .unwrap(),
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

fn mixed_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-mixed.sse").to_owned()
}

fn refusal_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-refusal.sse").to_owned()
}

fn incomplete_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-incomplete.sse").to_owned()
}

fn failed_response_sse() -> String {
    include_str!("../../../tests/fixtures/openai/responses-failed.sse").to_owned()
}

fn sse(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {}\n\n",
                event["type"].as_str().unwrap(),
                event
            )
        })
        .collect()
}

#[derive(Clone)]
struct FixtureState {
    status: StatusCode,
    response: Arc<String>,
    retry_after: Option<Arc<String>>,
    calls: Arc<AtomicUsize>,
    capture: Arc<Mutex<Option<Capture>>>,
}

#[derive(Clone)]
struct Capture {
    authorization_present: bool,
    client_request_id_present: bool,
    body: Value,
}

struct FixtureServer {
    authority: String,
    state: FixtureState,
    task: tokio::task::JoinHandle<()>,
}

struct RawCapture {
    method_and_path: String,
    authorization_present: bool,
    client_request_id_present: bool,
    body: Value,
}

struct RawFixtureServer {
    authority: String,
    task: tokio::task::JoinHandle<RawCapture>,
}

struct HangingServer {
    authority: String,
    calls: Arc<AtomicUsize>,
    received: Option<tokio::sync::oneshot::Receiver<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl HangingServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let calls = Arc::new(AtomicUsize::new(0));
        let task_calls = Arc::clone(&calls);
        let (sender, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            task_calls.fetch_add(1, Ordering::AcqRel);
            let _request = read_http_request(&mut socket).await;
            let _ = sender.send(());
            std::future::pending::<()>().await;
        });
        Self {
            authority,
            calls,
            received: Some(received),
            task,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1", self.authority)
    }

    async fn wait_received(&mut self) {
        self.received.take().unwrap().await.unwrap();
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }

    fn stop(self) {
        self.task.abort();
    }
}

impl RawFixtureServer {
    async fn start(response_body: String, fragment_sizes: Vec<usize>) -> Self {
        use tokio::io::AsyncWriteExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            let (headers, body) = split_http_request(&request);
            let request_line = headers.lines().next().unwrap();
            let mut words = request_line.split_whitespace();
            let method_and_path = format!(
                "{} {}",
                words.next().unwrap_or_default(),
                words.next().unwrap_or_default()
            );
            let lower_headers = headers.to_ascii_lowercase();
            let capture = RawCapture {
                method_and_path,
                authorization_present: lower_headers
                    .lines()
                    .any(|line| line.starts_with("authorization: bearer ")),
                client_request_id_present: lower_headers
                    .lines()
                    .any(|line| line.starts_with("x-client-request-id: ")),
                body: serde_json::from_slice(body).unwrap(),
            };
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-request-id: request_raw_1\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let bytes = response_body.as_bytes();
            let mut offset = 0;
            let mut size_index = 0;
            while offset < bytes.len() {
                let desired = fragment_sizes
                    .get(size_index)
                    .copied()
                    .unwrap_or(bytes.len() - offset);
                size_index += 1;
                let end = offset.saturating_add(desired).min(bytes.len());
                let chunk = &bytes[offset..end];
                socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .unwrap();
                socket.write_all(chunk).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                tokio::task::yield_now().await;
                offset = end;
            }
            socket.write_all(b"0\r\n\r\n").await.unwrap();
            capture
        });
        Self { authority, task }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1", self.authority)
    }

    async fn finish(self) -> RawCapture {
        self.task.await.unwrap()
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    use tokio::io::AsyncReadExt as _;

    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0);
        request.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = find_header_end(&request) {
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if request.len() >= header_end + 4 + length {
                return request;
            }
        }
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_http_request(request: &[u8]) -> (&str, &[u8]) {
    let end = find_header_end(request).unwrap();
    (
        std::str::from_utf8(&request[..end]).unwrap(),
        &request[end + 4..],
    )
}

impl FixtureServer {
    async fn start(status: StatusCode, response: String) -> Self {
        Self::start_with_retry_after(status, response, (!status.is_success()).then_some("120"))
            .await
    }

    async fn start_with_retry_after(
        status: StatusCode,
        response: String,
        retry_after: Option<&str>,
    ) -> Self {
        let state = FixtureState {
            status,
            response: Arc::new(response),
            retry_after: retry_after.map(|value| Arc::new(value.to_owned())),
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
    let authorization_present = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("Bearer "));
    let client_request_id_present = headers
        .get("x-client-request-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.is_empty() && value.len() <= 512 && value.is_ascii());
    let body = serde_json::from_slice(&body).unwrap();
    *state.capture.lock().unwrap() = Some(Capture {
        authorization_present,
        client_request_id_present,
        body,
    });
    let mut response = Response::builder()
        .status(state.status)
        .header("x-request-id", "request_fixture_1");
    if state.status.is_success() {
        response = response.header("content-type", "text/event-stream; charset=utf-8");
    } else if let Some(retry_after) = &state.retry_after {
        response = response.header("retry-after", retry_after.as_str());
    }
    response
        .body(Body::from(state.response.as_str().to_owned()))
        .unwrap()
}
