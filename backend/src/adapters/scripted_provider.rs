//! Deterministic, network-free Stage 15 provider adapter for permanent contract tests.

use std::collections::{BTreeSet, VecDeque};
use std::fmt::{self, Formatter};
use std::sync::{Arc, Mutex};

use crate::adapters::system_clock::SystemClock;
use crate::domain::{
    ModelCapabilitySnapshot, ModelInputItem, ModelRequest, ModelStreamEvent,
    ModelStreamProviderErrorKind, ModelTarget, ModelTargetId, ModelToolCallId, ProviderId,
    Sha256Digest,
};
use crate::ports::clock::{Clock, MonotonicInstant};
use crate::ports::model_provider::{
    ConservativeTokenEstimate, ModelProvider, ModelProviderFuture, ModelProviderInvocation,
    ModelProviderStream, ProviderAttempt, ProviderError, ProviderErrorKind,
    ProviderOutcomeCertainty, TokenEstimateUnit, TokenEstimator,
};

/// Deterministic release barrier used instead of sleeps or wall-clock timing.
#[derive(Clone, Debug)]
pub struct ScriptGate {
    sender: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for ScriptGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptGate {
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = tokio::sync::watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }

    pub fn release(&self) {
        self.sender.send_replace(true);
    }

    async fn wait(&self) {
        if *self.sender.borrow() {
            return;
        }
        let mut receiver = self.sender.subscribe();
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
    }
}

/// One deterministic stream action. No action sleeps or performs I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScriptedStep {
    Emit(Box<ModelStreamEvent>),
    Fail(ProviderError),
    AwaitRelease(ScriptGate),
}

impl ScriptedStep {
    #[must_use]
    pub fn emit(event: ModelStreamEvent) -> Self {
        Self::Emit(Box::new(event))
    }
}

impl PartialEq for ScriptGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.sender, &other.sender)
    }
}

impl Eq for ScriptGate {}

/// Expected invocation identity for one queued deterministic program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptExpectation {
    pub target_id: ModelTargetId,
    pub request_sha256: Option<Sha256Digest>,
    pub fixture_key: Option<String>,
    pub required_prior_tool_result: Option<ModelToolCallId>,
    pub invocation_ordinal: u64,
    pub attempt: ProviderAttempt,
}

/// One invocation program consumed exactly once in queue order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptedProgram {
    pub expectation: ScriptExpectation,
    pub steps: Vec<ScriptedStep>,
}

/// Redacted terminal observation retained by capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptedTerminalCapture {
    Completed,
    ProviderFailed(ModelStreamProviderErrorKind),
    Failed(ProviderErrorKind),
    Cancelled,
    OutcomeUnknown,
    TimedOut(ProviderErrorKind),
    ScriptMismatch,
    InvalidProgram,
    EndedWithoutTerminal,
}

/// Exact test-safe capture. Its Debug implementation never renders request content or events.
#[derive(Clone, Eq, PartialEq)]
pub struct ScriptedInvocationCapture {
    request: ModelRequest,
    target_id: ModelTargetId,
    request_sha256: Sha256Digest,
    invocation_ordinal: u64,
    attempt: ProviderAttempt,
    emitted_events: Vec<ModelStreamEvent>,
    cancellation_observed: bool,
    terminal: Option<ScriptedTerminalCapture>,
}

impl ScriptedInvocationCapture {
    #[must_use]
    pub const fn request(&self) -> &ModelRequest {
        &self.request
    }

    #[must_use]
    pub const fn target_id(&self) -> &ModelTargetId {
        &self.target_id
    }

    #[must_use]
    pub const fn request_sha256(&self) -> Sha256Digest {
        self.request_sha256
    }

    #[must_use]
    pub const fn invocation_ordinal(&self) -> u64 {
        self.invocation_ordinal
    }

    #[must_use]
    pub const fn attempt(&self) -> ProviderAttempt {
        self.attempt
    }

    #[must_use]
    pub fn emitted_events(&self) -> &[ModelStreamEvent] {
        &self.emitted_events
    }

    #[must_use]
    pub const fn cancellation_observed(&self) -> bool {
        self.cancellation_observed
    }

    #[must_use]
    pub const fn terminal(&self) -> Option<ScriptedTerminalCapture> {
        self.terminal
    }
}

impl fmt::Debug for ScriptedInvocationCapture {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptedInvocationCapture")
            .field("target_id", &self.target_id)
            .field("request_sha256", &self.request_sha256)
            .field("invocation_ordinal", &self.invocation_ordinal)
            .field("attempt", &self.attempt)
            .field("emitted_event_count", &self.emitted_events.len())
            .field("cancellation_observed", &self.cancellation_observed)
            .field("terminal", &self.terminal)
            .finish()
    }
}

#[derive(Default)]
struct ScriptedState {
    programs: VecDeque<ScriptedProgram>,
    captures: Vec<ScriptedInvocationCapture>,
    invocation_count: u64,
}

/// The only concrete Stage 15 provider adapter. It is fixture-only and never composed in production.
pub struct ScriptedProvider {
    fixture_provider_id: ProviderId,
    state: Arc<Mutex<ScriptedState>>,
    clock: Arc<dyn Clock>,
}

/// One exact deterministic estimator expectation consumed in queue order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptedEstimate {
    pub target_id: ModelTargetId,
    pub units: Vec<TokenEstimateUnit>,
    pub tokens: u64,
}

/// Deterministic estimator seam for Stage 16 tests; it does not assemble context.
#[derive(Debug)]
pub struct ScriptedTokenEstimator {
    identity: crate::domain::TokenEstimatorIdentity,
    expected: Box<[ScriptedEstimate]>,
    captures: Mutex<Vec<(ModelTargetId, Vec<TokenEstimateUnit>)>>,
}

impl ScriptedTokenEstimator {
    pub fn try_new(
        identity: crate::domain::TokenEstimatorIdentity,
        expected: Vec<ScriptedEstimate>,
    ) -> Result<Self, ProviderError> {
        let mut keys = BTreeSet::new();
        for estimate in &expected {
            if !keys.insert((estimate.target_id.clone(), estimate.units.clone()))
                || estimate.tokens < conservative_minimum(&estimate.units)?
            {
                return Err(invalid_program());
            }
        }
        Ok(Self {
            identity,
            expected: expected.into_boxed_slice(),
            captures: Mutex::new(Vec::new()),
        })
    }

