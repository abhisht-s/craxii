//! Opt-in, spend-bearing current-wire smoke test. Normal CI never runs this test.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use craxii_server::adapters::openai::OpenAiProvider;
use craxii_server::adapters::system_clock::SystemClock;
use craxii_server::bootstrap::secret::SecretString;
use craxii_server::domain::{
    ContextManifestId, LogicalInvocationId, ModelCapabilitySnapshot,
    ModelCapabilitySnapshotInput, ModelConfigReference, ModelInputItem, ModelInputRole,
    ModelOutputItem, ModelRequest, ModelRequestInput, ModelResponse, ModelStopReason,
    ModelStreamEvent, ModelTarget, ModelTargetId, ModelTargetInput, ModelTextPart,
    ModelToolChoicePolicy, ModelToolDefinition, ProviderId, ProviderModelId,
    ProviderModelReference, ProviderNativeOptions, SchemaVersion, TargetConfigurationVersion,
    TokenCount, TokenEstimatorIdentity, ToolName, ToolVersion,
};
use craxii_server::ports::clock::Clock;
use craxii_server::ports::model_provider::{
    ModelInvocationControl, ModelProvider, ModelProviderInvocation, ProviderAttempt,
    ProviderCancellationToken,
};
use serde_json::json;

#[tokio::test]
#[ignore = "requires explicit CRAXII_OPENAI_LIVE=1, a spend-limited key, and model"]
async fn live_stateless_custom_tool_round_trip() {
    assert_eq!(std::env::var("CRAXII_OPENAI_LIVE").as_deref(), Ok("1"));
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required");
    let model = std::env::var("CRAXII_OPENAI_MODEL").expect("CRAXII_OPENAI_MODEL is required");
    let endpoint = std::env::var("CRAXII_OPENAI_ENDPOINT")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
    let clock = Arc::new(SystemClock::new());
    let provider_clock: Arc<dyn Clock> = clock.clone();
    let provider = OpenAiProvider::try_new(
        BTreeMap::from([("live".to_owned(), SecretString::new(key))]),
        provider_clock,
    )
    .expect("live provider composition");
    let target = target(&endpoint, &model);
    let initial_user = user(
        "Call read_file exactly once for path stage19-live-fixture.txt. Do not guess its contents.",
    );
    let first = request(target.clone(), vec![initial_user.clone()]);
    let first_response = invoke(&provider, clock.as_ref(), first).await;
    assert_eq!(first_response.stop_reason(), ModelStopReason::ToolContinuation);
    assert!(first_response.provider_request_id().is_some());
    assert!(first_response.provider_response_id().is_some());
    assert!(first_response.usage().total_tokens() > 0);
    let calls = first_response
        .output_items()
        .iter()
        .filter_map(ModelOutputItem::tool_call)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name().as_str(), "read_file");
    calls[0].require_valid_arguments().unwrap();

    let mut full_history = vec![initial_user, ModelInputItem::ToolCall(calls[0].clone())];
    full_history.push(
        ModelInputItem::tool_result(
            calls[0].call_id().clone(),
            json!({
                "status": "success",
                "path": "stage19-live-fixture.txt",
                "content": "stage19-live-deterministic-marker"
            }),
        )
        .unwrap(),
    );
    let second = request(target, full_history);
    let final_response = invoke(&provider, clock.as_ref(), second).await;
    assert_eq!(final_response.stop_reason(), ModelStopReason::Completed);
    assert!(final_response.provider_request_id().is_some());
    assert!(final_response.provider_response_id().is_some());
    assert!(final_response.usage().total_tokens() > 0);
    assert!(final_response.output_items().iter().any(|item| {
        item.content_parts().is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part.as_str().contains("stage19-live-deterministic-marker"))
        })
    }));
    eprintln!(
        "STAGE_19_LIVE_SMOKE: PASS calls=2 tool_calls=1 stateless=true store=false"
    );
}

async fn invoke(
    provider: &OpenAiProvider,
    clock: &SystemClock,
    request: ModelRequest,
) -> ModelResponse {
    let control = ModelInvocationControl::try_new(
        ProviderCancellationToken::new(),
        clock
            .monotonic_now()
            .checked_add(Duration::from_secs(180))
            .unwrap(),
        Duration::from_secs(60),
    )
    .unwrap();
    let mut stream = provider
        .invoke_stream(ModelProviderInvocation {
            request,
            attempt: ProviderAttempt::try_new(1).unwrap(),
            control,
            fixture_key: None,
        })
        .await
        .expect("provider request");
    let mut completed = None;
    while let Some(event) = stream.next_event().await.expect("provider stream") {
        match event {
            ModelStreamEvent::Completed(response) => completed = Some(response),
            ModelStreamEvent::ProviderError { kind } => panic!("provider terminal error: {kind:?}"),
            ModelStreamEvent::UnknownProviderEvent(event) => {
                panic!("unsupported provider event: {}", event.type_label())
            }
            _ => {}
        }
    }
    completed.expect("complete provider response")
}

fn request(target: ModelTarget, ordered_input_items: Vec<ModelInputItem>) -> ModelRequest {
    ModelRequest::try_new(ModelRequestInput {
        logical_invocation_id: LogicalInvocationId::generate(),
        target,
        ordered_input_items,
        instructions: vec![ModelTextPart::try_new(
            "You are a deterministic integration test. Use only the supplied custom tool and report its result exactly.",
        )
        .unwrap()],
        tool_definitions: vec![ModelToolDefinition::try_new(
            ToolName::try_new("read_file").unwrap(),
            ToolVersion::try_new("1.0.0").unwrap(),
            SchemaVersion::try_new(1).unwrap(),
            "Read one UTF-8 file by logical path.",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": false
            }),
        )
        .unwrap()],
        requested_output_limit: TokenCount::try_new(1024).unwrap(),
        tool_choice_policy: ModelToolChoicePolicy::Automatic,
        provider_native_options: ProviderNativeOptions::new(false),
        context_manifest_id: ContextManifestId::generate(),
    })
    .unwrap()
}

fn target(endpoint: &str, model: &str) -> ModelTarget {
    ModelTarget::try_new(ModelTargetInput {
        reference: ProviderModelReference::new(
            ModelTargetId::try_new("live-openai").unwrap(),
            ProviderId::try_new("openai").unwrap(),
            ProviderModelId::try_new(model).expect("valid configured model ID"),
            TargetConfigurationVersion::try_new(1).unwrap(),
            ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
                text_input: true,
                text_output: true,
                custom_tool_calling: true,
                streaming: true,
                ordered_output_items: true,
                structured_output: true,
                reasoning_continuation: false,
                context_window_tokens: TokenCount::try_new(128_000).unwrap(),
                max_output_tokens: TokenCount::try_new(16_384).unwrap(),
            }),
        ),
        enabled: true,
        endpoint_reference: ModelConfigReference::endpoint(endpoint).expect("valid endpoint"),
        account_reference: ModelConfigReference::named("live").unwrap(),
        requested_output_tokens: TokenCount::try_new(1024).unwrap(),
        estimator: TokenEstimatorIdentity::try_new("conservative_v1", 1).unwrap(),
        provider_native_options: ProviderNativeOptions::new(false),
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
