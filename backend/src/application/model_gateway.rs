//! Provider-neutral durable model-attempt orchestration.

use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::Instrument;

use crate::application::observability::SafeProviderCorrelation;

use crate::application::context_assembler::ContextAssemblyResult;
use crate::application::model_selection::{ModelSelectionReason, ModelSelectionResult};
use crate::domain::model::{ModelUsage as CanonicalModelUsage, RequiredModelCapabilities};
use crate::domain::{
    AgentStepNo, ArtifactEncoding, ArtifactId, ArtifactLogicalName, ArtifactMimeType,
    ArtifactProducer, ArtifactReference, ArtifactReferenceInput, ArtifactRetention,
    ArtifactStorageKey, AttemptNo, CanonicalByteCount, CleanupStatus, ConversationId,
    CorrelationId, CraxiiId, CurrentWorkAttempt, DraftId, JournalEventId, ModelAttemptReference,
    ModelAttemptReferenceInput, ModelContractErrorKind, ModelInvocationId, ModelInvocationState,
    ModelOutputItem, ModelResponse, ModelStreamEvent, ModelStreamProviderErrorKind,
    ModelStreamState, NormalizedError, ProviderOpaqueEvidence, UtcTimestamp, WorkId,
    WorkInterruptionReason, WorkLifecycleSnapshot, WorkTransitionGuard, WorkTransitionRequest,
    decide_work_transition, validate_model_stream,
};
use crate::ports::artifact_store::{ArtifactStore, BeginArtifactCapture};
use crate::ports::clock::{Clock, MonotonicInstant};
use crate::ports::model_provider::{
    BackoffDecision, DEFAULT_PROVIDER_IDLE_TIMEOUT, DEFAULT_PROVIDER_INVOCATION_LIMIT,
    FullJitterSource, MAX_PROVIDER_ATTEMPTS, ModelInvocationControl, ModelProvider,
    ModelProviderInvocation, ModelUsageStatus, ProviderAttempt, ProviderCancellationToken,
    ProviderError, ProviderErrorKind, ProviderOutcomeCertainty, ProviderRetryEvidence,
    classify_provider_retry, provider_backoff,
};
use crate::ports::state_store::{
    BeginModelInvocationRequest, EventIntent, FinishModelInvocationRequest, LoadOwnedWorkRequest,
    MarkModelStreamingRequest, ModelExpectation, ModelSelectionReason as StoredSelectionReason,
    ModelStateStore, ModelStreamingObservation, ModelTerminalOutcome,
    ModelUsage as StoredModelUsage, NormalizedModelOutput, NormalizedModelOutputItem,
    PreparedArtifact, PreparedModelInvocation, ProviderOption, ProviderOptionValue,
    RequiredModelCapabilities as StoredRequiredModelCapabilities, StateStoreError, WorkExpectation,
};

const MAX_STREAM_EVENTS: usize = 4_096;

/// Provider-neutral, already-validated semantic delta offered to future delivery code.
#[derive(Clone, Eq, PartialEq)]
pub enum CanonicalDraftDelta {
    Text { text: String },
    Refusal { text: String },
}

