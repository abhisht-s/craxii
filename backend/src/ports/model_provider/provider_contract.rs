//! Reusable provider contract harness. Fixtures may configure adapters, but assertions use only
//! the public provider-neutral `ModelProvider` boundary.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::{
    ModelCapabilitySnapshot, ModelOutputItem, ModelStreamEvent, ModelStreamProviderErrorKind,
    ModelStreamState, validate_model_stream,
};

use super::{ModelProvider, ModelProviderInvocation, ProviderErrorKind, ProviderOutcomeCertainty};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ProviderContractScenario {
    OrderedTextAndTools,
    Refusal,
    StructuredReasoningOpaque,
    UnknownItemFailClosed,
    OutputItemLimit,
    ArgumentLimit,
    Cancellation,
    AbsoluteDeadline,
    IdleTimeout,
    AuthenticationFailure,
    OutcomeUnknown,
    TimeoutAfterSemanticOutput,
    MalformedResponse,
}

pub(crate) const PROVIDER_CONTRACT_SCENARIOS: [ProviderContractScenario; 13] = [
    ProviderContractScenario::OrderedTextAndTools,
    ProviderContractScenario::Refusal,
    ProviderContractScenario::StructuredReasoningOpaque,
    ProviderContractScenario::UnknownItemFailClosed,
    ProviderContractScenario::OutputItemLimit,
    ProviderContractScenario::ArgumentLimit,
    ProviderContractScenario::Cancellation,
    ProviderContractScenario::AbsoluteDeadline,
    ProviderContractScenario::IdleTimeout,
    ProviderContractScenario::AuthenticationFailure,
    ProviderContractScenario::OutcomeUnknown,
    ProviderContractScenario::TimeoutAfterSemanticOutput,
    ProviderContractScenario::MalformedResponse,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderContractErrorStage {
    Invoke,
    Stream,
}

pub(crate) enum ProviderContractExpected {
    Events(Vec<ModelStreamEvent>),
    Error {
        stage: ProviderContractErrorStage,
        kind: ProviderErrorKind,
        certainty: ProviderOutcomeCertainty,
        semantic_output_observed: bool,
    },
}

pub(crate) struct ProviderContractCase {
    pub provider: Arc<dyn ModelProvider>,
    pub invocation: ModelProviderInvocation,
    pub expected_capabilities: ModelCapabilitySnapshot,
    pub expected: ProviderContractExpected,
    /// A fixture-only deterministic action run before requesting the indexed next event.
    pub before_next: BTreeMap<usize, Arc<dyn Fn() + Send + Sync>>,
}

pub(crate) trait ModelProviderContractFixture {
    fn build_case(&self, scenario: ProviderContractScenario) -> ProviderContractCase;
}

pub(crate) async fn assert_model_provider_contract(
    fixture: &dyn ModelProviderContractFixture,
) -> usize {
    for scenario in PROVIDER_CONTRACT_SCENARIOS {
        assert_case(fixture.build_case(scenario), scenario).await;
    }
    PROVIDER_CONTRACT_SCENARIOS.len()
}

async fn assert_case(mut case: ProviderContractCase, scenario: ProviderContractScenario) {
    let target = case.invocation.request.target();
    assert_eq!(
        case.provider.provider_id(),
        target.reference().provider_id(),
        "{scenario:?}: selected provider identity"
    );
    assert_eq!(
        case.provider.capabilities(target).unwrap(),
        case.expected_capabilities,
        "{scenario:?}: capabilities"
    );
    let request_hash = case.invocation.request.canonical_sha256();
    assert_eq!(
        request_hash,
        case.invocation.request.canonical_sha256(),
        "{scenario:?}: stable request fixture correlation"
    );
    assert!(!case.invocation.request.parallel_tool_calls());
    assert!(
        case.invocation
            .fixture_key
            .as_deref()
            .is_some_and(|key| !key.is_empty()),
        "{scenario:?}: fixture correlation key"
    );

    let stream = case.provider.invoke_stream(case.invocation).await;
    let mut stream = match (stream, &case.expected) {
        (
            Err(error),
            ProviderContractExpected::Error {
                stage: ProviderContractErrorStage::Invoke,
                kind,
                certainty,
                semantic_output_observed,
            },
        ) => {
            assert_provider_error(
                &error,
                *kind,
                *certainty,
                *semantic_output_observed,
                scenario,
            );
            return;
        }
        (Err(error), _) => panic!("{scenario:?}: unexpected invoke error: {error:?}"),
        (
            Ok(_),
            ProviderContractExpected::Error {
                stage: ProviderContractErrorStage::Invoke,
                ..
            },
        ) => {
            panic!("{scenario:?}: expected invoke error")
        }
        (Ok(stream), _) => stream,
    };

    let mut events = Vec::new();
    loop {
        if let Some(action) = case.before_next.remove(&events.len()) {
            action();
        }
        match stream.next_event().await {
            Ok(Some(event)) => events.push(event),
            Ok(None) => break,
            Err(error) => match &case.expected {
                ProviderContractExpected::Error {
                    stage: ProviderContractErrorStage::Stream,
                    kind,
                    certainty,
                    semantic_output_observed,
                } => {
                    assert_provider_error(
                        &error,
                        *kind,
                        *certainty,
                        *semantic_output_observed,
                        scenario,
                    );
                    assert_eq!(
                        events.iter().any(ModelStreamEvent::is_semantic_output),
                        *semantic_output_observed,
                        "{scenario:?}: shared semantic-output predicate"
                    );
                    return;
                }
                _ => panic!("{scenario:?}: unexpected stream error: {error:?}"),
            },
        }
    }

    let ProviderContractExpected::Events(expected) = case.expected else {
        panic!("{scenario:?}: expected provider error")
    };
    assert_eq!(events, expected, "{scenario:?}: exact event order");
    let state = validate_model_stream(&events).expect("contract stream must validate");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModelStreamEvent::ResponseStarted { .. }))
            .count(),
        1,
        "{scenario:?}: exactly one started event"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                ModelStreamEvent::Usage(_) | ModelStreamEvent::UsageUnavailable
            ))
            .count(),
        1,
        "{scenario:?}: exactly one usage result"
    );
    assert_eq!(
        events.iter().filter(|event| event.is_terminal()).count(),
        1,
        "{scenario:?}: exactly one terminal event"
    );
    assert!(events.last().is_some_and(ModelStreamEvent::is_terminal));
    assert_scenario_semantics(scenario, &events, state);
}