    #[must_use]
    pub fn captures(&self) -> Vec<(ModelTargetId, Vec<TokenEstimateUnit>)> {
        self.captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl TokenEstimator for ScriptedTokenEstimator {
    fn identity(&self) -> &crate::domain::TokenEstimatorIdentity {
        &self.identity
    }

    fn estimate(
        &self,
        target: &ModelTarget,
        units: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError> {
        let target_id = target.reference().model_target_id();
        let expectation = self
            .expected
            .iter()
            .find(|expected| &expected.target_id == target_id && expected.units == units)
            .ok_or_else(script_mismatch)?;
        self.captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((target_id.clone(), units.to_vec()));
        ConservativeTokenEstimate::try_new(self.identity.clone(), expectation.tokens)
    }
}

impl ScriptedProvider {
    pub fn new(fixture_provider_id: ProviderId, programs: Vec<ScriptedProgram>) -> Self {
        Self::with_clock(fixture_provider_id, programs, Arc::new(SystemClock::new()))
    }

    pub fn with_clock(
        fixture_provider_id: ProviderId,
        programs: Vec<ScriptedProgram>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            fixture_provider_id,
            state: Arc::new(Mutex::new(ScriptedState {
                programs: programs.into(),
                ..ScriptedState::default()
            })),
            clock,
        }
    }

    #[must_use]
    pub fn invocation_count(&self) -> u64 {
        lock_state(&self.state).invocation_count
    }

    #[must_use]
    pub fn captures(&self) -> Vec<ScriptedInvocationCapture> {
        lock_state(&self.state).captures.clone()
    }

    #[must_use]
    pub fn remaining_programs(&self) -> usize {
        lock_state(&self.state).programs.len()
    }
}

impl fmt::Debug for ScriptedProvider {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptedProvider")
            .field("fixture_provider_id", &self.fixture_provider_id)
            .field("invocation_count", &self.invocation_count())
            .field("remaining_programs", &self.remaining_programs())
            .finish()
    }
}

impl ModelProvider for ScriptedProvider {
    fn provider_id(&self) -> &ProviderId {
        &self.fixture_provider_id
    }

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        if target.reference().provider_id() != &self.fixture_provider_id {
            return Err(ProviderError::new(
                ProviderErrorKind::UnknownModel,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            ));
        }
        Ok(target.reference().capabilities().clone())
    }

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            let target_id = invocation
                .request
                .target()
                .reference()
                .model_target_id()
                .clone();
            let request_sha256 = invocation.request.canonical_sha256();
            let mut state = lock_state(&self.state);
            state.invocation_count = state.invocation_count.saturating_add(1);
            let ordinal = state.invocation_count;
            let cancellation_observed = invocation.control.cancellation().is_cancelled();
            let mut capture = ScriptedInvocationCapture {
                request: invocation.request.clone(),
                target_id: target_id.clone(),
                request_sha256,
                invocation_ordinal: ordinal,
                attempt: invocation.attempt,
                emitted_events: Vec::new(),
                cancellation_observed,
                terminal: None,
            };
            let Some(program) = state.programs.pop_front() else {
                capture.terminal = Some(ScriptedTerminalCapture::ScriptMismatch);
                state.captures.push(capture);
                return Err(script_mismatch());
            };
            if program.expectation.target_id != target_id
                || program.expectation.invocation_ordinal != ordinal
                || program.expectation.attempt != invocation.attempt
                || program
                    .expectation
                    .request_sha256
                    .is_some_and(|expected| expected != request_sha256)
                || program.expectation.fixture_key != invocation.fixture_key
                || program
                    .expectation
                    .required_prior_tool_result
                    .as_ref()
                    .is_some_and(|required| !request_has_tool_result(&invocation.request, required))
            {
                capture.terminal = Some(ScriptedTerminalCapture::ScriptMismatch);
                state.captures.push(capture);
                return Err(script_mismatch());
            }
            if scripted_program_has_terminal_residue(&program.steps) {
                capture.terminal = Some(ScriptedTerminalCapture::InvalidProgram);
                state.captures.push(capture);
                return Err(invalid_program());
            }
            if cancellation_observed {
                capture.terminal = Some(ScriptedTerminalCapture::Cancelled);
            }
            let now = self.clock.monotonic_now();
            if !cancellation_observed
                && deadline_expired(now, invocation.control.absolute_deadline())
            {
                capture.terminal = Some(ScriptedTerminalCapture::TimedOut(
                    ProviderErrorKind::TimeoutBeforeOutput,
                ));
            }
            state.captures.push(capture);
            let capture_index = state.captures.len() - 1;
            drop(state);
            if cancellation_observed {
                return Err(cancelled_error(false));
            }
            if deadline_expired(now, invocation.control.absolute_deadline()) {
                return Err(timeout_error(false));
            }
            Ok(Box::new(ScriptedStream {
                state: Arc::clone(&self.state),
                capture_index,
                steps: program.steps.into(),
                cancellation: invocation.control.cancellation().clone(),
                absolute_deadline: invocation.control.absolute_deadline(),
                idle_timeout: invocation.control.idle_timeout(),
                clock: Arc::clone(&self.clock),
                last_activity: now,
                semantic_output_observed: false,
                terminal: false,
            }) as Box<dyn ModelProviderStream>)
        })
    }
}

struct ScriptedStream {
    state: Arc<Mutex<ScriptedState>>,
    capture_index: usize,
    steps: VecDeque<ScriptedStep>,
    cancellation: crate::ports::model_provider::ProviderCancellationToken,
    absolute_deadline: MonotonicInstant,
    idle_timeout: std::time::Duration,
    clock: Arc<dyn Clock>,
    last_activity: MonotonicInstant,
    semantic_output_observed: bool,
    terminal: bool,
}

impl ModelProviderStream for ScriptedStream {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>> {
        Box::pin(async move {
            if self.terminal {
                return Ok(None);
            }
            loop {
                if self.cancellation.is_cancelled() {
                    return self.finish_cancelled();
                }
                if let Some(error) = self.expired_timeout() {
                    return self.finish_timeout(error);
                }
                match self.steps.pop_front() {
                    Some(ScriptedStep::Emit(event)) => {
                        let event = *event;
                        let terminal = event.is_terminal();
                        self.last_activity = self.clock.monotonic_now();
                        self.semantic_output_observed |= event.is_semantic_output();
                        update_capture(&self.state, self.capture_index, |capture| {
                            capture.emitted_events.push(event.clone());
                            if terminal {
                                capture.terminal = Some(terminal_capture_for_event(&event));
                            }
                        });
                        self.terminal = terminal;
                        return Ok(Some(event));
                    }
                    Some(ScriptedStep::Fail(error)) => {
                        self.terminal = true;
                        update_capture(&self.state, self.capture_index, |capture| {
                            capture.terminal = Some(terminal_capture_for_error(&error));
                        });
                        return Err(error);
                    }
                    Some(ScriptedStep::AwaitRelease(gate)) => {
                        tokio::select! {
                            () = gate.wait() => {}
                            () = self.cancellation.cancelled() => {
                                return self.finish_cancelled();
                            }
                        }
                        if self.cancellation.is_cancelled() {
                            return self.finish_cancelled();
                        }
                        if let Some(error) = self.expired_timeout() {
                            return self.finish_timeout(error);
                        }
                    }
                    None => {
                        self.terminal = true;
                        update_capture(&self.state, self.capture_index, |capture| {
                            capture.terminal = Some(ScriptedTerminalCapture::EndedWithoutTerminal);
                        });
                        return Ok(None);
                    }
                }
            }
        })
    }
}

impl ScriptedStream {
    fn expired_timeout(&self) -> Option<ProviderError> {
        let now = self.clock.monotonic_now();
        let overall_expired = deadline_expired(now, self.absolute_deadline);
        let idle_expired = now
            .checked_duration_since(self.last_activity)
            .is_some_and(|elapsed| elapsed >= self.idle_timeout);
        (overall_expired || idle_expired).then(|| timeout_error(self.semantic_output_observed))
    }

    fn finish_cancelled<T>(&mut self) -> Result<T, ProviderError> {
        self.terminal = true;
        update_capture(&self.state, self.capture_index, |capture| {
            capture.cancellation_observed = true;
            capture.terminal = Some(ScriptedTerminalCapture::Cancelled);
        });
        Err(cancelled_error(self.semantic_output_observed))
    }

    fn finish_timeout<T>(&mut self, error: ProviderError) -> Result<T, ProviderError> {
        self.terminal = true;
        update_capture(&self.state, self.capture_index, |capture| {
            capture.terminal = Some(terminal_capture_for_error(&error));
        });
        Err(error)
    }
}

fn request_has_tool_result(request: &ModelRequest, required: &ModelToolCallId) -> bool {
    request.ordered_input_items().iter().any(|item| {
        matches!(
            item,
            ModelInputItem::ToolResult { call_id, .. } if call_id == required
        )
    })
}

fn script_mismatch() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::ScriptMismatch,
        ProviderOutcomeCertainty::DefinitelyNotSent,
    )
}

fn invalid_program() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::InvalidScriptProgram,
        ProviderOutcomeCertainty::DefinitelyNotSent,
    )
}

fn cancelled_error(semantic_output_observed: bool) -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::Cancelled,
        if semantic_output_observed {
            ProviderOutcomeCertainty::SemanticOutputObserved
        } else {
            ProviderOutcomeCertainty::ProviderOutcomeUnknown
        },
    )
}

fn timeout_error(semantic_output_observed: bool) -> ProviderError {
    ProviderError::new(
        if semantic_output_observed {
            ProviderErrorKind::TimeoutAfterOutput
        } else {
            ProviderErrorKind::TimeoutBeforeOutput
        },
        if semantic_output_observed {
            ProviderOutcomeCertainty::SemanticOutputObserved
        } else {
            ProviderOutcomeCertainty::DefiniteProviderFailure
        },
    )
}

fn deadline_expired(now: MonotonicInstant, deadline: MonotonicInstant) -> bool {
    now >= deadline
}

fn scripted_program_has_terminal_residue(steps: &[ScriptedStep]) -> bool {
    steps.iter().enumerate().any(|(index, step)| {
        let terminal = matches!(step, ScriptedStep::Fail(_))
            || matches!(step, ScriptedStep::Emit(event) if event.is_terminal());
        terminal && index + 1 != steps.len()
    })
}