impl fmt::Debug for CanonicalDraftDelta {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text } => formatter
                .debug_struct("CanonicalDraftDelta::Text")
                .field("text_bytes", &text.len())
                .finish(),
            Self::Refusal { text } => formatter
                .debug_struct("CanonicalDraftDelta::Refusal")
                .field("text_bytes", &text.len())
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DraftIdentity {
    pub conversation_id: ConversationId,
    pub work_id: WorkId,
    pub invocation_id: ModelInvocationId,
    pub draft_id: DraftId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftExposure {
    NotExposed,
    Exposed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftAbandonCause {
    ToolContinuation,
    Superseded,
    Cancelled,
    Failed,
    Interrupted,
    DeliveryLimit,
}

/// Stage 20 seam. Implementations must not treat a draft as conversation history.
pub trait DraftSink: Send + Sync {
    /// Immediate best-effort offer; it must never wait for client I/O.
    fn offer(&self, identity: DraftIdentity, delta: CanonicalDraftDelta) -> DraftExposure;
    fn abandon(&self, invocation_id: ModelInvocationId, reason: DraftAbandonCause);
    fn finalize_work(&self, work_id: WorkId);
}

/// Test/offline composition that deliberately exposes no draft output.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopDraftSink;

impl DraftSink for NoopDraftSink {
    fn offer(&self, _: DraftIdentity, _: CanonicalDraftDelta) -> DraftExposure {
        DraftExposure::NotExposed
    }

    fn abandon(&self, _: ModelInvocationId, _: DraftAbandonCause) {}

    fn finalize_work(&self, _: WorkId) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelGatewayLimits {
    pub maximum_attempts_per_logical_invocation: u32,
    pub maximum_attempts_per_work: u32,
    pub provider_invocation_limit: Duration,
    pub stream_idle_limit: Duration,
}

impl Default for ModelGatewayLimits {
    fn default() -> Self {
        Self {
            maximum_attempts_per_logical_invocation: MAX_PROVIDER_ATTEMPTS,
            maximum_attempts_per_work: 32,
            provider_invocation_limit: DEFAULT_PROVIDER_INVOCATION_LIMIT,
            stream_idle_limit: DEFAULT_PROVIDER_IDLE_TIMEOUT,
        }
    }
}

impl ModelGatewayLimits {
    fn validate(self) -> Result<Self, ModelGatewayError> {
        if self.maximum_attempts_per_logical_invocation != MAX_PROVIDER_ATTEMPTS
            || self.maximum_attempts_per_work != 32
            || self.provider_invocation_limit != DEFAULT_PROVIDER_INVOCATION_LIMIT
            || self.stream_idle_limit != DEFAULT_PROVIDER_IDLE_TIMEOUT
        {
            return Err(ModelGatewayError::InvalidComposition);
        }
        Ok(self)
    }
}

pub struct GatewayInvocation {
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub work: WorkLifecycleSnapshot,
    pub context: ContextAssemblyResult,
    pub selection: ModelSelectionResult,
    pub agent_step: AgentStepNo,
    pub correlation_id: CorrelationId,
    pub causation_event_id: JournalEventId,
    pub cancellation: tokio::sync::watch::Receiver<bool>,
    pub work_deadline: MonotonicInstant,
    pub shutdown_deadline: Option<MonotonicInstant>,
    pub work_attempts_before_invocation: u32,
}

#[derive(Debug)]
pub struct DurableModelAttempt {
    pub model_invocation_id: ModelInvocationId,
    pub logical_invocation_id: crate::domain::LogicalInvocationId,
    pub attempt_no: u32,
    pub terminal_model_event_id: JournalEventId,
    pub terminal_work_event_id: JournalEventId,
    pub work: WorkLifecycleSnapshot,
}

#[derive(Debug)]
pub enum DurableModelOutcome {
    Completed {
        response: Box<ModelResponse>,
        attempt: DurableModelAttempt,
    },
    Failed {
        error_kind: ProviderErrorKind,
        semantic_output_observed: bool,
        retries_exhausted: bool,
        attempt: DurableModelAttempt,
    },
    Interrupted {
        error_kind: ProviderErrorKind,
        attempt: DurableModelAttempt,
    },
    CancelledBeforeAttempt {
        work: WorkLifecycleSnapshot,
    },
    DeadlineBeforeAttempt {
        work: WorkLifecycleSnapshot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelGatewayError {
    InvalidComposition,
    InvalidInvocation,
    StateStore,
    Clock,
    Artifact,
    Lifecycle,
}

impl Display for ModelGatewayError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidComposition => "invalid model gateway composition",
            Self::InvalidInvocation => "invalid model gateway invocation",
            Self::StateStore => "model gateway state-store failure",
            Self::Clock => "model gateway clock failure",
            Self::Artifact => "model gateway artifact failure",
            Self::Lifecycle => "model gateway lifecycle failure",
        })
    }
}

impl std::error::Error for ModelGatewayError {}

impl From<StateStoreError> for ModelGatewayError {
    fn from(_: StateStoreError) -> Self {
        Self::StateStore
    }
}

/// Sole application owner of physical provider attempts and retry orchestration.
pub struct ModelGateway {
    state_store: Arc<dyn ModelStateStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    provider: Arc<dyn ModelProvider>,
    draft_sink: Arc<dyn DraftSink>,
    clock: Arc<dyn Clock>,
    jitter: Mutex<Box<dyn FullJitterSource + Send>>,
    limits: ModelGatewayLimits,
}

impl fmt::Debug for ModelGateway {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelGateway")
            .field("provider_id", self.provider.provider_id())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ModelGateway {
    pub fn new(
        state_store: Arc<dyn ModelStateStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        provider: Arc<dyn ModelProvider>,
        draft_sink: Arc<dyn DraftSink>,
        clock: Arc<dyn Clock>,
        jitter: Box<dyn FullJitterSource + Send>,
        limits: ModelGatewayLimits,
    ) -> Result<Self, ModelGatewayError> {
        Ok(Self {
            state_store,
            artifact_store,
            provider,
            draft_sink,
            clock,
            jitter: Mutex::new(jitter),
            limits: limits.validate()?,
        })
    }

    pub fn abandon_draft(&self, invocation_id: ModelInvocationId, reason: DraftAbandonCause) {
        self.draft_sink.abandon(invocation_id, reason);
    }

    pub fn finalize_drafts_for_work(&self, work_id: WorkId) {
        self.draft_sink.finalize_work(work_id);
    }

    /// Invokes one exact Stage 16 logical request, durably recording every physical attempt.
    pub async fn invoke(
        &self,
        mut invocation: GatewayInvocation,
    ) -> Result<DurableModelOutcome, ModelGatewayError> {
        self.validate_invocation(&invocation)?;
        if *invocation.cancellation.borrow() {
            return Ok(DurableModelOutcome::CancelledBeforeAttempt {
                work: invocation.work,
            });
        }

        let logical_id = invocation.context.package().logical_invocation_id();
        let mut attempt_no = 1_u32;
        let mut retry_of: Option<ModelInvocationId> = None;
        let mut retry_evidence: Option<ProviderRetryEvidence> = None;
        let mut preceding_event = invocation.causation_event_id;

        loop {
            if invocation
                .work_attempts_before_invocation
                .saturating_add(attempt_no)
                > self.limits.maximum_attempts_per_work
            {
                return Err(ModelGatewayError::InvalidInvocation);
            }
            if *invocation.cancellation.borrow() {
                return Ok(DurableModelOutcome::CancelledBeforeAttempt {
                    work: invocation.work,
                });
            }
            if self.clock.monotonic_now() >= invocation.work_deadline {
                return Ok(DurableModelOutcome::DeadlineBeforeAttempt {
                    work: invocation.work,
                });
            }

            let attempt_id = ModelInvocationId::generate();
            let started_at = self.wall_now()?;
            let target = invocation.selection.selected_target().reference();
            let attempt_observation = ModelAttemptObservation {
                work_id: invocation.work.work_id().to_string(),
                logical_invocation_id: logical_id.to_string(),
                model_invocation_id: attempt_id.to_string(),
                attempt_ordinal: attempt_no,
                target: target.model_target_id().as_str().to_owned(),
                provider: target.provider_id().as_str().to_owned(),
                model: target.provider_model_id().as_str().to_owned(),
                request_sha256: invocation.context.request().canonical_sha256().to_string(),
                request_bytes: invocation.context.budget().request_serialized_bytes,
                retry_of_invocation_id: retry_of.map(|value| value.to_string()),
                retry_reason: retry_evidence.map(|value| value.reason.as_str()),
                retry_delay_ms: retry_evidence
                    .map(|value| u64::try_from(value.delay.as_millis()).unwrap_or(u64::MAX)),
            };
            let attempt_span = tracing::info_span!(
                "model_invocation_attempt",
                craxii_id = %invocation.craxii_id,
                conversation_id = %invocation.conversation_id,
                work_id = attempt_observation.work_id.as_str(),
                runtime_instance_id = %invocation.work.runtime_owner().ok_or(ModelGatewayError::InvalidInvocation)?,
                logical_invocation_id = attempt_observation.logical_invocation_id.as_str(),
                model_invocation_id = attempt_observation.model_invocation_id.as_str(),
                agent_step = invocation.agent_step.get(),
                attempt_ordinal = attempt_observation.attempt_ordinal,
                target = attempt_observation.target.as_str(),
                provider = attempt_observation.provider.as_str(),
                model = attempt_observation.model.as_str(),
                request_sha256 = attempt_observation.request_sha256.as_str(),
                request_bytes = attempt_observation.request_bytes,
                retry_of_invocation_id = attempt_observation.retry_of_invocation_id.as_deref(),
                retry_reason = attempt_observation.retry_reason,
                retry_delay_ms = attempt_observation.retry_delay_ms,
                result_class = tracing::field::Empty,
                certainty = tracing::field::Empty,
                provider_error_kind = tracing::field::Empty,
                provider_http_status = tracing::field::Empty,
                total_latency_ms = tracing::field::Empty,
                first_response_latency_ms = tracing::field::Empty,
                first_semantic_output_latency_ms = tracing::field::Empty,
                output_item_count = tracing::field::Empty,
                tool_call_count = tracing::field::Empty,
                stop_reason = tracing::field::Empty,
                draft_exposed = tracing::field::Empty,
                input_tokens = tracing::field::Empty,
                cached_input_tokens = tracing::field::Empty,
                output_tokens = tracing::field::Empty,
                reasoning_tokens = tracing::field::Empty,
                total_tokens = tracing::field::Empty,
                provider_request_digest = tracing::field::Empty,
                provider_response_digest = tracing::field::Empty,
            );
            let attempt_started = Instant::now();
            let wait = decide_work_transition(
                &invocation.work,
                WorkTransitionGuard::for_snapshot(&invocation.work),
                WorkTransitionRequest::WaitForModel {
                    model_invocation_id: attempt_id,
                },
            )
            .map_err(|_| ModelGatewayError::Lifecycle)?
            .into_next();
            let retained_wait = decide_work_transition(
                &invocation.work,
                WorkTransitionGuard::for_snapshot(&invocation.work),
                WorkTransitionRequest::WaitForModel {
                    model_invocation_id: attempt_id,
                },
            )
            .map_err(|_| ModelGatewayError::Lifecycle)?
            .into_next();
            let started_event = JournalEventId::generate();
            let waiting_event = JournalEventId::generate();
            let attempt_reference = ModelAttemptReference::new(ModelAttemptReferenceInput {
                logical_invocation_id: logical_id,
                model_invocation_id: attempt_id,
                work_id: invocation.work.work_id(),
                runtime_instance_id: invocation
                    .work
                    .runtime_owner()
                    .ok_or(ModelGatewayError::InvalidInvocation)?,
                context_manifest_id: invocation.context.package().context_manifest_id(),
                agent_step_no: invocation.agent_step,
                attempt_no: AttemptNo::try_new(i64::from(attempt_no))
                    .map_err(|_| ModelGatewayError::InvalidInvocation)?,
                provider_model: invocation.selection.selected_target().reference().clone(),
                retry_of,
            });
            self.state_store
                .begin_model_invocation(BeginModelInvocationRequest {
                    expected_work: WorkExpectation::for_snapshot(&invocation.work),
                    manifest: invocation.context.prepared_manifest().clone(),
                    invocation: PreparedModelInvocation {
                        attempt: attempt_reference,
                        selection_reason: stored_selection_reason(invocation.selection.reason()),
                        required_capabilities: stored_required_capabilities(
                            invocation.selection.required_capabilities(),
                        ),
                        provider_options: vec![ProviderOption {
                            key: "reasoning_continuation".to_owned(),
                            value: ProviderOptionValue::Boolean(
                                invocation
                                    .context
                                    .package()
                                    .provider_native_options()
                                    .reasoning_continuation(),
                            ),
                        }],
                        request_sha256: invocation.context.request().canonical_sha256(),
                        request_artifact_id: None,
                        retry_evidence,
                        started_at,
                    },
                    artifacts: Vec::new(),
                    work_next: wait,
                    invocation_event: EventIntent {
                        event_id: started_event,
                        correlation_id: invocation.correlation_id,
                        causation_event_id: Some(preceding_event),
                    },
                    work_event: EventIntent {
                        event_id: waiting_event,
                        correlation_id: invocation.correlation_id,
                        causation_event_id: Some(started_event),
                    },
                })
                .await?;
            invocation.work = retained_wait;
            #[cfg(feature = "test-failpoints")]
            crate::test_failpoints::reach(
                crate::test_failpoints::PhysicalHook::ModelAttemptAfterCommitBeforeProviderIo,
            );

            let provider_attempt = ProviderAttempt::try_new(attempt_no)
                .map_err(|_| ModelGatewayError::InvalidInvocation)?;
            let attempt_result = self
                .run_physical_attempt(
                    &mut invocation,
                    provider_attempt,
                    attempt_id,
                    started_event,
                    waiting_event,
                )
                .instrument(attempt_span.clone())
                .await;

            let total_latency_ms =
                u64::try_from(attempt_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            attempt_span.record("total_latency_ms", total_latency_ms);
            attempt_span.in_scope(|| {
                observe_model_attempt(
                    &attempt_span,
                    &attempt_observation,
                    total_latency_ms,
                    &attempt_result,
                );
            });

            match attempt_result {
                PhysicalAttemptResult::Completed { response, attempt } => {
                    return Ok(DurableModelOutcome::Completed { response, attempt });
                }
                PhysicalAttemptResult::Interrupted {
                    error_kind,
                    attempt,
                } => {
                    let reason = if error_kind == ProviderErrorKind::Cancelled {
                        DraftAbandonCause::Cancelled
                    } else {
                        DraftAbandonCause::Interrupted
                    };
                    self.draft_sink.abandon(attempt.model_invocation_id, reason);
                    return Ok(DurableModelOutcome::Interrupted {
                        error_kind,
                        attempt,
                    });
                }
                PhysicalAttemptResult::Failed {
                    error,
                    semantic_output_observed,
                    draft_exposed,
                    attempt,
                } => {
                    if error.kind() == ProviderErrorKind::Cancelled
                        && error.certainty() == ProviderOutcomeCertainty::DefinitelyNotSent
                    {
                        self.draft_sink
                            .abandon(attempt.model_invocation_id, DraftAbandonCause::Cancelled);
                        return Ok(DurableModelOutcome::CancelledBeforeAttempt {
                            work: attempt.work,
                        });
                    }
                    let decision = classify_provider_retry(
                        &error,
                        semantic_output_observed,
                        attempt_no,
                        *invocation.cancellation.borrow(),
                        self.clock.monotonic_now() >= invocation.work_deadline,
                    );
                    let cap_reached = attempt_no
                        >= self.limits.maximum_attempts_per_logical_invocation
                        || invocation
                            .work_attempts_before_invocation
                            .saturating_add(attempt_no)
                            >= self.limits.maximum_attempts_per_work;
                    let transient_before_output = matches!(
                        error.kind(),
                        ProviderErrorKind::RateLimited
                            | ProviderErrorKind::TemporarilyUnavailable
                            | ProviderErrorKind::TransportBeforeResponse
                            | ProviderErrorKind::TimeoutBeforeOutput
                    ) && matches!(
                        error.certainty(),
                        ProviderOutcomeCertainty::DefinitelyNotSent
                            | ProviderOutcomeCertainty::DefiniteProviderFailure
                    ) && !semantic_output_observed;
                    if !decision.retryable() || draft_exposed || cap_reached {
                        if draft_exposed {
                            let reason = if error.kind() == ProviderErrorKind::Cancelled {
                                DraftAbandonCause::Cancelled
                            } else {
                                DraftAbandonCause::Failed
                            };
                            self.draft_sink.abandon(attempt.model_invocation_id, reason);
                        }
                        return Ok(DurableModelOutcome::Failed {
                            error_kind: error.kind(),
                            semantic_output_observed,
                            retries_exhausted: transient_before_output && cap_reached,
                            attempt,
                        });
                    }
                    let remaining = invocation
                        .work_deadline
                        .checked_duration_since(self.clock.monotonic_now());
                    let backoff = {
                        let mut jitter = self
                            .jitter
                            .lock()
                            .map_err(|_| ModelGatewayError::InvalidComposition)?;
                        provider_backoff(
                            attempt_no,
                            decision.provider_retry_after(),
                            jitter.as_mut(),
                            *invocation.cancellation.borrow(),
                            remaining,
                        )
                    };
                    let BackoffDecision::Delay(delay) = backoff else {
                        return Ok(DurableModelOutcome::Failed {
                            error_kind: error.kind(),
                            semantic_output_observed,
                            retries_exhausted: true,
                            attempt,
                        });
                    };
                    attempt_span.in_scope(|| {
                        observe_model_retry_scheduled(
                            &attempt_observation,
                            total_latency_ms,
                            error.kind(),
                            error.certainty(),
                            decision.reason().as_str(),
                            delay,
                        );
                    });
                    preceding_event = attempt.terminal_work_event_id;
                    retry_of = Some(attempt.model_invocation_id);
                    invocation.work = attempt.work;
                    if !delay.is_zero()
                        && wait_for_delay_or_cancellation(
                            delay,
                            &mut invocation.cancellation,
                            invocation.work_deadline,
                            self.clock.as_ref(),
                        )
                        .await
                    {
                        return Ok(if *invocation.cancellation.borrow() {
                            DurableModelOutcome::CancelledBeforeAttempt {
                                work: invocation.work,
                            }
                        } else {
                            DurableModelOutcome::DeadlineBeforeAttempt {
                                work: invocation.work,
                            }
                        });
                    }
                    retry_evidence = Some(ProviderRetryEvidence {
                        reason: decision.reason(),
                        delay,
                        provider_retry_after: decision.provider_retry_after(),
                    });
                    attempt_no = attempt_no.saturating_add(1);
                }
                PhysicalAttemptResult::Infrastructure(error) => {
                    self.draft_sink
                        .abandon(attempt_id, DraftAbandonCause::Interrupted);
                    return Err(error);
                }
            }
        }
    }

    fn validate_invocation(&self, invocation: &GatewayInvocation) -> Result<(), ModelGatewayError> {
        let target = invocation.selection.selected_target();
        let capabilities = self
            .provider
            .capabilities(target)
            .map_err(|_| ModelGatewayError::InvalidInvocation)?;
        if invocation.work.state() != crate::domain::WorkState::Running
            || invocation.work.runtime_owner().is_none()
            || invocation.work.current_attempt() != crate::domain::CurrentWorkAttempt::None
            || invocation.context.request().target() != target
            || invocation.context.package().selected_target() != &invocation.selection
            || self.provider.provider_id() != target.reference().provider_id()
            || capabilities != *target.reference().capabilities()
            || invocation.work_attempts_before_invocation >= self.limits.maximum_attempts_per_work
        {
            return Err(ModelGatewayError::InvalidInvocation);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_physical_attempt(
        &self,
        invocation: &mut GatewayInvocation,
        provider_attempt: ProviderAttempt,
        attempt_id: ModelInvocationId,
        started_event: JournalEventId,
        waiting_event: JournalEventId,
    ) -> PhysicalAttemptResult {
        let attempt_no = provider_attempt.get();
        let started_mono = self.clock.monotonic_now();
        let Some(local_deadline) = started_mono.checked_add(self.limits.provider_invocation_limit)
        else {
            return PhysicalAttemptResult::Infrastructure(ModelGatewayError::Clock);
        };
        let effective_deadline = invocation
            .shutdown_deadline
            .into_iter()
            .chain([invocation.work_deadline, local_deadline])
            .min()
            .expect("fixed provider and Work deadlines exist");
        let cancellation = ProviderCancellationToken::new();
        let control = match ModelInvocationControl::try_new(
            cancellation.clone(),
            effective_deadline,
            self.limits.stream_idle_limit,
        ) {
            Ok(value) => value,
            Err(_) => {
                return PhysicalAttemptResult::Infrastructure(
                    ModelGatewayError::InvalidComposition,
                );
            }
        };
        let provider_invocation = ModelProviderInvocation {
            request: invocation.context.request().clone(),
            attempt: provider_attempt,
            control,
            fixture_key: None,
        };
        let remaining = remaining_duration(effective_deadline, self.clock.as_ref());
        if remaining.is_zero() {
            let error = ProviderError::new(
                ProviderErrorKind::TimeoutBeforeOutput,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            );
            return self
                .finish_failed_attempt(
                    invocation,
                    attempt_id,
                    attempt_no,
                    ModelInvocationState::Requesting,
                    waiting_event,
                    error,
                    false,
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
        }
        if *invocation.cancellation.borrow() {
            cancellation.cancel();
            let error = ProviderError::new(
                ProviderErrorKind::Cancelled,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            );
            return self
                .finish_failed_attempt(
                    invocation,
                    attempt_id,
                    attempt_no,
                    ModelInvocationState::Requesting,
                    waiting_event,
                    error,
                    false,
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
        }

        let attempt_observation_span = tracing::Span::current();
        let provider_span = tracing::info_span!(
            "provider_stream",
            provider = self.provider.provider_id().as_str(),
            model_invocation_id = %attempt_id,
            attempt_ordinal = attempt_no,
            first_response_latency_ms = tracing::field::Empty,
            first_semantic_output_latency_ms = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
            result_class = tracing::field::Empty,
        );
        let mut provider_observation =
            ProviderStreamObservation::new(provider_span.clone(), attempt_observation_span);
        let stream = tokio::select! {
            biased;
            () = cancellation_requested(&mut invocation.cancellation) => {
                provider_observation.classify("cancelled");
                cancellation.cancel();
                let error = ProviderError::new(
                    ProviderErrorKind::Cancelled,
                    ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                );
                return self.finish_failed_attempt(
                    invocation,
                    attempt_id,
                    attempt_no,
                    ModelInvocationState::Requesting,
                    waiting_event,
                    error,
                    false,
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                ).await;
            }
            () = tokio::time::sleep(remaining) => {
                provider_observation.classify("timeout");
                cancellation.cancel();
                let error = ProviderError::new(
                    ProviderErrorKind::TimeoutBeforeOutput,
                    ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                );
                return self.finish_failed_attempt(
                    invocation,
                    attempt_id,
                    attempt_no,
                    ModelInvocationState::Requesting,
                    waiting_event,
                    error,
                    false,
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                ).await;
            }
            result = self.provider.invoke_stream(provider_invocation).instrument(provider_span.clone()) => result,
        };
        let mut stream = match stream {
            Ok(value) => value,
            Err(error) => {
                provider_observation.classify("open_failed");
                return self
                    .finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        ModelInvocationState::Requesting,
                        waiting_event,
                        error,
                        false,
                        false,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await;
            }
        };

        let mut accumulator = CanonicalStreamAccumulator::new();
        let mut model_state = ModelInvocationState::Requesting;
        let mut last_attempt_event = started_event;
        let mut first_byte_at = None;
        let mut first_output_at = None;
        let mut draft_exposed = false;
        let draft_identity = DraftIdentity {
            conversation_id: invocation.conversation_id,
            work_id: invocation.work.work_id(),
            invocation_id: attempt_id,
            draft_id: DraftId::generate(),
        };
        #[cfg(feature = "test-failpoints")]
        let mut first_semantic_delta_observed = false;

        loop {
            let remaining = remaining_duration(effective_deadline, self.clock.as_ref());
            if remaining.is_zero() {
                provider_observation.classify("timeout");
                cancellation.cancel();
                let error = timeout_error(accumulator.semantic_output_observed());
                return self
                    .finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    )
                    .await;
            }
            let idle = self.limits.stream_idle_limit.min(remaining);
            let event = tokio::select! {
                biased;
                () = cancellation_requested(&mut invocation.cancellation) => {
                    provider_observation.classify("cancelled");
                    cancellation.cancel();
                    let error = ProviderError::new(
                        ProviderErrorKind::Cancelled,
                        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                    );
                    return self.finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    ).await;
                }
                () = tokio::time::sleep(idle) => {
                    provider_observation.classify("timeout");
                    cancellation.cancel();
                    let error = timeout_error(accumulator.semantic_output_observed());
                    return self.finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    ).await;
                }
                result = stream.next_event().instrument(provider_span.clone()) => result,
            };
            let event = match event {
                Ok(Some(value)) => value,
                Ok(None) => {
                    provider_observation.classify("unexpected_end");
                    let error = ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        if accumulator.semantic_output_observed() {
                            ProviderOutcomeCertainty::SemanticOutputObserved
                        } else {
                            ProviderOutcomeCertainty::ProviderOutcomeUnknown
                        },
                    );
                    return self
                        .finish_provider_error(
                            invocation,
                            attempt_id,
                            attempt_no,
                            model_state,
                            last_attempt_event,
                            error,
                            accumulator.semantic_output_observed(),
                            draft_exposed,
                            first_byte_at,
                            first_output_at,
                            accumulator.provider_request_id(),
                            accumulator.provider_response_id(),
                            accumulator.usage(),
                        )
                        .await;
                }
                Err(error) => {
                    provider_observation.classify("stream_failed");
                    return self
                        .finish_provider_error(
                            invocation,
                            attempt_id,
                            attempt_no,
                            model_state,
                            last_attempt_event,
                            error,
                            accumulator.semantic_output_observed(),
                            draft_exposed,
                            first_byte_at,
                            first_output_at,
                            accumulator.provider_request_id(),
                            accumulator.provider_response_id(),
                            accumulator.usage(),
                        )
                        .await;
                }
            };

            let observed_at = match self.wall_now() {
                Ok(value) => value,
                Err(error) => return PhysicalAttemptResult::Infrastructure(error),
            };
            if first_byte_at.is_none() {
                first_byte_at = Some(observed_at);
                provider_observation.observe_first_response();
                let (request_id, response_id) = started_ids(&event);
                let streaming_event = JournalEventId::generate();
                if let Err(error) = self
                    .state_store
                    .mark_model_streaming(MarkModelStreamingRequest {
                        expected_work: WorkExpectation::for_snapshot(&invocation.work),
                        expected_model: ModelExpectation {
                            model_invocation_id: attempt_id,
                            state: ModelInvocationState::Requesting,
                        },
                        observation: ModelStreamingObservation {
                            first_byte_at: observed_at,
                            first_output_at: event.is_semantic_output().then_some(observed_at),
                            provider_request_id: request_id,
                            provider_response_id: response_id,
                            draft_exposed: false,
                        },
                        event: EventIntent {
                            event_id: streaming_event,
                            correlation_id: invocation.correlation_id,
                            causation_event_id: Some(last_attempt_event),
                        },
                    })
                    .await
                {
                    return PhysicalAttemptResult::Infrastructure(error.into());
                }
                model_state = ModelInvocationState::Streaming;
                last_attempt_event = streaming_event;
            }
            if event.is_semantic_output() && first_output_at.is_none() {
                first_output_at = Some(observed_at);
                provider_observation.observe_first_semantic_output();
            }
            #[cfg(feature = "test-failpoints")]
            if event.is_semantic_output() && !first_semantic_delta_observed {
                first_semantic_delta_observed = true;
                crate::test_failpoints::reach(
                    crate::test_failpoints::PhysicalHook::AfterFirstProviderDelta,
                );
            }
            if let Some(delta) = draft_delta(&event) {
                match self.draft_sink.offer(draft_identity, delta) {
                    DraftExposure::Exposed => draft_exposed = true,
                    DraftExposure::NotExposed => {}
                }
            }
            let terminal = event.is_terminal();
            if let Err(kind) = accumulator.observe(event) {
                provider_observation.classify("malformed_stream");
                let error = ProviderError::new(
                    contract_error_kind(kind),
                    if accumulator.semantic_output_observed() {
                        ProviderOutcomeCertainty::SemanticOutputObserved
                    } else {
                        ProviderOutcomeCertainty::ProviderOutcomeUnknown
                    },
                );
                return self
                    .finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    )
                    .await;
            }
            if !terminal {
                continue;
            }
            // A terminal item is authoritative only after the provider stream closes. This
            // proves there is exactly one terminal result; a stalled or failed tail is the same
            // conservative crash window as a terminal response not yet durably committed.
            let trailing_remaining = remaining_duration(effective_deadline, self.clock.as_ref());
            let trailing_idle = self.limits.stream_idle_limit.min(trailing_remaining);
            let trailing = if trailing_idle.is_zero() {
                None
            } else {
                tokio::select! {
                    biased;
                    () = cancellation_requested(&mut invocation.cancellation) => {
                        cancellation.cancel();
                        None
                    }
                    () = tokio::time::sleep(trailing_idle) => {
                        cancellation.cancel();
                        None
                    }
                    result = stream.next_event().instrument(provider_span.clone()) => Some(result),
                }
            };
            match trailing {
                Some(Ok(None)) => {}
                Some(Ok(Some(_))) => {
                    provider_observation.classify("malformed_stream");
                    let error = ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        if accumulator.semantic_output_observed() {
                            ProviderOutcomeCertainty::SemanticOutputObserved
                        } else {
                            ProviderOutcomeCertainty::DefiniteProviderFailure
                        },
                    );
                    return self
                        .finish_provider_error(
                            invocation,
                            attempt_id,
                            attempt_no,
                            model_state,
                            last_attempt_event,
                            error,
                            accumulator.semantic_output_observed(),
                            draft_exposed,
                            first_byte_at,
                            first_output_at,
                            accumulator.provider_request_id(),
                            accumulator.provider_response_id(),
                            accumulator.usage(),
                        )
                        .await;
                }
                Some(Err(_)) | None => {
                    provider_observation.classify("incomplete_stream");
                    let error = ProviderError::new(
                        ProviderErrorKind::ProviderOutcomeUnknown,
                        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                    );
                    return self
                        .finish_provider_error(
                            invocation,
                            attempt_id,
                            attempt_no,
                            model_state,
                            last_attempt_event,
                            error,
                            accumulator.semantic_output_observed(),
                            draft_exposed,
                            first_byte_at,
                            first_output_at,
                            accumulator.provider_request_id(),
                            accumulator.provider_response_id(),
                            accumulator.usage(),
                        )
                        .await;
                }
            }
            return match accumulator.finish() {
                Ok(StreamTerminal::Completed(response)) => {
                    if first_output_at.is_none() && !response.output_items().is_empty() {
                        first_output_at = Some(observed_at);
                        provider_observation.observe_first_semantic_output();
                    }
                    provider_observation.classify("completed");
                    self.finish_completed_attempt(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        *response,
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                    )
                    .await
                }
                Ok(StreamTerminal::ProviderError(kind)) => {
                    provider_observation.classify("provider_error");
                    let error = stream_error(kind, accumulator.semantic_output_observed());
                    self.finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    )
                    .await
                }
                Err(kind) => {
                    provider_observation.classify("malformed_stream");
                    let error = ProviderError::new(
                        contract_error_kind(kind),
                        if accumulator.semantic_output_observed() {
                            ProviderOutcomeCertainty::SemanticOutputObserved
                        } else {
                            ProviderOutcomeCertainty::ProviderOutcomeUnknown
                        },
                    );
                    self.finish_provider_error(
                        invocation,
                        attempt_id,
                        attempt_no,
                        model_state,
                        last_attempt_event,
                        error,
                        accumulator.semantic_output_observed(),
                        draft_exposed,
                        first_byte_at,
                        first_output_at,
                        accumulator.provider_request_id(),
                        accumulator.provider_response_id(),
                        accumulator.usage(),
                    )
                    .await
                }
            };
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_completed_attempt(
        &self,
        invocation: &mut GatewayInvocation,
        attempt_id: ModelInvocationId,
        attempt_no: u32,
        model_state: ModelInvocationState,
        mut cause: JournalEventId,
        response: ModelResponse,
        draft_exposed: bool,
        first_byte_at: Option<UtcTimestamp>,
        first_output_at: Option<UtcTimestamp>,
    ) -> PhysicalAttemptResult {
        if *invocation.cancellation.borrow() {
            cause = self
                .refresh_durable_cancellation(invocation, attempt_id)
                .await
                .unwrap_or(cause);
        }
        let completed_at = match self.wall_now() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let (normalized_output, artifacts) =
            match self.prepare_normalized_output(invocation, attempt_id, &response, completed_at) {
                Ok(value) => value,
                Err(_) => {
                    let error = ProviderError::new(
                        ProviderErrorKind::ProviderOutcomeUnknown,
                        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                    );
                    return self
                        .finish_failed_attempt(
                            invocation,
                            attempt_id,
                            attempt_no,
                            model_state,
                            cause,
                            error,
                            true,
                            draft_exposed,
                            first_byte_at,
                            first_output_at,
                            response
                                .provider_request_id()
                                .map(|value| value.as_str().to_owned()),
                            response
                                .provider_response_id()
                                .map(|value| value.as_str().to_owned()),
                            response.usage(),
                        )
                        .await;
                }
            };
        let completion_transition = || {
            if invocation.work.state() == crate::domain::WorkState::CancelRequested {
                let reason = invocation
                    .work
                    .cancellation_reason()
                    .ok_or(ModelGatewayError::Lifecycle)?;
                decide_work_transition(
                    &invocation.work,
                    WorkTransitionGuard::for_snapshot(&invocation.work),
                    WorkTransitionRequest::Cancel {
                        reason,
                        cleanup_status: CleanupStatus::Confirmed,
                    },
                )
                .map_err(|_| ModelGatewayError::Lifecycle)
                .map(|value| value.into_next())
            } else {
                resume_from_model(&invocation.work, attempt_id)
            }
        };
        let resumed = match completion_transition() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let retained_resumed = match completion_transition() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let model_event = JournalEventId::generate();
        let work_event = JournalEventId::generate();
        let usage = response.usage();
        let outcome = ModelTerminalOutcome {
            state: ModelInvocationState::Completed,
            response_sha256: Some(response.canonical_sha256()),
            response_artifact_id: None,
            normalized_output: Some(normalized_output),
            provider_request_id: response
                .provider_request_id()
                .map(|value| value.as_str().to_owned()),
            provider_response_id: response
                .provider_response_id()
                .map(|value| value.as_str().to_owned()),
            first_byte_at,
            first_output_at,
            completed_at,
            usage: usage.map(stored_usage),
            usage_status: if usage.is_some() {
                ModelUsageStatus::Reported
            } else {
                ModelUsageStatus::Unavailable
            },
            provider_error_kind: None,
            provider_outcome_certainty: ProviderOutcomeCertainty::DefinitelyCompleted,
            billing_ambiguity: false,
            stop_reason: Some(response.stop_reason().as_str().to_owned()),
            tool_call_count: Some(
                u64::try_from(
                    response
                        .output_items()
                        .iter()
                        .filter(|item| item.tool_call().is_some())
                        .count(),
                )
                .unwrap_or(u64::MAX),
            ),
            draft_exposed,
            normalized_error: None,
        };
        if let Err(error) = self
            .state_store
            .finish_model_invocation(FinishModelInvocationRequest {
                expected_work: WorkExpectation::for_snapshot(&invocation.work),
                expected_model: ModelExpectation {
                    model_invocation_id: attempt_id,
                    state: model_state,
                },
                outcome,
                artifacts,
                work_next: resumed,
                model_event: EventIntent {
                    event_id: model_event,
                    correlation_id: invocation.correlation_id,
                    causation_event_id: Some(cause),
                },
                work_event: EventIntent {
                    event_id: work_event,
                    correlation_id: invocation.correlation_id,
                    causation_event_id: Some(model_event),
                },
            })
            .await
        {
            return PhysicalAttemptResult::Infrastructure(error.into());
        }
        #[cfg(feature = "test-failpoints")]
        crate::test_failpoints::reach(
            crate::test_failpoints::PhysicalHook::AfterModelResponseCommit,
        );
        if retained_resumed.state().is_terminal() {
            self.draft_sink
                .abandon(attempt_id, DraftAbandonCause::Cancelled);
        }
        PhysicalAttemptResult::Completed {
            response: Box::new(response),
            attempt: DurableModelAttempt {
                model_invocation_id: attempt_id,
                logical_invocation_id: invocation.context.package().logical_invocation_id(),
                attempt_no,
                terminal_model_event_id: model_event,
                terminal_work_event_id: work_event,
                work: retained_resumed,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_provider_error(
        &self,
        invocation: &mut GatewayInvocation,
        attempt_id: ModelInvocationId,
        attempt_no: u32,
        model_state: ModelInvocationState,
        cause: JournalEventId,
        error: ProviderError,
        semantic_output_observed: bool,
        draft_exposed: bool,
        first_byte_at: Option<UtcTimestamp>,
        first_output_at: Option<UtcTimestamp>,
        provider_request_id: Option<String>,
        provider_response_id: Option<String>,
        usage: Option<CanonicalModelUsage>,
    ) -> PhysicalAttemptResult {
        let provider_request_id = provider_request_id.or_else(|| {
            error
                .provider_request_id()
                .map(|value| value.as_str().to_owned())
        });
        self.finish_failed_attempt(
            invocation,
            attempt_id,
            attempt_no,
            model_state,
            cause,
            error,
            semantic_output_observed,
            draft_exposed,
            first_byte_at,
            first_output_at,
            provider_request_id,
            provider_response_id,
            usage,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_failed_attempt(
        &self,
        invocation: &mut GatewayInvocation,
        attempt_id: ModelInvocationId,
        attempt_no: u32,
        model_state: ModelInvocationState,
        mut cause: JournalEventId,
        error: ProviderError,
        semantic_output_observed: bool,
        draft_exposed: bool,
        first_byte_at: Option<UtcTimestamp>,
        first_output_at: Option<UtcTimestamp>,
        provider_request_id: Option<String>,
        provider_response_id: Option<String>,
        usage: Option<CanonicalModelUsage>,
    ) -> PhysicalAttemptResult {
        if *invocation.cancellation.borrow() {
            cause = self
                .refresh_durable_cancellation(invocation, attempt_id)
                .await
                .unwrap_or(cause);
        }
        let ambiguous = error.certainty() == ProviderOutcomeCertainty::ProviderOutcomeUnknown
            || matches!(
                error.kind(),
                ProviderErrorKind::TransportAfterPossibleProcessing
                    | ProviderErrorKind::ProviderOutcomeUnknown
            );
        let completed_at = match self.wall_now() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let confirmed_cancellation = !ambiguous
            && error.kind() == ProviderErrorKind::Cancelled
            && error.certainty() == ProviderOutcomeCertainty::DefinitelyNotSent
            && invocation.work.state() == crate::domain::WorkState::CancelRequested;
        let terminal_transition = || {
            if ambiguous {
                decide_work_transition(
                    &invocation.work,
                    WorkTransitionGuard::for_snapshot(&invocation.work),
                    WorkTransitionRequest::Interrupt {
                        reason: WorkInterruptionReason::ProviderOutcomeUnknown,
                    },
                )
                .map_err(|_| ModelGatewayError::Lifecycle)
                .map(|value| value.into_next())
            } else if confirmed_cancellation {
                let reason = invocation
                    .work
                    .cancellation_reason()
                    .ok_or(ModelGatewayError::Lifecycle)?;
                decide_work_transition(
                    &invocation.work,
                    WorkTransitionGuard::for_snapshot(&invocation.work),
                    WorkTransitionRequest::Cancel {
                        reason,
                        cleanup_status: CleanupStatus::Confirmed,
                    },
                )
                .map_err(|_| ModelGatewayError::Lifecycle)
                .map(|value| value.into_next())
            } else {
                resume_from_model(&invocation.work, attempt_id)
            }
        };
        let work_next = match terminal_transition() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let retained_work_next = match terminal_transition() {
            Ok(value) => value,
            Err(error) => return PhysicalAttemptResult::Infrastructure(error),
        };
        let model_event = JournalEventId::generate();
        let work_event = JournalEventId::generate();
        let terminal_state = if ambiguous {
            ModelInvocationState::ProviderOutcomeUnknown
        } else if error.kind() == ProviderErrorKind::Cancelled
            && error.certainty() == ProviderOutcomeCertainty::DefinitelyNotSent
        {
            ModelInvocationState::CancelledLocally
        } else {
            ModelInvocationState::Failed
        };
        let certainty =
            if terminal_state == ModelInvocationState::Failed && semantic_output_observed {
                ProviderOutcomeCertainty::SemanticOutputObserved
            } else {
                error.certainty()
            };
        let normalized_error = if ambiguous {
            NormalizedError::provider(crate::domain::Certainty::OutcomeUnknown, None)
        } else {
            error.normalized()
        };
        let outcome = ModelTerminalOutcome {
            state: terminal_state,
            response_sha256: None,
            response_artifact_id: None,
            normalized_output: None,
            provider_request_id,
            provider_response_id,
            first_byte_at,
            first_output_at,
            completed_at,
            usage: usage.map(stored_usage),
            usage_status: if usage.is_some() {
                ModelUsageStatus::Reported
            } else {
                ModelUsageStatus::Unavailable
            },
            provider_error_kind: Some(error.kind()),
            provider_outcome_certainty: certainty,
            billing_ambiguity: ambiguous,
            stop_reason: None,
            tool_call_count: None,
            draft_exposed,
            normalized_error: Some(normalized_error),
        };
        if let Err(error) = self
            .state_store
            .finish_model_invocation(FinishModelInvocationRequest {
                expected_work: WorkExpectation::for_snapshot(&invocation.work),
                expected_model: ModelExpectation {
                    model_invocation_id: attempt_id,
                    state: model_state,
                },
                outcome,
                artifacts: Vec::new(),
                work_next,
                model_event: EventIntent {
                    event_id: model_event,
                    correlation_id: invocation.correlation_id,
                    causation_event_id: Some(cause),
                },
                work_event: EventIntent {
                    event_id: work_event,
                    correlation_id: invocation.correlation_id,
                    causation_event_id: Some(model_event),
                },
            })
            .await
        {
            return PhysicalAttemptResult::Infrastructure(error.into());
        }
        let attempt = DurableModelAttempt {
            model_invocation_id: attempt_id,
            logical_invocation_id: invocation.context.package().logical_invocation_id(),
            attempt_no,
            terminal_model_event_id: model_event,
            terminal_work_event_id: work_event,
            work: retained_work_next,
        };
        if ambiguous {
            PhysicalAttemptResult::Interrupted {
                error_kind: error.kind(),
                attempt,
            }
        } else {
            PhysicalAttemptResult::Failed {
                error,
                semantic_output_observed,
                draft_exposed,
                attempt,
            }
        }
    }

    /// Refreshes the exact Work guard after the scheduler has durably won cancellation.
    /// A failed refresh is deliberately non-authoritative: the subsequent guarded commit
    /// still rejects stale ownership or state, while simple test stores need not emulate it.
    async fn refresh_durable_cancellation(
        &self,
        invocation: &mut GatewayInvocation,
        attempt_id: ModelInvocationId,
    ) -> Option<JournalEventId> {
        let runtime_id = invocation.work.runtime_owner()?;
        let owned = self
            .state_store
            .load_owned_work(LoadOwnedWorkRequest {
                work_id: invocation.work.work_id(),
                runtime_id,
            })
            .await
            .ok()?;
        if owned.lifecycle.state() != crate::domain::WorkState::CancelRequested
            || owned.lifecycle.current_attempt() != CurrentWorkAttempt::Model(attempt_id)
        {
            return None;
        }
        invocation.work = owned.lifecycle;
        Some(owned.latest_work_event_id)
    }

    fn prepare_normalized_output(
        &self,
        invocation: &GatewayInvocation,
        attempt_id: ModelInvocationId,
        response: &ModelResponse,
        created_at: UtcTimestamp,
    ) -> Result<(NormalizedModelOutput, Vec<PreparedArtifact>), ModelGatewayError> {
        let mut normalized = Vec::with_capacity(response.output_items().len());
        let mut artifacts = Vec::new();
        let mut captured_opaque = Vec::new();
        for (index, item) in response.output_items().iter().enumerate() {
            match item {
                ModelOutputItem::Text { content_parts } => {
                    normalized.push(NormalizedModelOutputItem::Text {
                        text: join_parts(content_parts),
                    });
                }
                ModelOutputItem::ToolCall(call) => {
                    let arguments = call
                        .require_valid_arguments()
                        .map_err(|_| ModelGatewayError::InvalidInvocation)?;
                    normalized.push(NormalizedModelOutputItem::ToolCall {
                        call_id: call.call_id().as_str().to_owned(),
                        tool_name: call.name().clone(),
                        arguments_json: serde_json::to_string(arguments)
                            .map_err(|_| ModelGatewayError::InvalidInvocation)?,
                    });
                }
                ModelOutputItem::StructuredData { data } => {
                    normalized.push(NormalizedModelOutputItem::StructuredData {
                        canonical_json: serde_json::to_string(data)
                            .map_err(|_| ModelGatewayError::InvalidInvocation)?,
                    });
                }
                ModelOutputItem::Refusal { content_parts } => {
                    normalized.push(NormalizedModelOutputItem::Refusal {
                        text: join_parts(content_parts),
                    });
                }
                ModelOutputItem::ReasoningSummary { content_parts } => {
                    normalized.push(NormalizedModelOutputItem::ReasoningSummary {
                        text: join_parts(content_parts),
                    });
                }
                ModelOutputItem::ProviderOpaque(opaque) => {
                    let (item, artifact) =
                        self.capture_opaque(invocation, attempt_id, opaque, index, created_at)?;
                    captured_opaque.push((
                        opaque.provider_id().clone(),
                        opaque.type_label().to_owned(),
                        opaque.sha256(),
                    ));
                    normalized.push(item);
                    artifacts.push(artifact);
                }
                ModelOutputItem::UnknownProviderItem(_) => {
                    return Err(ModelGatewayError::InvalidInvocation);
                }
            }
        }
        if let Some(continuation) = response.provider_continuation()
            && !captured_opaque
                .iter()
                .any(|(provider_id, type_label, digest)| {
                    provider_id == continuation.provider_id()
                        && type_label == continuation.type_label()
                        && *digest == continuation.sha256()
                })
        {
            let (item, artifact) = self.capture_opaque(
                invocation,
                attempt_id,
                continuation,
                response.output_items().len(),
                created_at,
            )?;
            normalized.push(item);
            artifacts.push(artifact);
        }
        Ok((NormalizedModelOutput { items: normalized }, artifacts))
    }

    fn capture_opaque(
        &self,
        invocation: &GatewayInvocation,
        attempt_id: ModelInvocationId,
        opaque: &ProviderOpaqueEvidence,
        index: usize,
        created_at: UtcTimestamp,
    ) -> Result<(NormalizedModelOutputItem, PreparedArtifact), ModelGatewayError> {
        let artifact_id = ArtifactId::generate();
        let length = CanonicalByteCount::try_new(
            u64::try_from(opaque.opaque().len()).map_err(|_| ModelGatewayError::Artifact)?,
        )
        .map_err(|_| ModelGatewayError::Artifact)?;
        let mut capture = self
            .artifact_store
            .begin_capture(BeginArtifactCapture {
                artifact_id,
                hard_capture_limit: length,
            })
            .map_err(|_| ModelGatewayError::Artifact)?;
        capture
            .write_chunk(opaque.opaque().as_bytes())
            .map_err(|_| ModelGatewayError::Artifact)?;
        let finalized = capture
            .finalize()
            .map_err(|_| ModelGatewayError::Artifact)?;
        if finalized.sha256() != opaque.sha256()
            || finalized.captured_byte_count() != length
            || finalized.truncated()
        {
            return Err(ModelGatewayError::Artifact);
        }
        let metadata = ArtifactReference::new(ArtifactReferenceInput {
            artifact_id,
            craxii_id: invocation.craxii_id,
            producing_work_id: Some(invocation.work.work_id()),
            producer: ArtifactProducer::Model(attempt_id),
            storage_key: ArtifactStorageKey::from_digest(finalized.sha256()),
            sha256: finalized.sha256(),
            canonical_length: finalized.captured_byte_count(),
            observed_length: Some(finalized.observed_byte_count()),
            mime_type: ArtifactMimeType::try_new("application/octet-stream")
                .map_err(|_| ModelGatewayError::Artifact)?,
            encoding: Some(
                ArtifactEncoding::try_new("utf-8").map_err(|_| ModelGatewayError::Artifact)?,
            ),
            logical_name: Some(
                ArtifactLogicalName::try_new(format!("provider-opaque-{index:02}.bin"))
                    .map_err(|_| ModelGatewayError::Artifact)?,
            ),
            retention: ArtifactRetention::CanonicalEvidence,
            truncated: false,
            compression: None,
            created_at,
        });
        Ok((
            NormalizedModelOutputItem::ProviderOpaque {
                provider_id: opaque.provider_id().clone(),
                item_type: opaque.type_label().to_owned(),
                sha256: opaque.sha256(),
                artifact_id,
            },
            PreparedArtifact {
                finalized,
                metadata,
                event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id: invocation.correlation_id,
                    causation_event_id: None,
                },
            },
        ))
    }

    fn wall_now(&self) -> Result<UtcTimestamp, ModelGatewayError> {
        self.clock
            .utc_now()
            .map_err(|_| ModelGatewayError::Clock)
            .and_then(|value| {
                UtcTimestamp::from_offset_datetime(value).map_err(|_| ModelGatewayError::Clock)
            })
    }
}

enum PhysicalAttemptResult {
    Completed {
        response: Box<ModelResponse>,
        attempt: DurableModelAttempt,
    },
    Failed {
        error: ProviderError,
        semantic_output_observed: bool,
        draft_exposed: bool,
        attempt: DurableModelAttempt,
    },
    Interrupted {
        error_kind: ProviderErrorKind,
        attempt: DurableModelAttempt,
    },
    Infrastructure(ModelGatewayError),
}

struct ModelAttemptObservation {
    work_id: String,
    logical_invocation_id: String,
    model_invocation_id: String,
    attempt_ordinal: u32,
    target: String,
    provider: String,
    model: String,
    request_sha256: String,
    request_bytes: u64,
    retry_of_invocation_id: Option<String>,
    retry_reason: Option<&'static str>,
    retry_delay_ms: Option<u64>,
}

struct ProviderStreamObservation {
    span: tracing::Span,
    attempt_span: tracing::Span,
    started: Instant,
    result_class: &'static str,
    finished: bool,
    first_response_observed: bool,
    first_semantic_output_observed: bool,
}

impl ProviderStreamObservation {
    fn new(span: tracing::Span, attempt_span: tracing::Span) -> Self {
        Self {
            span,
            attempt_span,
            started: Instant::now(),
            result_class: "incomplete",
            finished: false,
            first_response_observed: false,
            first_semantic_output_observed: false,
        }
    }

    fn classify(&mut self, result_class: &'static str) {
        if self.finished {
            return;
        }
        self.result_class = result_class;
        self.finished = true;
        self.record_terminal();
    }

    fn observe_first_response(&mut self) {
        if self.first_response_observed {
            return;
        }
        self.first_response_observed = true;
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.span.record("first_response_latency_ms", elapsed);
        self.attempt_span
            .record("first_response_latency_ms", elapsed);
    }

    fn observe_first_semantic_output(&mut self) {
        if self.first_semantic_output_observed {
            return;
        }
        self.first_semantic_output_observed = true;
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.span
            .record("first_semantic_output_latency_ms", elapsed);
        self.attempt_span
            .record("first_semantic_output_latency_ms", elapsed);
    }

    fn record_terminal(&self) {
        self.span.record("result_class", self.result_class);
        self.span.record(
            "duration_ms",
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
    }
}

impl Drop for ProviderStreamObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.record_terminal();
        }
    }
}

fn observe_model_attempt(
    span: &tracing::Span,
    observation: &ModelAttemptObservation,
    total_latency_ms: u64,
    result: &PhysicalAttemptResult,
) {
    match result {
        PhysicalAttemptResult::Completed {
            response,
            attempt: _,
        } => {
            span.record("result_class", "completed");
            span.record("certainty", "definitely_completed");
            span.record(
                "output_item_count",
                u64::try_from(response.output_items().len()).unwrap_or(u64::MAX),
            );
            span.record(
                "tool_call_count",
                u64::try_from(
                    response
                        .output_items()
                        .iter()
                        .filter(|item| matches!(item, ModelOutputItem::ToolCall(_)))
                        .count(),
                )
                .unwrap_or(u64::MAX),
            );
            if let Some(usage) = response.usage() {
                span.record("input_tokens", usage.input_tokens());
                span.record("cached_input_tokens", usage.cached_input_tokens());
                span.record("output_tokens", usage.output_tokens());
                span.record("reasoning_tokens", usage.reasoning_tokens());
                span.record("total_tokens", usage.total_tokens());
            }
            span.record("stop_reason", response.stop_reason().as_str());
            if let Some(value) = response.provider_request_id() {
                span.record(
                    "provider_request_digest",
                    SafeProviderCorrelation::from_provider_id(value).as_str(),
                );
            }
            if let Some(value) = response.provider_response_id() {
                span.record(
                    "provider_response_digest",
                    SafeProviderCorrelation::from_provider_id(value).as_str(),
                );
            }
            let usage = response.usage();
            let provider_request_digest = response
                .provider_request_id()
                .map(SafeProviderCorrelation::from_provider_id);
            let provider_response_digest = response
                .provider_response_id()
                .map(SafeProviderCorrelation::from_provider_id);
            tracing::info!(
                event_name = "model_attempt_terminal",
                work_id = observation.work_id.as_str(),
                logical_invocation_id = observation.logical_invocation_id.as_str(),
                model_invocation_id = observation.model_invocation_id.as_str(),
                attempt_ordinal = observation.attempt_ordinal,
                target = observation.target.as_str(),
                provider = observation.provider.as_str(),
                model = observation.model.as_str(),
                request_sha256 = observation.request_sha256.as_str(),
                request_bytes = observation.request_bytes,
                retry_of_invocation_id = observation.retry_of_invocation_id.as_deref(),
                retry_reason = observation.retry_reason,
                retry_delay_ms = observation.retry_delay_ms,
                total_latency_ms,
                result_class = "completed",
                certainty = "definitely_completed",
                stop_reason = response.stop_reason().as_str(),
                output_item_count =
                    u64::try_from(response.output_items().len()).unwrap_or(u64::MAX),
                tool_call_count = u64::try_from(
                    response
                        .output_items()
                        .iter()
                        .filter(|item| matches!(item, ModelOutputItem::ToolCall(_)))
                        .count(),
                )
                .unwrap_or(u64::MAX),
                usage_status = if usage.is_some() {
                    "reported"
                } else {
                    "unavailable"
                },
                input_tokens = usage.map(|value| value.input_tokens()),
                cached_input_tokens = usage.map(|value| value.cached_input_tokens()),
                output_tokens = usage.map(|value| value.output_tokens()),
                reasoning_tokens = usage.map(|value| value.reasoning_tokens()),
                total_tokens = usage.map(|value| value.total_tokens()),
                provider_request_digest = provider_request_digest
                    .as_ref()
                    .map(SafeProviderCorrelation::as_str),
                provider_response_digest = provider_response_digest
                    .as_ref()
                    .map(SafeProviderCorrelation::as_str),
            );
        }
        PhysicalAttemptResult::Failed {
            error,
            semantic_output_observed,
            draft_exposed,
            attempt: _,
        } => {
            span.record(
                "result_class",
                if error.certainty() == ProviderOutcomeCertainty::ProviderOutcomeUnknown {
                    "outcome_unknown"
                } else {
                    "failed"
                },
            );
            span.record("certainty", error.certainty().as_str());
            span.record("provider_error_kind", error.kind().code());
            if let Some(status) = error.provider_http_status() {
                span.record("provider_http_status", status);
            }
            if let Some(value) = error.provider_request_id() {
                span.record(
                    "provider_request_digest",
                    SafeProviderCorrelation::from_provider_id(value).as_str(),
                );
            }
            span.record("draft_exposed", *draft_exposed);
            let provider_request_digest = error
                .provider_request_id()
                .map(SafeProviderCorrelation::from_provider_id);
            tracing::warn!(
                event_name = "model_attempt_terminal",
                work_id = observation.work_id.as_str(),
                logical_invocation_id = observation.logical_invocation_id.as_str(),
                model_invocation_id = observation.model_invocation_id.as_str(),
                attempt_ordinal = observation.attempt_ordinal,
                target = observation.target.as_str(),
                provider = observation.provider.as_str(),
                model = observation.model.as_str(),
                request_sha256 = observation.request_sha256.as_str(),
                request_bytes = observation.request_bytes,
                retry_of_invocation_id = observation.retry_of_invocation_id.as_deref(),
                retry_reason = observation.retry_reason,
                retry_delay_ms = observation.retry_delay_ms,
                total_latency_ms,
                result_class =
                    if error.certainty() == ProviderOutcomeCertainty::ProviderOutcomeUnknown {
                        "outcome_unknown"
                    } else {
                        "failed"
                    },
                provider_error_kind = error.kind().code(),
                provider_http_status = error.provider_http_status(),
                provider_request_digest = provider_request_digest
                    .as_ref()
                    .map(SafeProviderCorrelation::as_str),
                certainty = error.certainty().as_str(),
                semantic_output_observed = *semantic_output_observed,
                draft_exposed = *draft_exposed,
                usage_status = "not_observed",
            );
        }
        PhysicalAttemptResult::Interrupted {
            error_kind,
            attempt: _,
        } => {
            span.record("result_class", "interrupted");
            span.record("provider_error_kind", error_kind.code());
            tracing::warn!(
                event_name = "model_attempt_terminal",
                work_id = observation.work_id.as_str(),
                logical_invocation_id = observation.logical_invocation_id.as_str(),
                model_invocation_id = observation.model_invocation_id.as_str(),
                attempt_ordinal = observation.attempt_ordinal,
                target = observation.target.as_str(),
                provider = observation.provider.as_str(),
                model = observation.model.as_str(),
                request_sha256 = observation.request_sha256.as_str(),
                request_bytes = observation.request_bytes,
                retry_of_invocation_id = observation.retry_of_invocation_id.as_deref(),
                retry_reason = observation.retry_reason,
                retry_delay_ms = observation.retry_delay_ms,
                total_latency_ms,
                result_class = "interrupted",
                provider_error_kind = error_kind.code(),
                certainty = "outcome_unknown",
                usage_status = "not_observed",
            );
        }
        PhysicalAttemptResult::Infrastructure(error) => {
            span.record("result_class", "infrastructure_failure");
            tracing::warn!(
                event_name = "model_attempt_terminal",
                work_id = observation.work_id.as_str(),
                logical_invocation_id = observation.logical_invocation_id.as_str(),
                model_invocation_id = observation.model_invocation_id.as_str(),
                attempt_ordinal = observation.attempt_ordinal,
                target = observation.target.as_str(),
                provider = observation.provider.as_str(),
                model = observation.model.as_str(),
                request_sha256 = observation.request_sha256.as_str(),
                request_bytes = observation.request_bytes,
                retry_of_invocation_id = observation.retry_of_invocation_id.as_deref(),
                retry_reason = observation.retry_reason,
                retry_delay_ms = observation.retry_delay_ms,
                total_latency_ms,
                result_class = "infrastructure_failure",
                error_class = %error,
                certainty = "not_observed",
                usage_status = "not_observed",
            );
        }
    }
}

fn observe_model_retry_scheduled(
    observation: &ModelAttemptObservation,
    total_latency_ms: u64,
    provider_error_kind: ProviderErrorKind,
    certainty: ProviderOutcomeCertainty,
    retry_reason: &'static str,
    retry_delay: Duration,
) {
    tracing::info!(
        event_name = "model_attempt_retry_scheduled",
        work_id = observation.work_id.as_str(),
        logical_invocation_id = observation.logical_invocation_id.as_str(),
        model_invocation_id = observation.model_invocation_id.as_str(),
        attempt_ordinal = observation.attempt_ordinal,
        target = observation.target.as_str(),
        provider = observation.provider.as_str(),
        model = observation.model.as_str(),
        request_sha256 = observation.request_sha256.as_str(),
        request_bytes = observation.request_bytes,
        total_latency_ms,
        result_class = "retry_scheduled",
        provider_error_kind = provider_error_kind.code(),
        certainty = certainty.as_str(),
        retry_reason,
        retry_delay_ms = u64::try_from(retry_delay.as_millis()).unwrap_or(u64::MAX),
    );
}

enum StreamTerminal {
    Completed(Box<ModelResponse>),
    ProviderError(ModelStreamProviderErrorKind),
}

/// One bounded accumulator for canonical provider stream order and terminal validation.
struct CanonicalStreamAccumulator {
    events: Vec<ModelStreamEvent>,
    semantic_output_observed: bool,
    provider_request_id: Option<String>,
    provider_response_id: Option<String>,
    usage: Option<CanonicalModelUsage>,
}

impl CanonicalStreamAccumulator {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            semantic_output_observed: false,
            provider_request_id: None,
            provider_response_id: None,
            usage: None,
        }
    }

    fn observe(&mut self, event: ModelStreamEvent) -> Result<(), ModelContractErrorKind> {
        if self.events.len() >= MAX_STREAM_EVENTS {
            return Err(ModelContractErrorKind::NormalizedOutputTooLarge);
        }
        if let ModelStreamEvent::ResponseStarted {
            provider_request_id,
            provider_response_id,
            ..
        } = &event
        {
            self.provider_request_id = provider_request_id
                .as_ref()
                .map(|value| value.as_str().to_owned());
            self.provider_response_id = provider_response_id
                .as_ref()
                .map(|value| value.as_str().to_owned());
        }
        if let ModelStreamEvent::Usage(usage) = &event {
            self.usage = Some(*usage);
        }
        self.semantic_output_observed |= event.is_semantic_output();
        self.events.push(event);
        validate_model_stream(&self.events)
            .map(|_| ())
            .map_err(|error| error.kind())
    }

    fn finish(&self) -> Result<StreamTerminal, ModelContractErrorKind> {
        let state = validate_model_stream(&self.events).map_err(|error| error.kind())?;
        match (state, self.events.last()) {
            (ModelStreamState::Completed, Some(ModelStreamEvent::Completed(response))) => {
                response
                    .require_supported_semantics()
                    .map_err(|error| error.kind())?;
                Ok(StreamTerminal::Completed(response.clone()))
            }
            (_, Some(ModelStreamEvent::ProviderError { kind })) => {
                Ok(StreamTerminal::ProviderError(*kind))
            }
            _ => Err(ModelContractErrorKind::InvalidStreamOrdering),
        }
    }

    const fn semantic_output_observed(&self) -> bool {
        self.semantic_output_observed
    }

    fn provider_request_id(&self) -> Option<String> {
        self.provider_request_id.clone()
    }

    fn provider_response_id(&self) -> Option<String> {
        self.provider_response_id.clone()
    }

    const fn usage(&self) -> Option<CanonicalModelUsage> {
        self.usage
    }
}

fn stored_selection_reason(value: ModelSelectionReason) -> StoredSelectionReason {
    match value {
        ModelSelectionReason::Explicit => StoredSelectionReason::Explicit,
        ModelSelectionReason::ConfiguredDefault => StoredSelectionReason::ConfiguredDefault,
    }
}

fn stored_required_capabilities(
    value: RequiredModelCapabilities,
) -> StoredRequiredModelCapabilities {
    StoredRequiredModelCapabilities {
        text_input: value.text_input,
        text_output: value.text_output,
        custom_tool_calling: value.custom_tool_calling,
        streaming: value.streaming,
        ordered_output_items: value.ordered_output_items,
        structured_output: value.structured_output,
        reasoning_continuation: value.reasoning_continuation,
    }
}

const fn stored_usage(value: CanonicalModelUsage) -> StoredModelUsage {
    StoredModelUsage {
        input_tokens: value.input_tokens(),
        cached_input_tokens: value.cached_input_tokens(),
        output_tokens: value.output_tokens(),
        reasoning_tokens: value.reasoning_tokens(),
        total_tokens: value.total_tokens(),
    }
}

fn join_parts(parts: &[crate::domain::ModelTextPart]) -> String {
    parts
        .iter()
        .map(crate::domain::ModelTextPart::as_str)
        .collect()
}

fn resume_from_model(
    work: &WorkLifecycleSnapshot,
    attempt_id: ModelInvocationId,
) -> Result<WorkLifecycleSnapshot, ModelGatewayError> {
    decide_work_transition(
        work,
        WorkTransitionGuard::for_snapshot(work),
        WorkTransitionRequest::ResumeFromModel {
            model_invocation_id: attempt_id,
        },
    )
    .map(|value| value.into_next())
    .map_err(|_| ModelGatewayError::Lifecycle)
}

fn started_ids(event: &ModelStreamEvent) -> (Option<String>, Option<String>) {
    match event {
        ModelStreamEvent::ResponseStarted {
            provider_request_id,
            provider_response_id,
            ..
        } => (
            provider_request_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
            provider_response_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        ),
        _ => (None, None),
    }
}

fn draft_delta(event: &ModelStreamEvent) -> Option<CanonicalDraftDelta> {
    match event {
        ModelStreamEvent::TextDelta { delta, .. } => Some(CanonicalDraftDelta::Text {
            text: delta.as_str().to_owned(),
        }),
        ModelStreamEvent::RefusalDelta { delta, .. } => Some(CanonicalDraftDelta::Refusal {
            text: delta.as_str().to_owned(),
        }),
        _ => None,
    }
}

fn contract_error_kind(kind: ModelContractErrorKind) -> ProviderErrorKind {
    match kind {
        ModelContractErrorKind::InvalidToolArguments
        | ModelContractErrorKind::ToolArgumentsTooLarge => {
            ProviderErrorKind::MalformedCompletedToolArguments
        }
        ModelContractErrorKind::TooManyOutputItems
        | ModelContractErrorKind::NormalizedOutputTooLarge => ProviderErrorKind::OutputTooLarge,
        ModelContractErrorKind::UnknownSemanticItem => ProviderErrorKind::UnsupportedResponseItem,
        _ => ProviderErrorKind::MalformedResponse,
    }
}

fn stream_error(kind: ModelStreamProviderErrorKind, semantic: bool) -> ProviderError {
    let (kind, certainty) = match kind {
        ModelStreamProviderErrorKind::DefiniteFailure => (
            ProviderErrorKind::InternalProviderError,
            ProviderOutcomeCertainty::DefiniteProviderFailure,
        ),
        ModelStreamProviderErrorKind::TransientUnavailable => (
            ProviderErrorKind::TemporarilyUnavailable,
            ProviderOutcomeCertainty::DefiniteProviderFailure,
        ),
        ModelStreamProviderErrorKind::Cancelled => (
            ProviderErrorKind::Cancelled,
            ProviderOutcomeCertainty::ProviderOutcomeUnknown,
        ),
        ModelStreamProviderErrorKind::OutcomeUnknown => (
            ProviderErrorKind::ProviderOutcomeUnknown,
            ProviderOutcomeCertainty::ProviderOutcomeUnknown,
        ),
        ModelStreamProviderErrorKind::TimeoutBeforeOutput => (
            ProviderErrorKind::TimeoutBeforeOutput,
            ProviderOutcomeCertainty::ProviderOutcomeUnknown,
        ),
        ModelStreamProviderErrorKind::TimeoutAfterOutput => (
            ProviderErrorKind::TimeoutAfterOutput,
            ProviderOutcomeCertainty::SemanticOutputObserved,
        ),
        ModelStreamProviderErrorKind::ProtocolFailure => (
            ProviderErrorKind::MalformedResponse,
            if semantic {
                ProviderOutcomeCertainty::SemanticOutputObserved
            } else {
                ProviderOutcomeCertainty::ProviderOutcomeUnknown
            },
        ),
    };
    ProviderError::new(kind, certainty)
}

fn timeout_error(semantic: bool) -> ProviderError {
    if semantic {
        ProviderError::new(
            ProviderErrorKind::TimeoutAfterOutput,
            ProviderOutcomeCertainty::SemanticOutputObserved,
        )
    } else {
        ProviderError::new(
            ProviderErrorKind::TimeoutBeforeOutput,
            ProviderOutcomeCertainty::ProviderOutcomeUnknown,
        )
    }
}

fn remaining_duration(deadline: MonotonicInstant, clock: &dyn Clock) -> Duration {
    deadline
        .checked_duration_since(clock.monotonic_now())
        .unwrap_or(Duration::ZERO)
}

async fn cancellation_requested(receiver: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn wait_for_delay_or_cancellation(
    delay: Duration,
    cancellation: &mut tokio::sync::watch::Receiver<bool>,
    work_deadline: MonotonicInstant,
    clock: &dyn Clock,
) -> bool {
    let remaining = remaining_duration(work_deadline, clock);
    let delay = delay.min(remaining);
    tokio::select! {
        biased;
        () = cancellation_requested(cancellation) => true,
        () = tokio::time::sleep(delay) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::adapters::artifacts::LocalArtifactStore;
    use crate::adapters::scripted_provider::{
        ScriptExpectation, ScriptGate, ScriptedProgram, ScriptedProvider, ScriptedStep,
    };
    use crate::application::context_assembler::ContextAssemblyResult;
    use crate::application::event_delivery::{LiveEventBroker, LiveEventReceive};
    use crate::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
    use crate::domain::{
        ContextManifestId, ConversationId, JournalOffset, LogicalInvocationId,
        ModelCapabilitySnapshot, ModelCapabilitySnapshotInput, ModelConfigReference,
        ModelInputItem, ModelInputRole, ModelRequest, ModelRequestInput, ModelResponseInput,
        ModelStopReason, ModelTarget, ModelTargetId, ModelTargetInput, ModelTextPart,
        ModelToolChoicePolicy, ProjectionVersion, ProviderEvidenceId, ProviderId, ProviderMetadata,
        ProviderModelId, ProviderModelReference, ProviderNativeOptions, ProviderOpaqueEvidence,
        Sha256Digest, TargetConfigurationVersion, TokenCount, TokenEstimatorIdentity, WorkId,
        WorkLifecycleSnapshotInput, WorkState,
    };
    use crate::ports::artifact_store::{
        ArtifactObjectReference, ArtifactOrphanReport, ArtifactStoreError, ArtifactStoreErrorKind,
    };
    use crate::ports::clock::TestClock;
    use crate::ports::model_provider::{ModelProviderFuture, ModelProviderStream};
    use crate::ports::state_store::{
        CommitReceipt, CommittedEventRange, LoadOwnedWorkRequest, OwnedWorkState, StateStoreError,
        StateStoreErrorKind, StateStoreFuture, TerminalizeOwnedWorkRequest,
    };

    struct MinimumJitter;

    impl FullJitterSource for MinimumJitter {
        fn sample_inclusive(&mut self, _: u64) -> u64 {
            0
        }
    }

    struct PendingInvocationProvider {
        provider_id: ProviderId,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl PendingInvocationProvider {
        fn new(provider_id: ProviderId) -> Self {
            Self {
                provider_id,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    impl ModelProvider for PendingInvocationProvider {
        fn provider_id(&self) -> &ProviderId {
            &self.provider_id
        }

        fn capabilities(
            &self,
            target: &ModelTarget,
        ) -> Result<ModelCapabilitySnapshot, ProviderError> {
            if target.reference().provider_id() != &self.provider_id {
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    ProviderOutcomeCertainty::DefinitelyNotSent,
                ));
            }
            Ok(target.reference().capabilities().clone())
        }

        fn invoke_stream(
            &self,
            _: ModelProviderInvocation,
        ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Box::pin(std::future::pending())
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct AttemptEvidence {
        attempt_no: i64,
        request_sha256: Sha256Digest,
        retry: Option<ProviderRetryEvidence>,
        terminal_state: Option<ModelInvocationState>,
        usage_status: Option<ModelUsageStatus>,
        draft_exposed: Option<bool>,
    }

    #[derive(Default)]
    struct FakeGatewayStore {
        attempts: Mutex<Vec<AttemptEvidence>>,
    }

    impl FakeGatewayStore {
        fn attempts(&self) -> Vec<AttemptEvidence> {
            self.attempts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn receipt() -> CommitReceipt {
            CommitReceipt {
                committed_version: None,
                events: Some(CommittedEventRange {
                    first: JournalOffset::try_new(1).unwrap(),
                    last: JournalOffset::try_new(1).unwrap(),
                }),
            }
        }
    }

    impl ModelStateStore for FakeGatewayStore {
        fn load_owned_work(&self, _: LoadOwnedWorkRequest) -> StateStoreFuture<'_, OwnedWorkState> {
            Box::pin(async { Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant)) })
        }

        fn begin_model_invocation(
            &self,
            request: BeginModelInvocationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                self.attempts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(AttemptEvidence {
                        attempt_no: request.invocation.attempt.attempt_no().get(),
                        request_sha256: request.invocation.request_sha256,
                        retry: request.invocation.retry_evidence,
                        terminal_state: None,
                        usage_status: None,
                        draft_exposed: None,
                    });
                Ok(Self::receipt())
            })
        }

        fn mark_model_streaming(
            &self,
            _: MarkModelStreamingRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async { Ok(Self::receipt()) })
        }

        fn finish_model_invocation(
            &self,
            request: FinishModelInvocationRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                let mut attempts = self
                    .attempts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let attempt = attempts
                    .last_mut()
                    .ok_or_else(|| StateStoreError::new(StateStoreErrorKind::InternalInvariant))?;
                attempt.terminal_state = Some(request.outcome.state);
                attempt.usage_status = Some(request.outcome.usage_status);
                attempt.draft_exposed = Some(request.outcome.draft_exposed);
                Ok(Self::receipt())
            })
        }

        fn terminalize_owned_work(
            &self,
            _: TerminalizeOwnedWorkRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async { Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant)) })
        }
    }

    struct RejectingArtifactStore;

    impl ArtifactStore for RejectingArtifactStore {
        fn begin_capture(
            &self,
            _: BeginArtifactCapture,
        ) -> Result<Box<dyn crate::ports::artifact_store::ArtifactCapture>, ArtifactStoreError>
        {
            Err(ArtifactStoreError::new(ArtifactStoreErrorKind::Storage))
        }

        fn verify(&self, _: &ArtifactObjectReference) -> Result<(), ArtifactStoreError> {
            Err(ArtifactStoreError::new(ArtifactStoreErrorKind::Storage))
        }

        fn read_verified(
            &self,
            _: &ArtifactObjectReference,
        ) -> Result<Vec<u8>, ArtifactStoreError> {
            Err(ArtifactStoreError::new(ArtifactStoreErrorKind::Storage))
        }

        fn scan_orphans(
            &self,
            _: &BTreeSet<ArtifactStorageKey>,
            _: UtcTimestamp,
        ) -> Result<ArtifactOrphanReport, ArtifactStoreError> {
            Ok(ArtifactOrphanReport {
                referenced_final_count: 0,
                orphans: Vec::new(),
            })
        }
    }

    struct GatewayFixture {
        target: ModelTarget,
        selection: ModelSelectionResult,
        context: ContextAssemblyResult,
        work: WorkLifecycleSnapshot,
        craxii_id: CraxiiId,
        conversation_id: ConversationId,
        correlation_id: CorrelationId,
        cause: JournalEventId,
        clock: Arc<TestClock>,
    }

    fn gateway_fixture() -> GatewayFixture {
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            context_window_tokens: TokenCount::try_new(128_000).unwrap(),
            max_output_tokens: TokenCount::try_new(4_096).unwrap(),
        });
        let target = ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new("fixture-primary").unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new("fixture-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("fixture-account").unwrap(),
            requested_output_tokens: TokenCount::try_new(512).unwrap(),
            estimator: TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
            provider_native_options: ProviderNativeOptions::new(true),
        })
        .unwrap();
        let snapshot = Arc::new(
            ModelTargetSnapshot::try_new(
                target.reference().model_target_id().clone(),
                vec![target.clone()],
            )
            .unwrap(),
        );
        let required = RequiredModelCapabilities {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: false,
            required_output_tokens: TokenCount::try_new(512).unwrap(),
        };
        let selection = ModelSelectionPolicy::new(snapshot)
            .select(None, required)
            .unwrap();
        let work_id = WorkId::generate();
        let conversation_id = ConversationId::generate();
        let runtime_id = crate::domain::RuntimeInstanceId::generate();
        let logical_id = LogicalInvocationId::generate();
        let manifest_id = ContextManifestId::generate();
        let request = ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: logical_id,
            target: target.clone(),
            ordered_input_items: vec![
                ModelInputItem::message(
                    ModelInputRole::User,
                    vec![ModelTextPart::try_new("hello").unwrap()],
                )
                .unwrap(),
            ],
            instructions: vec![ModelTextPart::try_new("answer safely").unwrap()],
            tool_definitions: Vec::new(),
            requested_output_limit: TokenCount::try_new(512).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::Automatic,
            provider_native_options: ProviderNativeOptions::new(true),
            context_manifest_id: manifest_id,
        })
        .unwrap();
        let manifest = crate::ports::state_store::PreparedContextManifest {
            context_manifest_id: manifest_id,
            work_id,
            logical_invocation_id: logical_id,
            provider_model: target.reference().clone(),
            assembler_version: "test-v1".to_owned(),
            context_policy_version: "test-v1".to_owned(),
            system_prompt_fingerprint: Sha256Digest::hash_bytes(b"prompt"),
            toolset_fingerprint: crate::domain::model_toolset_fingerprint(&[]),
            eligibility_conversation_id: conversation_id,
            active_work_ordinal: 1,
            highest_prior_terminal_work_ordinal: None,
            input_event_ids: Vec::new(),
            active_output_record_ids: Vec::new(),
            maximum_journal_offset: JournalOffset::try_new(1).unwrap(),
            canonical_byte_count: CanonicalByteCount::try_new(1).unwrap(),
            rendered_request_byte_count: CanonicalByteCount::try_new(
                u64::try_from(request.canonical_bytes().len()).unwrap(),
            )
            .unwrap(),
            estimated_input_tokens: 1,
            token_estimator_id: "fixture_v1@1".to_owned(),
            context_window_tokens: 128_000,
            reserved_output_tokens: 512,
            utilization_basis_points: 1,
            manifest_sha256: Sha256Digest::hash_bytes(b"manifest"),
            rendered_request_sha256: request.canonical_sha256(),
            rendered_request_artifact_id: None,
            omitted_source_count: 0,
            transformed_source_count: 0,
            sources: Vec::new(),
            created_at: "2026-09-01T00:00:00.000000Z".parse().unwrap(),
        };
        let context = crate::application::context_assembler::test_support::from_exact_parts(
            selection.clone(),
            request,
            manifest,
        )
        .unwrap();
        let work = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id,
            state: WorkState::Running,
            projection_version: ProjectionVersion::try_new(2).unwrap(),
            runtime_owner: Some(runtime_id),
            current_attempt: crate::domain::CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: None,
        })
        .unwrap();
        GatewayFixture {
            target,
            selection,
            context,
            work,
            craxii_id: CraxiiId::generate(),
            conversation_id,
            correlation_id: CorrelationId::generate(),
            cause: JournalEventId::generate(),
            clock: Arc::new(TestClock::new(
                time::OffsetDateTime::from_unix_timestamp(1_788_220_800).unwrap(),
                Duration::ZERO,
            )),
        }
    }

    fn usage() -> CanonicalModelUsage {
        CanonicalModelUsage::try_new(10, 2, 5, 1, 15).unwrap()
    }

    fn final_response(target: &ModelTarget, text: &str) -> ModelResponse {
        ModelResponse::try_new(ModelResponseInput {
            selected_target: target.identity(),
            output_items: vec![
                ModelOutputItem::text(vec![ModelTextPart::try_new(text).unwrap()]).unwrap(),
            ],
            stop_reason: ModelStopReason::Completed,
            usage: Some(usage()),
            provider_request_id: Some(ProviderEvidenceId::try_new("request-1").unwrap()),
            provider_response_id: Some(ProviderEvidenceId::try_new("response-1").unwrap()),
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        })
        .unwrap()
    }

    fn success_steps(target: &ModelTarget, text: &str) -> Vec<ScriptedStep> {
        vec![
            ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                target: target.identity(),
                provider_request_id: Some(ProviderEvidenceId::try_new("request-1").unwrap()),
                provider_response_id: Some(ProviderEvidenceId::try_new("response-1").unwrap()),
            }),
            ScriptedStep::emit(ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: ModelTextPart::try_new(text).unwrap(),
            }),
            ScriptedStep::emit(ModelStreamEvent::Usage(usage())),
            ScriptedStep::emit(ModelStreamEvent::Completed(Box::new(final_response(
                target, text,
            )))),
        ]
    }

    fn program(
        fixture: &GatewayFixture,
        ordinal: u64,
        attempt: u32,
        steps: Vec<ScriptedStep>,
    ) -> ScriptedProgram {
        ScriptedProgram {
            expectation: ScriptExpectation {
                target_id: fixture.target.reference().model_target_id().clone(),
                request_sha256: Some(fixture.context.request().canonical_sha256()),
                fixture_key: None,
                required_prior_tool_result: None,
                invocation_ordinal: ordinal,
                attempt: ProviderAttempt::try_new(attempt).unwrap(),
            },
            steps,
        }
    }

    fn invocation(
        fixture: GatewayFixture,
        cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> GatewayInvocation {
        let deadline = fixture
            .clock
            .monotonic_now()
            .checked_add(Duration::from_secs(30 * 60))
            .unwrap();
        GatewayInvocation {
            craxii_id: fixture.craxii_id,
            conversation_id: fixture.conversation_id,
            work: fixture.work,
            context: fixture.context,
            selection: fixture.selection,
            agent_step: AgentStepNo::try_new(1).unwrap(),
            correlation_id: fixture.correlation_id,
            causation_event_id: fixture.cause,
            cancellation,
            work_deadline: deadline,
            shutdown_deadline: None,
            work_attempts_before_invocation: 0,
        }
    }

    fn build_gateway(
        store: Arc<FakeGatewayStore>,
        provider: Arc<dyn ModelProvider>,
        clock: Arc<TestClock>,
    ) -> ModelGateway {
        ModelGateway::new(
            store,
            Arc::new(RejectingArtifactStore),
            provider,
            Arc::new(NoopDraftSink),
            clock,
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap()
    }

    struct TemporaryArtifactRoot(std::path::PathBuf);

    impl TemporaryArtifactRoot {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("craxii-stage19-gateway-{}", ArtifactId::generate())),
            )
        }
    }

    impl Drop for TemporaryArtifactRoot {
        fn drop(&mut self) {
            if self.0.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("craxii-stage19-gateway-")
            }) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn frozen_limits_are_exact_and_noop_drafts_are_never_exposed() {
        let limits = ModelGatewayLimits::default().validate().unwrap();
        assert_eq!(limits.maximum_attempts_per_logical_invocation, 3);
        assert_eq!(limits.maximum_attempts_per_work, 32);
        let _jitter: Box<dyn FullJitterSource + Send> = Box::new(MinimumJitter);
    }

    #[test]
    fn stage20_safe_projection_is_text_and_refusal_only() {
        let text = ModelTextPart::try_new("safe text").unwrap();
        assert_eq!(
            draft_delta(&ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: text.clone(),
            }),
            Some(CanonicalDraftDelta::Text {
                text: "safe text".to_owned(),
            })
        );
        assert_eq!(
            draft_delta(&ModelStreamEvent::RefusalDelta {
                item_ordinal: 0,
                delta: text.clone(),
            }),
            Some(CanonicalDraftDelta::Refusal {
                text: "safe text".to_owned(),
            })
        );
        let call = crate::domain::CanonicalModelToolCall::try_new(
            crate::domain::ModelToolCallId::try_new("private-call").unwrap(),
            "run_shell",
            r#"{"command":"private"}"#,
        )
        .unwrap();
        let opaque = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "private.opaque",
            "private-provider-state",
        )
        .unwrap();
        for event in [
            ModelStreamEvent::ReasoningSummaryDelta {
                item_ordinal: 0,
                delta: text,
            },
            ModelStreamEvent::ToolCallStarted {
                item_ordinal: 1,
                call_id: call.call_id().clone(),
                name: call.name().clone(),
            },
            ModelStreamEvent::ToolArgumentDelta {
                item_ordinal: 1,
                call_id: call.call_id().clone(),
                delta: call.raw_arguments().to_owned(),
            },
            ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 1,
                call,
            },
            ModelStreamEvent::RefusalCompleted { item_ordinal: 2 },
            ModelStreamEvent::StructuredData {
                item_ordinal: 3,
                data: serde_json::json!({"private": true}),
            },
            ModelStreamEvent::UnknownProviderEvent(opaque),
        ] {
            assert!(draft_delta(&event).is_none());
        }
    }

    #[test]
    fn response_level_provider_continuation_is_captured_as_durable_opaque_evidence() {
        let fixture = gateway_fixture();
        let target = fixture.target.clone();
        let clock = fixture.clock.clone();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            Vec::new(),
            clock.clone(),
        ));
        let root = TemporaryArtifactRoot::new();
        let artifact_store = Arc::new(LocalArtifactStore::initialize(&root.0).unwrap());
        let gateway = ModelGateway::new(
            Arc::new(FakeGatewayStore::default()),
            artifact_store.clone(),
            provider,
            Arc::new(NoopDraftSink),
            clock,
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap();
        let continuation = ProviderOpaqueEvidence::try_new(
            ProviderId::try_new("fixture").unwrap(),
            "openai.reasoning.encrypted_content",
            "bounded-encrypted-fixture",
        )
        .unwrap();
        let response = ModelResponse::try_new(ModelResponseInput {
            selected_target: target.identity(),
            output_items: vec![
                ModelOutputItem::text(vec![ModelTextPart::try_new("done").unwrap()]).unwrap(),
            ],
            stop_reason: ModelStopReason::Completed,
            usage: Some(usage()),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: Some(continuation.clone()),
            provider_metadata: ProviderMetadata::default(),
        })
        .unwrap();
        let (_, receiver) = tokio::sync::watch::channel(false);
        let gateway_invocation = invocation(fixture, receiver);

        let (normalized, artifacts) = gateway
            .prepare_normalized_output(
                &gateway_invocation,
                ModelInvocationId::generate(),
                &response,
                "2026-09-01T00:00:01.000000Z".parse().unwrap(),
            )
            .unwrap();

        assert_eq!(normalized.items.len(), 2);
        assert!(matches!(
            &normalized.items[1],
            NormalizedModelOutputItem::ProviderOpaque {
                provider_id,
                item_type,
                sha256,
                ..
            } if provider_id == continuation.provider_id()
                && item_type == continuation.type_label()
                && *sha256 == continuation.sha256()
        ));
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifact_store
                .read_verified(artifacts[0].finalized.object_reference())
                .unwrap(),
            continuation.opaque().as_bytes()
        );
    }

    #[tokio::test]
    async fn final_answer_is_one_durable_physical_attempt_with_exact_request_hash() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                success_steps(&fixture.target, "done"),
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let expected_hash = fixture.context.request().canonical_sha256();
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(result, DurableModelOutcome::Completed { .. }));
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(store.attempts().len(), 1);
        assert_eq!(store.attempts()[0].request_sha256, expected_hash);
        assert_eq!(
            store.attempts()[0].terminal_state,
            Some(ModelInvocationState::Completed)
        );
    }

    #[tokio::test]
    async fn transient_failure_retries_same_request_and_persists_retry_evidence() {
        let fixture = gateway_fixture();
        let transient = ProviderError::new(
            ProviderErrorKind::RateLimited,
            ProviderOutcomeCertainty::DefiniteProviderFailure,
        );
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![
                program(&fixture, 1, 1, vec![ScriptedStep::Fail(transient)]),
                program(&fixture, 2, 2, success_steps(&fixture.target, "retried")),
            ],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(result, DurableModelOutcome::Completed { .. }));
        let attempts = store.attempts();
        assert_eq!(provider.invocation_count(), 2);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].request_sha256, attempts[1].request_sha256);
        assert!(attempts[0].retry.is_none());
        assert_eq!(
            attempts[1].retry.unwrap().reason,
            crate::ports::model_provider::RetryReasonCode::ClassifiedTransientBeforeOutput
        );
    }

    #[tokio::test]
    async fn retry_exhaustion_is_exactly_three_attempts_and_permanent_failure_is_one() {
        let fixture = gateway_fixture();
        let retryable = || {
            ProviderError::new(
                ProviderErrorKind::TemporarilyUnavailable,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
            )
        };
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![
                program(&fixture, 1, 1, vec![ScriptedStep::Fail(retryable())]),
                program(&fixture, 2, 2, vec![ScriptedStep::Fail(retryable())]),
                program(&fixture, 3, 3, vec![ScriptedStep::Fail(retryable())]),
            ],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(
            result,
            DurableModelOutcome::Failed {
                retries_exhausted: true,
                ..
            }
        ));
        assert_eq!(provider.invocation_count(), 3);
        assert_eq!(store.attempts().len(), 3);

        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    ProviderOutcomeCertainty::DefiniteProviderFailure,
                ))],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store, provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(
            result,
            DurableModelOutcome::Failed {
                error_kind: ProviderErrorKind::Authentication,
                retries_exhausted: false,
                ..
            }
        ));
        assert_eq!(provider.invocation_count(), 1);
    }

    #[tokio::test]
    async fn semantic_output_and_provider_ambiguity_each_disable_retry() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![
                    ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                        target: fixture.target.identity(),
                        provider_request_id: None,
                        provider_response_id: None,
                    }),
                    ScriptedStep::emit(ModelStreamEvent::TextDelta {
                        item_ordinal: 0,
                        delta: ModelTextPart::try_new("partial").unwrap(),
                    }),
                    ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TimeoutAfterOutput,
                        ProviderOutcomeCertainty::SemanticOutputObserved,
                    )),
                ],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store, provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(
            result,
            DurableModelOutcome::Failed {
                semantic_output_observed: true,
                ..
            }
        ));
        assert_eq!(provider.invocation_count(), 1);

        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![ScriptedStep::Fail(ProviderError::new(
                    ProviderErrorKind::TransportAfterPossibleProcessing,
                    ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                ))],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store, provider.clone(), fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(result, DurableModelOutcome::Interrupted { .. }));
        assert_eq!(provider.invocation_count(), 1);
    }

    #[tokio::test]
    async fn stage20_real_live_sink_receives_gateway_text_and_persists_conservative_exposure() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                success_steps(&fixture.target, "visible draft"),
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let broker = Arc::new(LiveEventBroker::new());
        let mut subscriber = broker.subscribe().unwrap();
        let gateway = ModelGateway::new(
            store.clone(),
            Arc::new(RejectingArtifactStore),
            provider,
            broker as Arc<dyn DraftSink>,
            fixture.clock.clone(),
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap();
        let (_, receiver) = tokio::sync::watch::channel(false);
        assert!(matches!(
            gateway.invoke(invocation(fixture, receiver)).await.unwrap(),
            DurableModelOutcome::Completed { .. }
        ));
        let LiveEventReceive::Event(started) = subscriber.recv().await else {
            panic!("draft start expected");
        };
        let LiveEventReceive::Event(delta) = subscriber.recv().await else {
            panic!("draft delta expected");
        };
        assert_eq!(started.event_type, "assistant.draft_started");
        assert_eq!(delta.event_type, "assistant.draft_delta");
        assert_eq!(delta.delta_sequence, Some(1));
        assert_eq!(store.attempts()[0].draft_exposed, Some(true));
    }

    #[tokio::test]
    async fn stage20_pre_output_retry_has_no_stale_draft_before_successful_attempt() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![
                program(
                    &fixture,
                    1,
                    1,
                    vec![ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TemporarilyUnavailable,
                        ProviderOutcomeCertainty::DefiniteProviderFailure,
                    ))],
                ),
                program(
                    &fixture,
                    2,
                    2,
                    success_steps(&fixture.target, "retry answer"),
                ),
            ],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let broker = Arc::new(LiveEventBroker::new());
        let mut subscriber = broker.subscribe().unwrap();
        let gateway = ModelGateway::new(
            store.clone(),
            Arc::new(RejectingArtifactStore),
            provider.clone(),
            broker.clone() as Arc<dyn DraftSink>,
            fixture.clock.clone(),
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap();
        let (_, receiver) = tokio::sync::watch::channel(false);
        assert!(matches!(
            gateway.invoke(invocation(fixture, receiver)).await.unwrap(),
            DurableModelOutcome::Completed { .. }
        ));
        assert_eq!(provider.invocation_count(), 2);
        let LiveEventReceive::Event(started) = subscriber.recv().await else {
            panic!("single successful draft start expected");
        };
        let LiveEventReceive::Event(delta) = subscriber.recv().await else {
            panic!("single successful draft delta expected");
        };
        assert_eq!(started.event_type, "assistant.draft_started");
        assert_eq!(delta.event_type, "assistant.draft_delta");
        assert_eq!(broker.metrics().drafts_started, 1);
        assert_eq!(broker.metrics().drafts_abandoned, 0);
        assert_eq!(store.attempts()[0].draft_exposed, Some(false));
        assert_eq!(store.attempts()[1].draft_exposed, Some(true));
    }

    #[tokio::test]
    async fn stage20_definite_failure_after_live_output_abandons_without_retry() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![
                    ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                        target: fixture.target.identity(),
                        provider_request_id: None,
                        provider_response_id: None,
                    }),
                    ScriptedStep::emit(ModelStreamEvent::TextDelta {
                        item_ordinal: 0,
                        delta: ModelTextPart::try_new("partial").unwrap(),
                    }),
                    ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TemporarilyUnavailable,
                        ProviderOutcomeCertainty::DefiniteProviderFailure,
                    )),
                ],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let broker = Arc::new(LiveEventBroker::new());
        let mut subscriber = broker.subscribe().unwrap();
        let gateway = ModelGateway::new(
            store.clone(),
            Arc::new(RejectingArtifactStore),
            provider.clone(),
            broker as Arc<dyn DraftSink>,
            fixture.clock.clone(),
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap();
        let (_, receiver) = tokio::sync::watch::channel(false);
        assert!(matches!(
            gateway.invoke(invocation(fixture, receiver)).await.unwrap(),
            DurableModelOutcome::Failed {
                semantic_output_observed: true,
                ..
            }
        ));
        assert_eq!(provider.invocation_count(), 1);
        for expected in [
            "assistant.draft_started",
            "assistant.draft_delta",
            "assistant.draft_abandoned",
        ] {
            let LiveEventReceive::Event(event) = subscriber.recv().await else {
                panic!("live event expected");
            };
            assert_eq!(event.event_type, expected);
            if expected == "assistant.draft_abandoned" {
                assert!(matches!(
                    event.payload,
                    crate::protocol::DraftEventPayload::Abandoned {
                        reason: crate::protocol::DraftAbandonReason::Failed
                    }
                ));
            }
        }
        assert_eq!(store.attempts()[0].draft_exposed, Some(true));
    }

    #[tokio::test]
    async fn stage20_provider_ambiguity_after_live_output_abandons_as_interrupted() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![
                    ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                        target: fixture.target.identity(),
                        provider_request_id: None,
                        provider_response_id: None,
                    }),
                    ScriptedStep::emit(ModelStreamEvent::TextDelta {
                        item_ordinal: 0,
                        delta: ModelTextPart::try_new("ambiguous partial").unwrap(),
                    }),
                    ScriptedStep::Fail(ProviderError::new(
                        ProviderErrorKind::TransportAfterPossibleProcessing,
                        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
                    )),
                ],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let broker = Arc::new(LiveEventBroker::new());
        let mut subscriber = broker.subscribe().unwrap();
        let gateway = ModelGateway::new(
            store.clone(),
            Arc::new(RejectingArtifactStore),
            provider.clone(),
            broker as Arc<dyn DraftSink>,
            fixture.clock.clone(),
            Box::new(MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap();
        let (_, receiver) = tokio::sync::watch::channel(false);
        assert!(matches!(
            gateway.invoke(invocation(fixture, receiver)).await.unwrap(),
            DurableModelOutcome::Interrupted { .. }
        ));
        assert_eq!(provider.invocation_count(), 1);
        let mut abandoned = None;
        for _ in 0..3 {
            let LiveEventReceive::Event(event) = subscriber.recv().await else {
                panic!("live event expected");
            };
            if event.event_type == "assistant.draft_abandoned" {
                abandoned = Some(event);
            }
        }
        assert!(matches!(
            abandoned.unwrap().payload,
            crate::protocol::DraftEventPayload::Abandoned {
                reason: crate::protocol::DraftAbandonReason::Interrupted
            }
        ));
        assert_eq!(store.attempts()[0].draft_exposed, Some(true));
    }

    #[tokio::test]
    async fn cancellation_before_intent_is_zero_calls_and_during_provider_is_ambiguous() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            Vec::new(),
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (sender, receiver) = tokio::sync::watch::channel(true);
        drop(sender);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(
            result,
            DurableModelOutcome::CancelledBeforeAttempt { .. }
        ));
        assert_eq!(provider.invocation_count(), 0);
        assert!(store.attempts().is_empty());

        let fixture = gateway_fixture();
        let gate = ScriptGate::new();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![ScriptedStep::AwaitRelease(gate)],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { gateway.invoke(invocation(fixture, receiver)).await });
        while provider.invocation_count() == 0 {
            tokio::task::yield_now().await;
        }
        sender.send_replace(true);
        let result = task.await.unwrap().unwrap();
        assert!(matches!(result, DurableModelOutcome::Interrupted { .. }));
        assert_eq!(provider.invocation_count(), 1);
        assert_eq!(
            store.attempts()[0].terminal_state,
            Some(ModelInvocationState::ProviderOutcomeUnknown)
        );
    }

    #[tokio::test]
    async fn cancellation_and_deadline_while_invoke_stream_is_pending_are_ambiguous() {
        let fixture = gateway_fixture();
        let provider = Arc::new(PendingInvocationProvider::new(
            ProviderId::try_new("fixture").unwrap(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), fixture.clock.clone());
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { gateway.invoke(invocation(fixture, receiver)).await });
        while provider.calls() == 0 {
            tokio::task::yield_now().await;
        }
        sender.send_replace(true);
        let result = task.await.unwrap().unwrap();
        assert!(matches!(result, DurableModelOutcome::Interrupted { .. }));
        assert_eq!(provider.calls(), 1);
        assert_eq!(
            store.attempts()[0].terminal_state,
            Some(ModelInvocationState::ProviderOutcomeUnknown)
        );

        let fixture = gateway_fixture();
        let clock = fixture.clock.clone();
        let provider = Arc::new(PendingInvocationProvider::new(
            ProviderId::try_new("fixture").unwrap(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider.clone(), clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let mut pending = invocation(fixture, receiver);
        pending.shutdown_deadline = Some(
            clock
                .monotonic_now()
                .checked_add(Duration::from_millis(25))
                .unwrap(),
        );
        let result = gateway.invoke(pending).await.unwrap();
        assert!(matches!(result, DurableModelOutcome::Interrupted { .. }));
        assert_eq!(provider.calls(), 1);
        assert_eq!(
            store.attempts()[0].terminal_state,
            Some(ModelInvocationState::ProviderOutcomeUnknown)
        );
    }

    #[tokio::test]
    async fn provider_reported_usage_survives_failed_terminal_attempt() {
        let fixture = gateway_fixture();
        let provider = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new("fixture").unwrap(),
            vec![program(
                &fixture,
                1,
                1,
                vec![
                    ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                        target: fixture.target.identity(),
                        provider_request_id: None,
                        provider_response_id: None,
                    }),
                    ScriptedStep::emit(ModelStreamEvent::Usage(usage())),
                    ScriptedStep::emit(ModelStreamEvent::ProviderError {
                        kind: ModelStreamProviderErrorKind::DefiniteFailure,
                    }),
                ],
            )],
            fixture.clock.clone(),
        ));
        let store = Arc::new(FakeGatewayStore::default());
        let gateway = build_gateway(store.clone(), provider, fixture.clock.clone());
        let (_, receiver) = tokio::sync::watch::channel(false);
        let result = gateway.invoke(invocation(fixture, receiver)).await.unwrap();
        assert!(matches!(result, DurableModelOutcome::Failed { .. }));
        assert_eq!(
            store.attempts()[0].usage_status,
            Some(ModelUsageStatus::Reported)
        );
    }
}