fn assert_provider_error(
    error: &super::ProviderError,
    kind: ProviderErrorKind,
    certainty: ProviderOutcomeCertainty,
    semantic_output_observed: bool,
    scenario: ProviderContractScenario,
) {
    assert_eq!(error.kind(), kind, "{scenario:?}: error kind");
    assert_eq!(error.certainty(), certainty, "{scenario:?}: certainty");
    let expected_terminal = match kind {
        ProviderErrorKind::Cancelled => ModelStreamProviderErrorKind::Cancelled,
        ProviderErrorKind::ProviderOutcomeUnknown
        | ProviderErrorKind::TransportAfterPossibleProcessing => {
            ModelStreamProviderErrorKind::OutcomeUnknown
        }
        ProviderErrorKind::TimeoutBeforeOutput => ModelStreamProviderErrorKind::TimeoutBeforeOutput,
        ProviderErrorKind::TimeoutAfterOutput => {
            assert!(semantic_output_observed);
            ModelStreamProviderErrorKind::TimeoutAfterOutput
        }
        ProviderErrorKind::MalformedResponse
        | ProviderErrorKind::MalformedCompletedToolArguments
        | ProviderErrorKind::OutputTooLarge
        | ProviderErrorKind::UnsupportedResponseItem => {
            ModelStreamProviderErrorKind::ProtocolFailure
        }
        ProviderErrorKind::RateLimited
        | ProviderErrorKind::TemporarilyUnavailable
        | ProviderErrorKind::TransportBeforeResponse => {
            ModelStreamProviderErrorKind::TransientUnavailable
        }
        _ => ModelStreamProviderErrorKind::DefiniteFailure,
    };
    assert_eq!(
        error.stream_terminal_kind(),
        expected_terminal,
        "{scenario:?}: terminal mapping"
    );
}

fn assert_scenario_semantics(
    scenario: ProviderContractScenario,
    events: &[ModelStreamEvent],
    state: ModelStreamState,
) {
    let completed_response = events.iter().find_map(|event| match event {
        ModelStreamEvent::Completed(response) => Some(response),
        _ => None,
    });
    match scenario {
        ProviderContractScenario::OrderedTextAndTools => {
            assert_eq!(state, ModelStreamState::Completed);
            let response = completed_response.unwrap();
            let calls = response
                .output_items()
                .iter()
                .filter_map(|item| match item {
                    ModelOutputItem::ToolCall(call) => Some((
                        call.call_id().as_str(),
                        call.name().as_str(),
                        call.raw_arguments(),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(calls.len(), 2);
            assert_ne!(calls[0].0, calls[1].0);
            assert_eq!(calls[0].2, "{\"path\":\"a\"}");
            assert_eq!(calls[1].2, "{\"path\":\"b\"}");
            assert!(matches!(
                response.output_items()[0],
                ModelOutputItem::Text { .. }
            ));
        }
        ProviderContractScenario::Refusal => {
            assert!(matches!(
                completed_response.unwrap().output_items(),
                [ModelOutputItem::Refusal { .. }]
            ));
        }
        ProviderContractScenario::StructuredReasoningOpaque => {
            let items = completed_response.unwrap().output_items();
            assert!(matches!(items[0], ModelOutputItem::StructuredData { .. }));
            assert!(matches!(items[1], ModelOutputItem::ReasoningSummary { .. }));
            assert!(matches!(items[2], ModelOutputItem::ProviderOpaque(_)));
        }
        ProviderContractScenario::UnknownItemFailClosed => {
            assert_eq!(state, ModelStreamState::ProtocolFailure);
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, ModelStreamEvent::UnknownProviderEvent(_)))
            );
        }
        _ => panic!("{scenario:?}: error scenario unexpectedly emitted a complete stream"),
    }
}