fn terminal_capture_for_event(event: &ModelStreamEvent) -> ScriptedTerminalCapture {
    match event {
        ModelStreamEvent::Completed(_) => ScriptedTerminalCapture::Completed,
        ModelStreamEvent::ProviderError { kind } => match kind {
            ModelStreamProviderErrorKind::Cancelled => ScriptedTerminalCapture::Cancelled,
            ModelStreamProviderErrorKind::OutcomeUnknown => ScriptedTerminalCapture::OutcomeUnknown,
            ModelStreamProviderErrorKind::TimeoutBeforeOutput => {
                ScriptedTerminalCapture::TimedOut(ProviderErrorKind::TimeoutBeforeOutput)
            }
            ModelStreamProviderErrorKind::TimeoutAfterOutput => {
                ScriptedTerminalCapture::TimedOut(ProviderErrorKind::TimeoutAfterOutput)
            }
            _ => ScriptedTerminalCapture::ProviderFailed(*kind),
        },
        _ => unreachable!("only terminal events are captured as terminal"),
    }
}

fn terminal_capture_for_error(error: &ProviderError) -> ScriptedTerminalCapture {
    match error.kind() {
        ProviderErrorKind::Cancelled => ScriptedTerminalCapture::Cancelled,
        ProviderErrorKind::ProviderOutcomeUnknown
        | ProviderErrorKind::TransportAfterPossibleProcessing => {
            ScriptedTerminalCapture::OutcomeUnknown
        }
        ProviderErrorKind::TimeoutBeforeOutput | ProviderErrorKind::TimeoutAfterOutput => {
            ScriptedTerminalCapture::TimedOut(error.kind())
        }
        ProviderErrorKind::ScriptMismatch => ScriptedTerminalCapture::ScriptMismatch,
        ProviderErrorKind::InvalidScriptProgram => ScriptedTerminalCapture::InvalidProgram,
        kind => ScriptedTerminalCapture::Failed(kind),
    }
}

fn conservative_minimum(units: &[TokenEstimateUnit]) -> Result<u64, ProviderError> {
    units.iter().try_fold(0_u64, |total, unit| {
        let bytes = match unit {
            TokenEstimateUnit::TextBytes(bytes)
            | TokenEstimateUnit::StructuredBytes(bytes)
            | TokenEstimateUnit::ToolDefinitionBytes(bytes)
            | TokenEstimateUnit::ProviderOpaqueBytes(bytes) => *bytes,
        };
        total.checked_add(bytes).ok_or_else(invalid_program)
    })
}

fn lock_state(state: &Mutex<ScriptedState>) -> std::sync::MutexGuard<'_, ScriptedState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn update_capture(
    state: &Mutex<ScriptedState>,
    index: usize,
    update: impl FnOnce(&mut ScriptedInvocationCapture),
) {
    if let Some(capture) = lock_state(state).captures.get_mut(index) {
        update(capture);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::domain::model::ModelUsage;
    use crate::domain::{
        CanonicalModelToolCall, ContextManifestId, LogicalInvocationId,
        ModelCapabilitySnapshotInput, ModelConfigReference, ModelInputRole, ModelOutputItem,
        ModelRequestInput, ModelResponse, ModelResponseInput, ModelStopReason, ModelTargetIdentity,
        ModelTargetInput, ModelTextPart, ModelToolChoicePolicy, ProviderEvidenceId,
        ProviderMetadata, ProviderModelId, ProviderModelReference, ProviderNativeOptions,
        ProviderOpaqueEvidence, TargetConfigurationVersion, TokenCount, TokenEstimatorIdentity,
        validate_model_stream,
    };
    use crate::ports::clock::{MonotonicInstant, TestClock};
    use crate::ports::model_provider::provider_contract::{
        ModelProviderContractFixture, ProviderContractCase, ProviderContractErrorStage,
        ProviderContractExpected, ProviderContractScenario, assert_model_provider_contract,
    };
    use crate::ports::model_provider::{
        DEFAULT_PROVIDER_IDLE_TIMEOUT, ModelInvocationControl, ProviderCancellationToken,
        classify_provider_retry,
    };

    const V7_A: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";
    const V7_B: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0e";

    fn text(value: &str) -> ModelTextPart {
        ModelTextPart::try_new(value).unwrap()
    }

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
                ModelTargetId::try_new("fixture-primary").unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new("fixture-primary-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("fixture-account").unwrap(),
            requested_output_tokens: TokenCount::try_new(1_024).unwrap(),
            estimator: TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
            provider_native_options: ProviderNativeOptions::new(true),
        })
        .unwrap()
    }

    fn request(extra: Vec<ModelInputItem>) -> ModelRequest {
        let mut items =
            vec![ModelInputItem::message(ModelInputRole::User, vec![text("inspect")]).unwrap()];
        items.extend(extra);
        ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: V7_A.parse::<LogicalInvocationId>().unwrap(),
            target: target(),
            ordered_input_items: items,
            instructions: vec![text("answer deterministically")],
            tool_definitions: vec![],
            requested_output_limit: TokenCount::try_new(1_024).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::Automatic,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: V7_B.parse::<ContextManifestId>().unwrap(),
        })
        .unwrap()
    }

    fn usage() -> ModelUsage {
        ModelUsage::try_new(10, 0, 5, 0, 15).unwrap()
    }

    fn response(items: Vec<ModelOutputItem>, stop: ModelStopReason) -> ModelResponse {
        ModelResponse::try_new(ModelResponseInput {
            selected_target: ModelTargetIdentity::from_reference(target().reference()),
            output_items: items,
            stop_reason: stop,
            usage: Some(usage()),
            provider_request_id: Some(ProviderEvidenceId::try_new("req-fixture").unwrap()),
            provider_response_id: Some(ProviderEvidenceId::try_new("resp-fixture").unwrap()),
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        })
        .unwrap()
    }

    fn started() -> ModelStreamEvent {
        ModelStreamEvent::ResponseStarted {
            target: target().identity(),
            provider_request_id: Some(ProviderEvidenceId::try_new("req-fixture").unwrap()),
            provider_response_id: Some(ProviderEvidenceId::try_new("resp-fixture").unwrap()),
        }
    }

    fn expectation(request: &ModelRequest) -> ScriptExpectation {
        ScriptExpectation {
            target_id: request.target().reference().model_target_id().clone(),
            request_sha256: Some(request.canonical_sha256()),
            fixture_key: None,
            required_prior_tool_result: None,
            invocation_ordinal: 1,
            attempt: ProviderAttempt::try_new(1).unwrap(),
        }
    }

    fn invocation(
        request: ModelRequest,
        cancellation: ProviderCancellationToken,
    ) -> ModelProviderInvocation {
        invocation_with_limits(
            request,
            cancellation,
            MonotonicInstant::from_elapsed(Duration::from_secs(300)),
            DEFAULT_PROVIDER_IDLE_TIMEOUT,
        )
    }

    fn invocation_with_limits(
        request: ModelRequest,
        cancellation: ProviderCancellationToken,
        absolute_deadline: MonotonicInstant,
        idle_timeout: Duration,
    ) -> ModelProviderInvocation {
        ModelProviderInvocation {
            request,
            attempt: ProviderAttempt::try_new(1).unwrap(),
            control: ModelInvocationControl::try_new(cancellation, absolute_deadline, idle_timeout)
                .unwrap(),
            fixture_key: None,
        }
    }

    async fn collect(
        provider: &dyn ModelProvider,
        invocation: ModelProviderInvocation,
    ) -> Result<Vec<ModelStreamEvent>, ProviderError> {
        let mut stream = provider.invoke_stream(invocation).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next_event().await? {
            events.push(event);
        }
        Ok(events)
    }

    async fn run_events(
        request: ModelRequest,
        events: Vec<ModelStreamEvent>,
    ) -> (ScriptedProvider, Vec<ModelStreamEvent>) {
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: events.into_iter().map(ScriptedStep::emit).collect(),
            }],
        );
        let captured = collect(
            &provider,
            invocation(request, ProviderCancellationToken::new()),
        )
        .await
        .unwrap();
        (provider, captured)
    }

    fn tool(call_id: &str, arguments: &str) -> CanonicalModelToolCall {
        CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new(call_id).unwrap(),
            "read_file",
            arguments,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn scripted_text_only_completion_is_ordered_and_captured_once() {
        let final_response = response(
            vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
            ModelStopReason::Completed,
        );
        let (provider, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text("done"),
                },
                ModelStreamEvent::Usage(usage()),
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert_eq!(
            validate_model_stream(&events).unwrap(),
            crate::domain::ModelStreamState::Completed
        );
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(provider.captures()[0].emitted_events(), events);
        assert_eq!(
            provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::Completed)
        );
    }

    #[tokio::test]
    async fn scripted_one_tool_call_preserves_identity_name_and_arguments() {
        let call = tool("call-1", "{\"path\":\"README.md\"}");
        let final_response = response(
            vec![ModelOutputItem::ToolCall(call.clone())],
            ModelStopReason::ToolContinuation,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::ToolCallStarted {
                    item_ordinal: 0,
                    call_id: call.call_id().clone(),
                    name: call.name().clone(),
                },
                ModelStreamEvent::tool_argument_delta(
                    0,
                    call.call_id().clone(),
                    call.raw_arguments(),
                )
                .unwrap(),
                ModelStreamEvent::ToolCallCompleted {
                    item_ordinal: 0,
                    call,
                },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn scripted_text_then_tool_call_preserves_provider_order() {
        let call = tool("call-1", "{}");
        let final_response = response(
            vec![
                ModelOutputItem::text(vec![text("checking")]).unwrap(),
                ModelOutputItem::ToolCall(call.clone()),
            ],
            ModelStopReason::ToolContinuation,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text("checking"),
                },
                ModelStreamEvent::ToolCallStarted {
                    item_ordinal: 1,
                    call_id: call.call_id().clone(),
                    name: call.name().clone(),
                },
                ModelStreamEvent::ToolCallCompleted {
                    item_ordinal: 1,
                    call,
                },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert!(matches!(events[1], ModelStreamEvent::TextDelta { .. }));
        assert!(matches!(
            events[2],
            ModelStreamEvent::ToolCallStarted { .. }
        ));
    }

    #[tokio::test]
    async fn scripted_multiple_tool_calls_preserve_exact_ordinals() {
        let first = tool("call-1", "{}");
        let second = tool("call-2", "{}");
        let final_response = response(
            vec![
                ModelOutputItem::ToolCall(first.clone()),
                ModelOutputItem::ToolCall(second.clone()),
            ],
            ModelStopReason::ToolContinuation,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::ToolCallCompleted {
                    item_ordinal: 0,
                    call: first,
                },
                ModelStreamEvent::ToolCallCompleted {
                    item_ordinal: 1,
                    call: second,
                },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert!(matches!(
            events[1],
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 0,
                ..
            }
        ));
        assert!(matches!(
            events[2],
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn scripted_complete_multi_tool_lifecycle_preserves_order_without_execution() {
        let first = tool("call-a", "{\"path\":\"a\"}");
        let second = tool("call-b", "{\"path\":\"b\"}");
        let final_response = response(
            vec![
                ModelOutputItem::ToolCall(first.clone()),
                ModelOutputItem::ToolCall(second.clone()),
            ],
            ModelStopReason::ToolContinuation,
        );
        let expected = vec![
            started(),
            ModelStreamEvent::ToolCallStarted {
                item_ordinal: 0,
                call_id: first.call_id().clone(),
                name: first.name().clone(),
            },
            ModelStreamEvent::tool_argument_delta(0, first.call_id().clone(), "{\"path\":")
                .unwrap(),
            ModelStreamEvent::tool_argument_delta(0, first.call_id().clone(), "\"a\"}").unwrap(),
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 0,
                call: first.clone(),
            },
            ModelStreamEvent::ToolCallStarted {
                item_ordinal: 1,
                call_id: second.call_id().clone(),
                name: second.name().clone(),
            },
            ModelStreamEvent::tool_argument_delta(1, second.call_id().clone(), "{\"path\":")
                .unwrap(),
            ModelStreamEvent::tool_argument_delta(1, second.call_id().clone(), "\"b\"}").unwrap(),
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 1,
                call: second.clone(),
            },
            ModelStreamEvent::Usage(usage()),
            ModelStreamEvent::Completed(Box::new(final_response)),
        ];
        let (_, events) = run_events(request(vec![]), expected.clone()).await;
        assert_eq!(events, expected);
        assert_eq!(
            validate_model_stream(&events).unwrap(),
            crate::domain::ModelStreamState::Completed
        );
        let ModelStreamEvent::Completed(response) = events.last().unwrap() else {
            panic!()
        };
        let calls = response
            .output_items()
            .iter()
            .map(|item| match item {
                ModelOutputItem::ToolCall(call) => (call.call_id().as_str(), call.raw_arguments()),
                _ => panic!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            calls,
            [
                (first.call_id().as_str(), first.raw_arguments()),
                (second.call_id().as_str(), second.raw_arguments()),
            ]
        );
    }

    #[tokio::test]
    async fn scripted_valid_mixed_text_and_tool_remains_ordered_evidence() {
        let call = tool("call-1", "{}");
        let final_response = response(
            vec![
                ModelOutputItem::text(vec![text("prefix")]).unwrap(),
                ModelOutputItem::ToolCall(call),
            ],
            ModelStopReason::ToolContinuation,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::Completed(Box::new(final_response.clone())),
            ],
        )
        .await;
        let ModelStreamEvent::Completed(captured) = &events[1] else {
            panic!()
        };
        assert_eq!(captured.output_items().len(), 2);
    }

    #[tokio::test]
    async fn scripted_refusal_is_semantic_output_not_transport_failure() {
        let final_response = response(
            vec![ModelOutputItem::refusal(vec![text("cannot")]).unwrap()],
            ModelStopReason::Refusal,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::RefusalDelta {
                    item_ordinal: 0,
                    delta: text("cannot"),
                },
                ModelStreamEvent::RefusalCompleted { item_ordinal: 0 },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert!(events[1].is_semantic_output());
    }

    #[tokio::test]
    async fn scripted_structured_data_preserves_canonical_json() {
        let final_response = response(
            vec![ModelOutputItem::structured_data(json!({"b": 2, "a": 1})).unwrap()],
            ModelStopReason::Completed,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::StructuredData {
                    item_ordinal: 0,
                    data: json!({"a": 1, "b": 2}),
                },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert!(matches!(events[1], ModelStreamEvent::StructuredData { .. }));
    }

    #[tokio::test]
    async fn scripted_reasoning_summary_exposes_only_summary_delta() {
        let final_response = response(
            vec![
                ModelOutputItem::text(vec![text("answer")]).unwrap(),
                ModelOutputItem::reasoning_summary(vec![text("summary")]).unwrap(),
            ],
            ModelStopReason::Completed,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::ReasoningSummaryDelta {
                    item_ordinal: 0,
                    delta: text("summary"),
                },
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert!(matches!(
            events[1],
            ModelStreamEvent::ReasoningSummaryDelta { .. }
        ));
    }

    #[tokio::test]
    async fn scripted_opaque_continuation_retains_provider_hash_and_type() {
        let opaque = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "continuation-v1",
            "opaque-fixture",
        )
        .unwrap();
        let final_response = ModelResponseInput {
            selected_target: target().identity(),
            output_items: vec![
                ModelOutputItem::text(vec![text("answer")]).unwrap(),
                ModelOutputItem::ProviderOpaque(opaque.clone()),
            ],
            stop_reason: ModelStopReason::Completed,
            usage: Some(usage()),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: Some(opaque.clone()),
            provider_metadata: ProviderMetadata::default(),
        };
        let response = ModelResponse::try_new(final_response).unwrap();
        let (_, events) = run_events(
            request(vec![]),
            vec![started(), ModelStreamEvent::Completed(Box::new(response))],
        )
        .await;
        let ModelStreamEvent::Completed(response) = &events[1] else {
            panic!()
        };
        assert_eq!(
            response.provider_continuation().unwrap().sha256(),
            opaque.sha256()
        );
    }

    #[tokio::test]
    async fn scripted_usage_and_provider_ids_are_preserved() {
        let final_response = response(
            vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
            ModelStopReason::Completed,
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::Usage(usage()),
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        let ModelStreamEvent::Completed(response) = &events[2] else {
            panic!()
        };
        assert_eq!(response.usage(), Some(usage()));
        assert_eq!(
            response.provider_request_id().unwrap().as_str(),
            "req-fixture"
        );
        assert_eq!(
            response.provider_response_id().unwrap().as_str(),
            "resp-fixture"
        );
    }

    #[tokio::test]
    async fn scripted_transient_pre_output_failure_is_retry_eligible() {
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::TemporarilyUnavailable,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                ))],
            }],
        );
        let error = collect(
            &provider,
            invocation(request, ProviderCancellationToken::new()),
        )
        .await
        .unwrap_err();
        assert!(classify_provider_retry(&error, false, 1, false, false).retryable());
        assert_eq!(
            provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::Failed(
                ProviderErrorKind::TemporarilyUnavailable
            ))
        );
    }

    #[tokio::test]
    async fn scripted_failure_after_semantic_output_is_never_retryable() {
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![
                    ScriptedStep::emit(started()),
                    ScriptedStep::emit(ModelStreamEvent::TextDelta {
                        item_ordinal: 0,
                        delta: text("draft"),
                    }),
                    ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TimeoutAfterOutput,
                        ProviderOutcomeCertainty::SemanticOutputObserved,
                    )),
                ],
            }],
        );
        let error = collect(
            &provider,
            invocation(request, ProviderCancellationToken::new()),
        )
        .await
        .unwrap_err();
        assert!(!classify_provider_retry(&error, true, 1, false, false).retryable());
        assert_eq!(provider.captures()[0].emitted_events().len(), 2);
    }

    #[tokio::test]
    async fn scripted_malformed_tool_arguments_are_retained_then_fail_closed() {
        let malformed = tool("call-1", "{");
        let final_response = response(
            vec![ModelOutputItem::ToolCall(malformed)],
            ModelStopReason::ToolContinuation,
        );
        assert_eq!(
            final_response
                .require_supported_semantics()
                .unwrap_err()
                .kind(),
            crate::domain::ModelContractErrorKind::InvalidToolArguments
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn scripted_oversized_tool_arguments_are_rejected_before_emission() {
        let error = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("call-1").unwrap(),
            "read_file",
            "x".repeat(crate::domain::MAX_MODEL_TOOL_ARGUMENT_BYTES + 1),
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            crate::domain::ModelContractErrorKind::ToolArgumentsTooLarge
        );
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::MalformedResponse,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                ))],
            }],
        );
        assert_eq!(
            collect(
                &provider,
                invocation(request, ProviderCancellationToken::new())
            )
            .await
            .unwrap_err()
            .kind(),
            ProviderErrorKind::MalformedResponse
        );
    }

    #[tokio::test]
    async fn scripted_duplicate_provider_tool_ids_fail_closed_without_item_drop() {
        let duplicate = vec![
            ModelOutputItem::ToolCall(tool("same", "{}")),
            ModelOutputItem::ToolCall(tool("same", "{}")),
        ];
        assert_eq!(
            ModelResponse::try_new(ModelResponseInput {
                selected_target: target().identity(),
                output_items: duplicate,
                stop_reason: ModelStopReason::ToolContinuation,
                usage: Some(usage()),
                provider_request_id: None,
                provider_response_id: None,
                provider_continuation: None,
                provider_metadata: ProviderMetadata::default()
            })
            .unwrap_err()
            .kind(),
            crate::domain::ModelContractErrorKind::DuplicateToolCallId
        );
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::UnsupportedResponseItem,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                ))],
            }],
        );
        assert_eq!(
            collect(
                &provider,
                invocation(request, ProviderCancellationToken::new())
            )
            .await
            .unwrap_err()
            .kind(),
            ProviderErrorKind::UnsupportedResponseItem
        );
    }

    #[tokio::test]
    async fn scripted_timeout_before_output_is_retry_eligible() {
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::TimeoutBeforeOutput,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                ))],
            }],
        );
        let error = collect(
            &provider,
            invocation(request, ProviderCancellationToken::new()),
        )
        .await
        .unwrap_err();
        assert!(classify_provider_retry(&error, false, 1, false, false).retryable());
    }

    #[tokio::test]
    async fn scripted_timeout_after_output_is_not_retryable() {
        let request = request(vec![]);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![
                    ScriptedStep::emit(started()),
                    ScriptedStep::emit(ModelStreamEvent::ToolArgumentDelta {
                        item_ordinal: 0,
                        call_id: ModelToolCallId::try_new("call").unwrap(),
                        delta: "{".into(),
                    }),
                    ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TimeoutAfterOutput,
                        ProviderOutcomeCertainty::SemanticOutputObserved,
                    )),
                ],
            }],
        );
        let error = collect(
            &provider,
            invocation(request, ProviderCancellationToken::new()),
        )
        .await
        .unwrap_err();
        assert!(!classify_provider_retry(&error, true, 1, false, false).retryable());
    }

    #[tokio::test]
    async fn scripted_cancellation_uses_barrier_and_records_observation_without_sleep() {
        let request = request(vec![]);
        let gate = ScriptGate::new();
        let provider = Arc::new(ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(&request),
                steps: vec![
                    ScriptedStep::emit(started()),
                    ScriptedStep::AwaitRelease(gate),
                    ScriptedStep::emit(ModelStreamEvent::ProviderError {
                        kind: crate::domain::ModelStreamProviderErrorKind::DefiniteFailure,
                    }),
                ],
            }],
        ));
        let cancellation = ProviderCancellationToken::new();
        let mut stream = provider
            .invoke_stream(invocation(request, cancellation.clone()))
            .await
            .unwrap();
        assert!(matches!(
            stream.next_event().await.unwrap(),
            Some(ModelStreamEvent::ResponseStarted { .. })
        ));
        let next = tokio::spawn(async move { stream.next_event().await });
        cancellation.cancel();
        assert_eq!(
            next.await.unwrap().unwrap_err().kind(),
            ProviderErrorKind::Cancelled
        );
        assert!(provider.captures()[0].cancellation_observed());
    }

    #[tokio::test]
    async fn scripted_unknown_provider_item_is_retained_and_rejected_semantically() {
        let unknown = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "future-item-v9",
            "bounded-evidence",
        )
        .unwrap();
        let final_response = response(
            vec![ModelOutputItem::UnknownProviderItem(unknown.clone())],
            ModelStopReason::ProviderFailure,
        );
        assert_eq!(
            final_response
                .require_supported_semantics()
                .unwrap_err()
                .kind(),
            crate::domain::ModelContractErrorKind::UnknownSemanticItem
        );
        let (_, events) = run_events(
            request(vec![]),
            vec![
                started(),
                ModelStreamEvent::UnknownProviderEvent(unknown),
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn scripted_request_hash_mismatch_fails_deterministically_before_stream() {
        let request = request(vec![]);
        let mut expected = expectation(&request);
        expected.request_sha256 = Some(Sha256Digest::hash_bytes(b"different"));
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expected,
                steps: vec![],
            }],
        );
        let error = match provider
            .invoke_stream(invocation(request, ProviderCancellationToken::new()))
            .await
        {
            Ok(_) => panic!("hash mismatch must fail before returning a stream"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ProviderErrorKind::ScriptMismatch);
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(
            provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::ScriptMismatch)
        );
    }

    #[tokio::test]
    async fn scripted_machine_inspection_answer_fixture_is_deterministic() {
        let answer = json!({"architecture": "fixture-arch", "os_release": "fixture-os", "workspace": "primary"});
        let final_response = response(
            vec![ModelOutputItem::structured_data(answer).unwrap()],
            ModelStopReason::Completed,
        );
        let request = request(vec![]);
        let first_hash = request.canonical_sha256();
        let (provider, events) = run_events(
            request,
            vec![
                started(),
                ModelStreamEvent::Completed(Box::new(final_response)),
            ],
        )
        .await;
        assert_eq!(provider.captures()[0].request_sha256(), first_hash);
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn scripted_expectation_can_require_prior_tool_result_and_attempt_ordinal() {
        let call_id = ModelToolCallId::try_new("call-prior").unwrap();
        let request = request(vec![
            ModelInputItem::tool_result(call_id.clone(), json!({"ok": true})).unwrap(),
        ]);
        let mut expected = expectation(&request);
        expected.required_prior_tool_result = Some(call_id);
        let provider = ScriptedProvider::new(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expected,
                steps: vec![
                    ScriptedStep::emit(started()),
                    ScriptedStep::emit(ModelStreamEvent::Completed(Box::new(response(
                        vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
                        ModelStopReason::Completed,
                    )))),
                ],
            }],
        );
        assert_eq!(
            collect(
                &provider,
                invocation(request, ProviderCancellationToken::new())
            )
            .await
            .unwrap()
            .len(),
            2
        );
        assert_eq!(provider.captures()[0].attempt().get(), 1);
        assert_eq!(provider.captures()[0].invocation_ordinal(), 1);
    }

    struct ScriptedProviderContractFixture;

    impl ScriptedProviderContractFixture {
        fn expectation(request: &ModelRequest) -> ScriptExpectation {
            let mut expectation = expectation(request);
            expectation.fixture_key = Some("provider-contract".to_owned());
            expectation
        }

        fn invocation(
            request: ModelRequest,
            cancellation: ProviderCancellationToken,
            deadline: MonotonicInstant,
            idle_timeout: Duration,
        ) -> ModelProviderInvocation {
            let mut invocation =
                invocation_with_limits(request, cancellation, deadline, idle_timeout);
            invocation.fixture_key = Some("provider-contract".to_owned());
            invocation
        }

        fn case_with_provider(
            request: ModelRequest,
            provider: Arc<dyn ModelProvider>,
            invocation: ModelProviderInvocation,
            expected: ProviderContractExpected,
            before_next: BTreeMap<usize, Arc<dyn Fn() + Send + Sync>>,
        ) -> ProviderContractCase {
            ProviderContractCase {
                expected_capabilities: request.target().reference().capabilities().clone(),
                provider,
                invocation,
                expected,
                before_next,
            }
        }

        fn immediate_case(
            request: ModelRequest,
            steps: Vec<ScriptedStep>,
            expected: ProviderContractExpected,
        ) -> ProviderContractCase {
            let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
                ProviderId::try_new("fixture").unwrap(),
                vec![ScriptedProgram {
                    expectation: Self::expectation(&request),
                    steps,
                }],
            ));
            let invocation = Self::invocation(
                request.clone(),
                ProviderCancellationToken::new(),
                MonotonicInstant::from_elapsed(Duration::from_secs(300)),
                DEFAULT_PROVIDER_IDLE_TIMEOUT,
            );
            Self::case_with_provider(request, provider, invocation, expected, BTreeMap::new())
        }

        fn stream_error_case(
            kind: ProviderErrorKind,
            certainty: ProviderOutcomeCertainty,
            semantic_output_observed: bool,
        ) -> ProviderContractCase {
            let request = request(vec![]);
            let mut steps = Vec::new();
            if semantic_output_observed {
                steps.extend([
                    ScriptedStep::emit(started()),
                    ScriptedStep::emit(ModelStreamEvent::TextDelta {
                        item_ordinal: 0,
                        delta: text("partial"),
                    }),
                ]);
            }
            steps.push(ScriptedStep::Fail(ProviderError::new(kind, certainty)));
            Self::immediate_case(
                request,
                steps,
                ProviderContractExpected::Error {
                    stage: ProviderContractErrorStage::Stream,
                    kind,
                    certainty,
                    semantic_output_observed,
                },
            )
        }
    }

    impl ModelProviderContractFixture for ScriptedProviderContractFixture {
        fn build_case(&self, scenario: ProviderContractScenario) -> ProviderContractCase {
            match scenario {
                ProviderContractScenario::OrderedTextAndTools => {
                    let request = request(vec![]);
                    let first = tool("contract-a", "{\"path\":\"a\"}");
                    let second = tool("contract-b", "{\"path\":\"b\"}");
                    let final_response = response(
                        vec![
                            ModelOutputItem::text(vec![text("first"), text("second")]).unwrap(),
                            ModelOutputItem::ToolCall(first.clone()),
                            ModelOutputItem::ToolCall(second.clone()),
                        ],
                        ModelStopReason::ToolContinuation,
                    );
                    let events = vec![
                        started(),
                        ModelStreamEvent::TextDelta {
                            item_ordinal: 0,
                            delta: text("first"),
                        },
                        ModelStreamEvent::TextDelta {
                            item_ordinal: 0,
                            delta: text("second"),
                        },
                        ModelStreamEvent::ToolCallStarted {
                            item_ordinal: 1,
                            call_id: first.call_id().clone(),
                            name: first.name().clone(),
                        },
                        ModelStreamEvent::tool_argument_delta(
                            1,
                            first.call_id().clone(),
                            first.raw_arguments(),
                        )
                        .unwrap(),
                        ModelStreamEvent::ToolCallCompleted {
                            item_ordinal: 1,
                            call: first,
                        },
                        ModelStreamEvent::ToolCallStarted {
                            item_ordinal: 2,
                            call_id: second.call_id().clone(),
                            name: second.name().clone(),
                        },
                        ModelStreamEvent::tool_argument_delta(
                            2,
                            second.call_id().clone(),
                            second.raw_arguments(),
                        )
                        .unwrap(),
                        ModelStreamEvent::ToolCallCompleted {
                            item_ordinal: 2,
                            call: second,
                        },
                        ModelStreamEvent::Usage(usage()),
                        ModelStreamEvent::Completed(Box::new(final_response)),
                    ];
                    Self::immediate_case(
                        request,
                        events.iter().cloned().map(ScriptedStep::emit).collect(),
                        ProviderContractExpected::Events(events),
                    )
                }
                ProviderContractScenario::Refusal => {
                    let request = request(vec![]);
                    let final_response = response(
                        vec![ModelOutputItem::refusal(vec![text("cannot")]).unwrap()],
                        ModelStopReason::Refusal,
                    );
                    let events = vec![
                        started(),
                        ModelStreamEvent::RefusalDelta {
                            item_ordinal: 0,
                            delta: text("cannot"),
                        },
                        ModelStreamEvent::RefusalCompleted { item_ordinal: 0 },
                        ModelStreamEvent::Usage(usage()),
                        ModelStreamEvent::Completed(Box::new(final_response)),
                    ];
                    Self::immediate_case(
                        request,
                        events.iter().cloned().map(ScriptedStep::emit).collect(),
                        ProviderContractExpected::Events(events),
                    )
                }
                ProviderContractScenario::StructuredReasoningOpaque => {
                    let request = request(vec![]);
                    let opaque = ProviderOpaqueEvidence::try_new(
                        ProviderId::try_new("fixture").unwrap(),
                        "continuation-v1",
                        "opaque-contract",
                    )
                    .unwrap();
                    let final_response = response(
                        vec![
                            ModelOutputItem::structured_data(json!({"answer": 1})).unwrap(),
                            ModelOutputItem::reasoning_summary(vec![text("summary")]).unwrap(),
                            ModelOutputItem::ProviderOpaque(opaque),
                        ],
                        ModelStopReason::Completed,
                    );
                    let events = vec![
                        started(),
                        ModelStreamEvent::StructuredData {
                            item_ordinal: 0,
                            data: json!({"answer": 1}),
                        },
                        ModelStreamEvent::ReasoningSummaryDelta {
                            item_ordinal: 1,
                            delta: text("summary"),
                        },
                        ModelStreamEvent::Usage(usage()),
                        ModelStreamEvent::Completed(Box::new(final_response)),
                    ];
                    Self::immediate_case(
                        request,
                        events.iter().cloned().map(ScriptedStep::emit).collect(),
                        ProviderContractExpected::Events(events),
                    )
                }
                ProviderContractScenario::UnknownItemFailClosed => {
                    let request = request(vec![]);
                    let unknown = ProviderOpaqueEvidence::try_new(
                        ProviderId::try_new("fixture").unwrap(),
                        "future-v9",
                        "unknown-contract",
                    )
                    .unwrap();
                    let events = vec![
                        started(),
                        ModelStreamEvent::UnknownProviderEvent(unknown),
                        ModelStreamEvent::UsageUnavailable,
                        ModelStreamEvent::ProviderError {
                            kind: ModelStreamProviderErrorKind::ProtocolFailure,
                        },
                    ];
                    Self::immediate_case(
                        request,
                        events.iter().cloned().map(ScriptedStep::emit).collect(),
                        ProviderContractExpected::Events(events),
                    )
                }
                ProviderContractScenario::OutputItemLimit => Self::stream_error_case(
                    ProviderErrorKind::OutputTooLarge,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                    false,
                ),
                ProviderContractScenario::ArgumentLimit => Self::stream_error_case(
                    ProviderErrorKind::MalformedCompletedToolArguments,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                    false,
                ),
                ProviderContractScenario::AuthenticationFailure => Self::stream_error_case(
                    ProviderErrorKind::Authentication,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                    false,
                ),
                ProviderContractScenario::OutcomeUnknown => Self::stream_error_case(
                    ProviderErrorKind::ProviderOutcomeUnknown,
                    ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                    false,
                ),
                ProviderContractScenario::TimeoutAfterSemanticOutput => Self::stream_error_case(
                    ProviderErrorKind::TimeoutAfterOutput,
                    ProviderOutcomeCertainty::SemanticOutputObserved,
                    true,
                ),
                ProviderContractScenario::MalformedResponse => Self::stream_error_case(
                    ProviderErrorKind::MalformedResponse,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                    false,
                ),
                ProviderContractScenario::Cancellation => {
                    let request = request(vec![]);
                    let cancellation = ProviderCancellationToken::new();
                    cancellation.cancel();
                    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
                        ProviderId::try_new("fixture").unwrap(),
                        vec![ScriptedProgram {
                            expectation: Self::expectation(&request),
                            steps: vec![ScriptedStep::emit(started())],
                        }],
                    ));
                    let invocation = Self::invocation(
                        request.clone(),
                        cancellation,
                        MonotonicInstant::from_elapsed(Duration::from_secs(300)),
                        DEFAULT_PROVIDER_IDLE_TIMEOUT,
                    );
                    Self::case_with_provider(
                        request,
                        provider,
                        invocation,
                        ProviderContractExpected::Error {
                            stage: ProviderContractErrorStage::Invoke,
                            kind: ProviderErrorKind::Cancelled,
                            certainty: ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                            semantic_output_observed: false,
                        },
                        BTreeMap::new(),
                    )
                }
                ProviderContractScenario::AbsoluteDeadline => {
                    let clock = Arc::new(TestClock::new(
                        time::OffsetDateTime::UNIX_EPOCH,
                        Duration::from_secs(100),
                    ));
                    let request = request(vec![]);
                    let clock_port: Arc<dyn Clock> = clock;
                    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::with_clock(
                        ProviderId::try_new("fixture").unwrap(),
                        vec![ScriptedProgram {
                            expectation: Self::expectation(&request),
                            steps: vec![ScriptedStep::emit(started())],
                        }],
                        clock_port,
                    ));
                    let invocation = Self::invocation(
                        request.clone(),
                        ProviderCancellationToken::new(),
                        MonotonicInstant::from_elapsed(Duration::from_secs(100)),
                        Duration::from_secs(5),
                    );
                    Self::case_with_provider(
                        request,
                        provider,
                        invocation,
                        ProviderContractExpected::Error {
                            stage: ProviderContractErrorStage::Invoke,
                            kind: ProviderErrorKind::TimeoutBeforeOutput,
                            certainty: ProviderOutcomeCertainty::DefiniteProviderFailure,
                            semantic_output_observed: false,
                        },
                        BTreeMap::new(),
                    )
                }
                ProviderContractScenario::IdleTimeout => {
                    let clock = Arc::new(TestClock::new(
                        time::OffsetDateTime::UNIX_EPOCH,
                        Duration::from_secs(100),
                    ));
                    let request = request(vec![]);
                    let gate = ScriptGate::new();
                    let clock_port: Arc<dyn Clock> = clock.clone();
                    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::with_clock(
                        ProviderId::try_new("fixture").unwrap(),
                        vec![ScriptedProgram {
                            expectation: Self::expectation(&request),
                            steps: vec![
                                ScriptedStep::emit(started()),
                                ScriptedStep::AwaitRelease(gate.clone()),
                                ScriptedStep::emit(ModelStreamEvent::UsageUnavailable),
                            ],
                        }],
                        clock_port,
                    ));
                    let invocation = Self::invocation(
                        request.clone(),
                        ProviderCancellationToken::new(),
                        MonotonicInstant::from_elapsed(Duration::from_secs(200)),
                        Duration::from_secs(5),
                    );
                    let action: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                        clock.advance_monotonic(Duration::from_secs(5)).unwrap();
                        gate.release();
                    });
                    Self::case_with_provider(
                        request,
                        provider,
                        invocation,
                        ProviderContractExpected::Error {
                            stage: ProviderContractErrorStage::Stream,
                            kind: ProviderErrorKind::TimeoutBeforeOutput,
                            certainty: ProviderOutcomeCertainty::DefiniteProviderFailure,
                            semantic_output_observed: false,
                        },
                        BTreeMap::from([(1, action)]),
                    )
                }
            }
        }
    }

    #[tokio::test]
    async fn reusable_model_provider_contract_suite_passes_via_public_port_only() {
        assert_eq!(
            assert_model_provider_contract(&ScriptedProviderContractFixture).await,
            13
        );
    }

    #[tokio::test]
    async fn scripted_stream_preserves_all_events_and_rejects_post_terminal_ordering() {
        let final_response = response(
            vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
            ModelStopReason::Completed,
        );
        let expected = vec![
            started(),
            ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text("done"),
            },
            ModelStreamEvent::Usage(usage()),
            ModelStreamEvent::Completed(Box::new(final_response)),
        ];
        let (_, events) = run_events(request(vec![]), expected.clone()).await;
        assert_eq!(events, expected);
        assert_eq!(
            validate_model_stream(&events).unwrap(),
            crate::domain::ModelStreamState::Completed
        );
        let mut invalid = events;
        invalid.push(ModelStreamEvent::Usage(usage()));
        assert_eq!(
            validate_model_stream(&invalid).unwrap_err().kind(),
            crate::domain::ModelContractErrorKind::InvalidStreamOrdering
        );
    }

    #[tokio::test]
    async fn scripted_program_rejects_every_post_terminal_residue_before_emission() {
        let terminal_response = || {
            ModelStreamEvent::Completed(Box::new(response(
                vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
                ModelStopReason::Completed,
            )))
        };
        let programs = [
            vec![
                ScriptedStep::emit(terminal_response()),
                ScriptedStep::emit(ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text("late"),
                }),
            ],
            vec![
                ScriptedStep::emit(ModelStreamEvent::ProviderError {
                    kind: ModelStreamProviderErrorKind::DefiniteFailure,
                }),
                ScriptedStep::emit(ModelStreamEvent::UsageUnavailable),
            ],
            vec![
                ScriptedStep::emit(terminal_response()),
                ScriptedStep::emit(terminal_response()),
            ],
        ];
        for steps in programs {
            let request = request(vec![]);
            let provider = ScriptedProvider::new(
                ProviderId::try_new("fixture").unwrap(),
                vec![ScriptedProgram {
                    expectation: expectation(&request),
                    steps,
                }],
            );
            let error = match provider
                .invoke_stream(invocation(request, ProviderCancellationToken::new()))
                .await
            {
                Ok(_) => panic!("terminal residue must fail before returning a stream"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), ProviderErrorKind::InvalidScriptProgram);
            assert!(provider.captures()[0].emitted_events().is_empty());
            assert_eq!(
                provider.captures()[0].terminal(),
                Some(ScriptedTerminalCapture::InvalidProgram)
            );
        }
    }

    #[tokio::test]
    async fn scripted_emitted_provider_error_capture_preserves_terminal_certainty() {
        let cases = [
            (
                ModelStreamProviderErrorKind::Cancelled,
                ScriptedTerminalCapture::Cancelled,
            ),
            (
                ModelStreamProviderErrorKind::OutcomeUnknown,
                ScriptedTerminalCapture::OutcomeUnknown,
            ),
            (
                ModelStreamProviderErrorKind::DefiniteFailure,
                ScriptedTerminalCapture::ProviderFailed(
                    ModelStreamProviderErrorKind::DefiniteFailure,
                ),
            ),
        ];
        for (kind, expected_capture) in cases {
            let request = request(vec![]);
            let provider = ScriptedProvider::new(
                ProviderId::try_new("fixture").unwrap(),
                vec![ScriptedProgram {
                    expectation: expectation(&request),
                    steps: vec![
                        ScriptedStep::emit(started()),
                        ScriptedStep::emit(ModelStreamEvent::UsageUnavailable),
                        ScriptedStep::emit(ModelStreamEvent::ProviderError { kind }),
                    ],
                }],
            );
            let events = collect(
                &provider,
                invocation(request, ProviderCancellationToken::new()),
            )
            .await
            .unwrap();
            assert_ne!(
                validate_model_stream(&events).unwrap(),
                crate::domain::ModelStreamState::Completed
            );
            assert_eq!(provider.captures()[0].terminal(), Some(expected_capture));
        }
    }

    fn clocked_provider(
        clock: &Arc<TestClock>,
        request: &ModelRequest,
        steps: Vec<ScriptedStep>,
    ) -> ScriptedProvider {
        let clock_port: Arc<dyn Clock> = clock.clone();
        ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![ScriptedProgram {
                expectation: expectation(request),
                steps,
            }],
            clock_port,
        )
    }

    #[tokio::test]
    async fn scripted_overall_deadline_expires_before_first_event_without_sleep() {
        let clock = Arc::new(TestClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
            Duration::from_secs(100),
        ));
        let request = request(vec![]);
        let gate = ScriptGate::new();
        let provider = clocked_provider(
            &clock,
            &request,
            vec![
                ScriptedStep::AwaitRelease(gate.clone()),
                ScriptedStep::emit(started()),
            ],
        );
        let mut stream = provider
            .invoke_stream(invocation_with_limits(
                request,
                ProviderCancellationToken::new(),
                MonotonicInstant::from_elapsed(Duration::from_secs(105)),
                Duration::from_secs(5),
            ))
            .await
            .unwrap();
        clock.advance_monotonic(Duration::from_secs(5)).unwrap();
        gate.release();
        let error = stream.next_event().await.unwrap_err();
        assert_eq!(error.kind(), ProviderErrorKind::TimeoutBeforeOutput);
        assert!(classify_provider_retry(&error, false, 1, false, false).retryable());
        assert_eq!(
            provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::TimedOut(
                ProviderErrorKind::TimeoutBeforeOutput
            ))
        );
    }

    #[tokio::test]
    async fn scripted_idle_timeout_classification_uses_shared_semantic_predicate() {
        for semantic_event in [
            None,
            Some(ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text("partial"),
            }),
            Some(ModelStreamEvent::ToolCallStarted {
                item_ordinal: 0,
                call_id: ModelToolCallId::try_new("call-timeout").unwrap(),
                name: crate::domain::ToolName::try_new("read_file").unwrap(),
            }),
        ] {
            let clock = Arc::new(TestClock::new(
                time::OffsetDateTime::UNIX_EPOCH,
                Duration::from_secs(100),
            ));
            let request = request(vec![]);
            let gate = ScriptGate::new();
            let mut steps = vec![ScriptedStep::emit(started())];
            if let Some(event) = semantic_event.clone() {
                steps.push(ScriptedStep::emit(event));
            }
            steps.extend([
                ScriptedStep::AwaitRelease(gate.clone()),
                ScriptedStep::emit(ModelStreamEvent::UsageUnavailable),
            ]);
            let provider = clocked_provider(&clock, &request, steps);
            let mut stream = provider
                .invoke_stream(invocation_with_limits(
                    request,
                    ProviderCancellationToken::new(),
                    MonotonicInstant::from_elapsed(Duration::from_secs(200)),
                    Duration::from_secs(5),
                ))
                .await
                .unwrap();
            assert!(stream.next_event().await.unwrap().is_some());
            if semantic_event.is_some() {
                assert!(stream.next_event().await.unwrap().is_some());
            }
            clock.advance_monotonic(Duration::from_secs(5)).unwrap();
            gate.release();
            let error = stream.next_event().await.unwrap_err();
            let semantic_observed = semantic_event.is_some();
            assert_eq!(
                error.kind(),
                if semantic_observed {
                    ProviderErrorKind::TimeoutAfterOutput
                } else {
                    ProviderErrorKind::TimeoutBeforeOutput
                }
            );
            assert_eq!(
                classify_provider_retry(&error, semantic_observed, 1, false, false).retryable(),
                !semantic_observed
            );
        }
    }

    #[tokio::test]
    async fn scripted_timeout_threshold_completion_and_cancellation_precedence_are_frozen() {
        let clock = Arc::new(TestClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
            Duration::from_secs(100),
        ));
        let completion_request = request(vec![]);
        let gate = ScriptGate::new();
        let final_response = response(
            vec![ModelOutputItem::text(vec![text("done")]).unwrap()],
            ModelStopReason::Completed,
        );
        let provider = clocked_provider(
            &clock,
            &completion_request,
            vec![
                ScriptedStep::emit(started()),
                ScriptedStep::AwaitRelease(gate.clone()),
                ScriptedStep::emit(ModelStreamEvent::Usage(usage())),
                ScriptedStep::emit(ModelStreamEvent::Completed(Box::new(final_response))),
            ],
        );
        let cancellation = ProviderCancellationToken::new();
        let mut stream = provider
            .invoke_stream(invocation_with_limits(
                completion_request,
                cancellation.clone(),
                MonotonicInstant::from_elapsed(Duration::from_secs(200)),
                Duration::from_secs(5),
            ))
            .await
            .unwrap();
        assert!(stream.next_event().await.unwrap().is_some());
        clock.advance_monotonic(Duration::from_secs(4)).unwrap();
        gate.release();
        assert!(stream.next_event().await.unwrap().is_some());
        assert!(stream.next_event().await.unwrap().is_some());
        assert!(stream.next_event().await.unwrap().is_none());
        assert_eq!(
            provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::Completed)
        );

        let exact_clock = Arc::new(TestClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
            Duration::from_secs(100),
        ));
        let exact_request = request(vec![]);
        let exact_gate = ScriptGate::new();
        let exact_provider = clocked_provider(
            &exact_clock,
            &exact_request,
            vec![
                ScriptedStep::emit(started()),
                ScriptedStep::AwaitRelease(exact_gate.clone()),
                ScriptedStep::emit(ModelStreamEvent::UsageUnavailable),
            ],
        );
        let exact_cancellation = ProviderCancellationToken::new();
        let mut exact_stream = exact_provider
            .invoke_stream(invocation_with_limits(
                exact_request,
                exact_cancellation.clone(),
                MonotonicInstant::from_elapsed(Duration::from_secs(200)),
                Duration::from_secs(5),
            ))
            .await
            .unwrap();
        assert!(exact_stream.next_event().await.unwrap().is_some());
        exact_clock
            .advance_monotonic(Duration::from_secs(5))
            .unwrap();
        exact_cancellation.cancel();
        exact_gate.release();
        assert_eq!(
            exact_stream.next_event().await.unwrap_err().kind(),
            ProviderErrorKind::Cancelled
        );
        assert_eq!(
            exact_provider.captures()[0].terminal(),
            Some(ScriptedTerminalCapture::Cancelled)
        );
    }

    #[test]
    fn scripted_token_estimator_preserves_identity_units_and_exact_conservative_result() {
        let units = vec![
            TokenEstimateUnit::TextBytes(100),
            TokenEstimateUnit::ToolDefinitionBytes(40),
        ];
        let estimator = ScriptedTokenEstimator::try_new(
            TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
            vec![ScriptedEstimate {
                target_id: target().reference().model_target_id().clone(),
                units: units.clone(),
                tokens: 140,
            }],
        )
        .unwrap();
        let estimate = estimator.estimate(&target(), &units).unwrap();
        assert_eq!(estimate.estimator(), estimator.identity());
        assert_eq!(estimate.tokens(), 140);
        assert_eq!(
            estimator.captures(),
            vec![(ModelTargetId::try_new("fixture-primary").unwrap(), units)]
        );
    }

    #[test]
    fn scripted_token_estimator_is_immutable_by_canonical_input_identity() {
        let first_units = vec![TokenEstimateUnit::TextBytes(10)];
        let second_units = vec![TokenEstimateUnit::StructuredBytes(20)];
        let estimator = ScriptedTokenEstimator::try_new(
            TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
            vec![
                ScriptedEstimate {
                    target_id: target().reference().model_target_id().clone(),
                    units: first_units.clone(),
                    tokens: 10,
                },
                ScriptedEstimate {
                    target_id: target().reference().model_target_id().clone(),
                    units: second_units.clone(),
                    tokens: 25,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            estimator
                .estimate(&target(), &first_units)
                .unwrap()
                .tokens(),
            estimator
                .estimate(&target(), &first_units.clone())
                .unwrap()
                .tokens()
        );
        assert_eq!(
            estimator
                .estimate(&target(), &second_units)
                .unwrap()
                .tokens(),
            25
        );
        assert_eq!(
            estimator
                .estimate(&target(), &[TokenEstimateUnit::TextBytes(11)])
                .unwrap_err()
                .kind(),
            ProviderErrorKind::ScriptMismatch
        );
        assert_eq!(
            ScriptedTokenEstimator::try_new(
                TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
                vec![ScriptedEstimate {
                    target_id: target().reference().model_target_id().clone(),
                    units: first_units.clone(),
                    tokens: 9,
                }],
            )
            .unwrap_err()
            .kind(),
            ProviderErrorKind::InvalidScriptProgram
        );
        assert_eq!(
            ScriptedTokenEstimator::try_new(
                TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
                vec![
                    ScriptedEstimate {
                        target_id: target().reference().model_target_id().clone(),
                        units: first_units.clone(),
                        tokens: 10,
                    },
                    ScriptedEstimate {
                        target_id: target().reference().model_target_id().clone(),
                        units: first_units,
                        tokens: 11,
                    },
                ],
            )
            .unwrap_err()
            .kind(),
            ProviderErrorKind::InvalidScriptProgram
        );
    }

    #[test]
    fn scripted_capture_debug_redacts_request_event_and_argument_content() {
        let request = request(vec![]);
        let canary = "SCRIPT_CAPTURE_CANARY";
        let capture = ScriptedInvocationCapture {
            request,
            target_id: ModelTargetId::try_new("fixture-primary").unwrap(),
            request_sha256: Sha256Digest::hash_bytes(b"request"),
            invocation_ordinal: 1,
            attempt: ProviderAttempt::try_new(1).unwrap(),
            emitted_events: vec![ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text(canary),
            }],
            cancellation_observed: false,
            terminal: None,
        };
        let debug = format!("{capture:?}");
        assert!(!debug.contains(canary));
        assert!(debug.contains("emitted_event_count"));
    }
}
