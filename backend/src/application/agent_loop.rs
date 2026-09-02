//! Explicit bounded Stage 17 agent loop and scheduler `WorkRunner` implementation.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use crate::application::authority::V0AuthorityConstraints;
use crate::application::context_assembler::{
    ContextAssembler, ContextAssemblyErrorKind, ContextAssemblyVersions,
};
use crate::application::model_gateway::{
    DraftAbandonCause, DurableModelAttempt, DurableModelOutcome, GatewayInvocation, ModelGateway,
};
use crate::application::model_selection::ModelSelectionPolicy;
use crate::application::scheduler::{
    WorkCancellation, WorkRunner, WorkRunnerExit, WorkRunnerFuture, WorkRunnerStartError,
};
use crate::application::tool_execution_service::{
    ToolCancellationNotice, ToolExecutionCall, ToolExecutionService, ToolExecutionServiceErrorKind,
};
use crate::domain::model::RequiredModelCapabilities;
use crate::domain::{
    AgentStepNo, CleanupStatus, ContentBlock, CurrentWorkAttempt, JournalEventId, LifecycleLimit,
    Message, MessageContent, MessageId, MessageInput, MessageRole, ModelOutputFailure,
    ModelOutputItem, ModelResponse, ModelStopReason, NormalizedError, ToolOrdinal, UtcTimestamp,
    WorkCompletionEvidence, WorkCompletionReason, WorkFailureReason, WorkLifecycleSnapshot,
    WorkState, WorkTransitionGuard, WorkTransitionRequest, WorkspaceIdentity, WorkstationIdentity,
    decide_work_transition,
};
use crate::ports::clock::{Clock, MonotonicInstant};
use crate::ports::model_provider::ProviderErrorKind;
use crate::ports::state_store::{
    CommitAssistantCompletionRequest, CompletionStateStore, EventIntent, LoadOwnedWorkRequest,
    ModelExpectation, ModelStateStore, OwnedWorkState, TerminalizeOwnedWorkRequest,
    WorkExpectation,
};

pub const MAX_MODEL_STEPS_PER_WORK: u32 = 16;
pub const MAX_PROVIDER_ATTEMPTS_PER_WORK: u32 = 32;
pub const MAX_TOOL_CALLS_PER_WORK: u32 = 32;
pub const MAX_WORK_DURATION: Duration = Duration::from_secs(30 * 60);

/// Exact state-store capabilities the loop needs; SQL and adapter types remain behind the port.
pub trait AgentLoopStateStore: ModelStateStore + CompletionStateStore {}

impl<T> AgentLoopStateStore for T where T: ModelStateStore + CompletionStateStore {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentLoopLimits {
    pub model_steps_per_work: u32,
    pub provider_attempts_per_work: u32,
    pub tool_calls_per_work: u32,
    pub work_duration: Duration,
}

impl Default for AgentLoopLimits {
    fn default() -> Self {
        Self {
            model_steps_per_work: MAX_MODEL_STEPS_PER_WORK,
            provider_attempts_per_work: MAX_PROVIDER_ATTEMPTS_PER_WORK,
            tool_calls_per_work: MAX_TOOL_CALLS_PER_WORK,
            work_duration: MAX_WORK_DURATION,
        }
    }
}

impl AgentLoopLimits {
    fn validate(self) -> Result<Self, AgentLoopError> {
        if self.model_steps_per_work != MAX_MODEL_STEPS_PER_WORK
            || self.provider_attempts_per_work != MAX_PROVIDER_ATTEMPTS_PER_WORK
            || self.tool_calls_per_work != MAX_TOOL_CALLS_PER_WORK
            || self.work_duration != MAX_WORK_DURATION
        {
            return Err(AgentLoopError::InvalidComposition);
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub struct AgentLoopRuntimeContext {
    pub workstation: WorkstationIdentity,
    pub workspace: WorkspaceIdentity,
    pub authority_constraints: V0AuthorityConstraints,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentLoopError {
    InvalidComposition,
    StateStore,
    Clock,
    Lifecycle,
    Content,
}

impl Display for AgentLoopError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidComposition => "invalid agent-loop composition",
            Self::StateStore => "agent-loop state-store failure",
            Self::Clock => "agent-loop clock failure",
            Self::Lifecycle => "agent-loop lifecycle failure",
            Self::Content => "agent-loop content failure",
        })
    }
}

impl std::error::Error for AgentLoopError {}

/// Concrete explicit-loop runner installed behind the scheduler's sole execution boundary.
#[derive(Clone)]
pub struct AgentLoop {
    selection: Arc<ModelSelectionPolicy>,
    context_assembler: Arc<ContextAssembler>,
    context_versions: ContextAssemblyVersions,
    model_gateway: Arc<ModelGateway>,
    tool_execution: Arc<ToolExecutionService>,
    state_store: Arc<dyn AgentLoopStateStore>,
    clock: Arc<dyn Clock>,
    required_capabilities: RequiredModelCapabilities,
    runtime_context: AgentLoopRuntimeContext,
    limits: AgentLoopLimits,
}

impl fmt::Debug for AgentLoop {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentLoop")
            .field("limits", &self.limits)
            .field(
                "workstation_id",
                &self.runtime_context.workstation.workstation_id(),
            )
            .field(
                "workspace_id",
                &self.runtime_context.workspace.workspace_id(),
            )
            .finish_non_exhaustive()
    }
}

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        selection: Arc<ModelSelectionPolicy>,
        context_assembler: Arc<ContextAssembler>,
        context_versions: ContextAssemblyVersions,
        model_gateway: Arc<ModelGateway>,
        tool_execution: Arc<ToolExecutionService>,
        state_store: Arc<dyn AgentLoopStateStore>,
        clock: Arc<dyn Clock>,
        required_capabilities: RequiredModelCapabilities,
        runtime_context: AgentLoopRuntimeContext,
        limits: AgentLoopLimits,
    ) -> Result<Self, AgentLoopError> {
        if runtime_context.workspace.workstation_id()
            != runtime_context.workstation.workstation_id()
            || runtime_context.workspace.craxii_id() != runtime_context.workstation.craxii_id()
        {
            return Err(AgentLoopError::InvalidComposition);
        }
        Ok(Self {
            selection,
            context_assembler,
            context_versions,
            model_gateway,
            tool_execution,
            state_store,
            clock,
            required_capabilities,
            runtime_context,
            limits: limits.validate()?,
        })
    }

    async fn run(
        self: Arc<Self>,
        claimed: crate::ports::state_store::ClaimedWork,
        mut cancellation: WorkCancellation,
    ) -> WorkRunnerExit {
        let Some(work_deadline) = self
            .clock
            .monotonic_now()
            .checked_add(self.limits.work_duration)
        else {
            return WorkRunnerExit::Abnormal;
        };
        let runtime_id = match claimed.lifecycle.runtime_owner() {
            Some(value) => value,
            None => return WorkRunnerExit::Abnormal,
        };
        if claimed.work.work_id() != claimed.lifecycle.work_id()
            || claimed.work.craxii_id() != self.runtime_context.workspace.craxii_id()
            || claimed.work.workspace_id() != self.runtime_context.workspace.workspace_id()
        {
            return WorkRunnerExit::Abnormal;
        }

        let mut agent_step = match AgentStepNo::try_new(1) {
            Ok(value) => value,
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        loop {
            let owned = match self.load_owned(claimed.work.work_id(), runtime_id).await {
                Ok(value) => value,
                Err(_) => return WorkRunnerExit::Abnormal,
            };
            if owned.lifecycle.state() == WorkState::CancelRequested {
                return if owned.lifecycle.current_attempt() == CurrentWorkAttempt::None {
                    WorkRunnerExit::CancellationConfirmed
                } else {
                    WorkRunnerExit::Abnormal
                };
            }
            if owned.lifecycle.state() != WorkState::Running
                || owned.lifecycle.runtime_owner() != Some(runtime_id)
                || owned.lifecycle.current_attempt() != CurrentWorkAttempt::None
            {
                return WorkRunnerExit::Abnormal;
            }
            if cancellation.is_requested() {
                return WorkRunnerExit::CancellationConfirmed;
            }
            if self.clock.monotonic_now() >= work_deadline {
                return self
                    .fail_owned(
                        owned,
                        WorkFailureReason::Limit(LifecycleLimit::TotalWorkTime),
                    )
                    .await;
            }
            if agent_step.get() > i64::from(self.limits.model_steps_per_work) {
                return self
                    .fail_owned(
                        owned,
                        WorkFailureReason::Limit(LifecycleLimit::AgentLoopSteps),
                    )
                    .await;
            }
            if owned.model_attempt_count >= u64::from(self.limits.provider_attempts_per_work) {
                return self
                    .fail_owned(
                        owned,
                        WorkFailureReason::Limit(LifecycleLimit::ModelAttempts),
                    )
                    .await;
            }
            if owned.tool_call_count > u64::from(self.limits.tool_calls_per_work) {
                return self
                    .fail_owned(owned, WorkFailureReason::Limit(LifecycleLimit::ToolCalls))
                    .await;
            }

            let selection = match self.selection.select(None, self.required_capabilities) {
                Ok(value) => value,
                Err(error) => {
                    return self
                        .fail_owned(owned, WorkFailureReason::Definite(error.normalized()))
                        .await;
                }
            };
            let context = match self
                .context_assembler
                .assemble(owned.work.work_id(), &selection, &self.context_versions)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let reason = if error.kind() == ContextAssemblyErrorKind::ContextLimitExceeded {
                        WorkFailureReason::Limit(LifecycleLimit::Context)
                    } else {
                        WorkFailureReason::Definite(error.normalized())
                    };
                    return self.fail_owned(owned, reason).await;
                }
            };
            let work_attempts_before_invocation = match u32::try_from(owned.model_attempt_count) {
                Ok(value) => value,
                Err(_) => {
                    return self
                        .fail_owned(
                            owned,
                            WorkFailureReason::Limit(LifecycleLimit::ModelAttempts),
                        )
                        .await;
                }
            };
            let gateway_outcome = self
                .model_gateway
                .invoke(GatewayInvocation {
                    craxii_id: owned.work.craxii_id(),
                    conversation_id: owned.work.conversation_id(),
                    work: owned.lifecycle,
                    context,
                    selection,
                    agent_step,
                    correlation_id: owned.work.correlation_id(),
                    causation_event_id: owned.latest_work_event_id,
                    cancellation: cancellation.receiver(),
                    work_deadline,
                    shutdown_deadline: None,
                    work_attempts_before_invocation,
                })
                .await;
            let outcome = match gateway_outcome {
                Ok(value) => value,
                Err(_) => return WorkRunnerExit::Abnormal,
            };
            match outcome {
                DurableModelOutcome::Interrupted { .. } => {
                    return WorkRunnerExit::TerminalCommitted;
                }
                DurableModelOutcome::CancelledBeforeAttempt { work } => {
                    return if work.state().is_terminal() {
                        WorkRunnerExit::TerminalCommitted
                    } else {
                        WorkRunnerExit::CancellationConfirmed
                    };
                }
                DurableModelOutcome::DeadlineBeforeAttempt { work } => {
                    return self
                        .fail_snapshot(
                            work,
                            claimed.work.correlation_id(),
                            WorkFailureReason::Limit(LifecycleLimit::TotalWorkTime),
                        )
                        .await;
                }
                DurableModelOutcome::Failed {
                    error_kind,
                    semantic_output_observed,
                    retries_exhausted,
                    attempt,
                } => {
                    let reason = provider_failure_reason(
                        error_kind,
                        semantic_output_observed,
                        retries_exhausted,
                    );
                    return self
                        .fail_snapshot(attempt.work, claimed.work.correlation_id(), reason)
                        .await;
                }
                DurableModelOutcome::Completed { response, attempt } => {
                    if attempt.work.state().is_terminal() {
                        return WorkRunnerExit::TerminalCommitted;
                    }
                    match response.stop_reason() {
                        ModelStopReason::Completed => {
                            return self
                                .commit_completion(
                                    &claimed.work,
                                    *response,
                                    attempt,
                                    WorkCompletionReason::Answered,
                                )
                                .await;
                        }
                        ModelStopReason::Refusal => {
                            return self
                                .commit_completion(
                                    &claimed.work,
                                    *response,
                                    attempt,
                                    WorkCompletionReason::Refused,
                                )
                                .await;
                        }
                        ModelStopReason::ToolContinuation => {
                            self.model_gateway.abandon_draft(
                                attempt.model_invocation_id,
                                DraftAbandonCause::ToolContinuation,
                            );
                            match self
                                .execute_tool_batch(
                                    &claimed.work,
                                    *response,
                                    attempt,
                                    agent_step,
                                    runtime_id,
                                    work_deadline,
                                    &mut cancellation,
                                )
                                .await
                            {
                                ToolBatchOutcome::Continue => {
                                    agent_step = match agent_step.checked_increment() {
                                        Ok(value) => value,
                                        Err(_) => return WorkRunnerExit::Abnormal,
                                    };
                                }
                                ToolBatchOutcome::Terminal(exit) => return exit,
                            }
                        }
                        ModelStopReason::IncompleteProviderLimit => {
                            self.model_gateway.abandon_draft(
                                attempt.model_invocation_id,
                                DraftAbandonCause::Failed,
                            );
                            return self
                                .fail_snapshot(
                                    attempt.work,
                                    claimed.work.correlation_id(),
                                    WorkFailureReason::InvalidModelOutput(
                                        ModelOutputFailure::Incomplete,
                                    ),
                                )
                                .await;
                        }
                        ModelStopReason::Cancelled | ModelStopReason::ProviderFailure => {
                            self.model_gateway.abandon_draft(
                                attempt.model_invocation_id,
                                if response.stop_reason() == ModelStopReason::Cancelled {
                                    DraftAbandonCause::Cancelled
                                } else {
                                    DraftAbandonCause::Failed
                                },
                            );
                            return self
                                .fail_snapshot(
                                    attempt.work,
                                    claimed.work.correlation_id(),
                                    WorkFailureReason::InvalidModelOutput(
                                        ModelOutputFailure::ProviderFailed,
                                    ),
                                )
                                .await;
                        }
                    }
                }
            }
        }
    }

    async fn load_owned(
        &self,
        work_id: crate::domain::WorkId,
        runtime_id: crate::domain::RuntimeInstanceId,
    ) -> Result<OwnedWorkState, AgentLoopError> {
        self.state_store
            .load_owned_work(LoadOwnedWorkRequest {
                work_id,
                runtime_id,
            })
            .await
            .map_err(|_| AgentLoopError::StateStore)
    }

    async fn fail_owned(&self, owned: OwnedWorkState, reason: WorkFailureReason) -> WorkRunnerExit {
        self.fail_snapshot(owned.lifecycle, owned.work.correlation_id(), reason)
            .await
    }

    async fn fail_snapshot(
        &self,
        work: WorkLifecycleSnapshot,
        correlation_id: crate::domain::CorrelationId,
        reason: WorkFailureReason,
    ) -> WorkRunnerExit {
        if work.state() != WorkState::Running {
            return WorkRunnerExit::Abnormal;
        }
        let failed = match decide_work_transition(
            &work,
            WorkTransitionGuard::for_snapshot(&work),
            WorkTransitionRequest::Fail {
                reason,
                cleanup_status: CleanupStatus::NotRequired,
            },
        ) {
            Ok(value) => value.into_next(),
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        let runtime_id = match work.runtime_owner() {
            Some(value) => value,
            None => return WorkRunnerExit::Abnormal,
        };
        let current = match self.load_owned(work.work_id(), runtime_id).await {
            Ok(value) => value,
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        if current.lifecycle.state() != work.state()
            || current.lifecycle.projection_version() != work.projection_version()
            || current.lifecycle.current_attempt() != work.current_attempt()
        {
            return WorkRunnerExit::Abnormal;
        }
        let terminal_at = match self.wall_now() {
            Ok(value) => value,
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        match self
            .state_store
            .terminalize_owned_work(TerminalizeOwnedWorkRequest {
                expected_work: WorkExpectation::for_snapshot(&work),
                work_next: failed,
                terminal_at,
                event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id,
                    causation_event_id: Some(current.latest_work_event_id),
                },
            })
            .await
        {
            Ok(_) => WorkRunnerExit::TerminalCommitted,
            Err(_) => WorkRunnerExit::Abnormal,
        }
    }

    async fn commit_completion(
        &self,
        work_item: &crate::domain::WorkItem,
        response: ModelResponse,
        attempt: DurableModelAttempt,
        reason: WorkCompletionReason,
    ) -> WorkRunnerExit {
        let content = match assistant_content(&response, reason) {
            Ok(value) => value,
            Err(_) => {
                self.model_gateway
                    .abandon_draft(attempt.model_invocation_id, DraftAbandonCause::Failed);
                return self
                    .fail_snapshot(
                        attempt.work,
                        work_item.correlation_id(),
                        WorkFailureReason::InvalidModelOutput(ModelOutputFailure::ProviderFailed),
                    )
                    .await;
            }
        };
        let committed_at = match self.wall_now() {
            Ok(value) => value,
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        let completed = match decide_work_transition(
            &attempt.work,
            WorkTransitionGuard::for_snapshot(&attempt.work),
            WorkTransitionRequest::Complete {
                reason,
                evidence: WorkCompletionEvidence::SATISFIED,
            },
        ) {
            Ok(value) => value.into_next(),
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        let message = match Message::try_new(MessageInput {
            message_id: MessageId::generate(),
            craxii_id: work_item.craxii_id(),
            conversation_id: work_item.conversation_id(),
            role: MessageRole::Assistant,
            content,
            produced_by_work_id: Some(work_item.work_id()),
            device_id: None,
            client_message_id: None,
            committed_at,
        }) {
            Ok(value) => value,
            Err(_) => return WorkRunnerExit::Abnormal,
        };
        let assistant_event = JournalEventId::generate();
        match self
            .state_store
            .commit_assistant_completion(CommitAssistantCompletionRequest {
                expected_work: WorkExpectation::for_snapshot(&attempt.work),
                expected_model: ModelExpectation {
                    model_invocation_id: attempt.model_invocation_id,
                    state: crate::domain::ModelInvocationState::Completed,
                },
                assistant_message: message,
                assistant_event: EventIntent {
                    event_id: assistant_event,
                    correlation_id: work_item.correlation_id(),
                    causation_event_id: Some(attempt.terminal_model_event_id),
                },
                completion_event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id: work_item.correlation_id(),
                    causation_event_id: Some(assistant_event),
                },
                work_next: completed,
            })
            .await
        {
            Ok(_) => {
                #[cfg(feature = "test-failpoints")]
                crate::test_failpoints::reach(
                    crate::test_failpoints::PhysicalHook::FinalAnswerAfterCommitBeforeNotification,
                );
                self.model_gateway
                    .finalize_drafts_for_work(work_item.work_id());
                WorkRunnerExit::TerminalCommitted
            }
            Err(_) => {
                self.model_gateway
                    .abandon_draft(attempt.model_invocation_id, DraftAbandonCause::Interrupted);
                WorkRunnerExit::Abnormal
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_tool_batch(
        &self,
        work_item: &crate::domain::WorkItem,
        response: ModelResponse,
        attempt: DurableModelAttempt,
        agent_step: AgentStepNo,
        runtime_id: crate::domain::RuntimeInstanceId,
        work_deadline: MonotonicInstant,
        cancellation: &mut WorkCancellation,
    ) -> ToolBatchOutcome {
        let calls = response
            .output_items()
            .iter()
            .filter_map(ModelOutputItem::tool_call)
            .cloned()
            .collect::<Vec<_>>();
        if calls.is_empty()
            || calls
                .iter()
                .any(|call| call.require_valid_arguments().is_err())
        {
            return ToolBatchOutcome::Terminal(
                self.fail_snapshot(
                    attempt.work,
                    work_item.correlation_id(),
                    WorkFailureReason::InvalidModelOutput(ModelOutputFailure::ProviderFailed),
                )
                .await,
            );
        }
        let owned = match self.load_owned(work_item.work_id(), runtime_id).await {
            Ok(value) => value,
            Err(_) => return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal),
        };
        let call_count = match u64::try_from(calls.len()) {
            Ok(value) => value,
            Err(_) => {
                return ToolBatchOutcome::Terminal(
                    self.fail_owned(owned, WorkFailureReason::Limit(LifecycleLimit::ToolCalls))
                        .await,
                );
            }
        };
        if owned
            .tool_call_count
            .checked_add(call_count)
            .is_none_or(|value| value > u64::from(self.limits.tool_calls_per_work))
        {
            return ToolBatchOutcome::Terminal(
                self.fail_owned(owned, WorkFailureReason::Limit(LifecycleLimit::ToolCalls))
                    .await,
            );
        }

        let mut current_work = attempt.work;
        for (index, call) in calls.into_iter().enumerate() {
            if cancellation.is_requested() {
                return ToolBatchOutcome::Terminal(WorkRunnerExit::CancellationConfirmed);
            }
            if self.clock.monotonic_now() >= work_deadline {
                return ToolBatchOutcome::Terminal(
                    self.fail_snapshot(
                        current_work,
                        work_item.correlation_id(),
                        WorkFailureReason::Limit(LifecycleLimit::TotalWorkTime),
                    )
                    .await,
                );
            }
            let tool_ordinal = match ToolOrdinal::try_new(
                i64::try_from(index.saturating_add(1)).unwrap_or(i64::MAX),
            ) {
                Ok(value) => value,
                Err(_) => return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal),
            };
            let (notice_sender, notice_receiver) = tokio::sync::watch::channel(None);
            let execute = self.tool_execution.execute_call(ToolExecutionCall {
                craxii_id: work_item.craxii_id(),
                work: current_work,
                runtime_instance_id: runtime_id,
                source_model_invocation_id: attempt.model_invocation_id,
                source_model_event_id: attempt.terminal_model_event_id,
                agent_step_no: agent_step,
                tool_ordinal,
                provider_tool_call_id: Some(call.call_id().as_str().to_owned()),
                tool_name: call.name().as_str().to_owned(),
                raw_arguments: call.raw_arguments().as_bytes().to_vec(),
                correlation_id: work_item.correlation_id(),
                workstation_id: self.runtime_context.workstation.workstation_id(),
                workstation_generation: self.runtime_context.workstation.generation(),
                workspace: self.runtime_context.workspace.clone(),
                work_deadline,
                shutdown_deadline: None,
                authority_constraints: self.runtime_context.authority_constraints,
                cancellation: Some(notice_receiver),
            });
            tokio::pin!(execute);
            let mut cancellation_forwarded = false;
            let result = tokio::select! {
                biased;
                () = cancellation.requested() => {
                    let owned = match self.load_owned(work_item.work_id(), runtime_id).await {
                        Ok(value) => value,
                        Err(_) => return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal),
                    };
                    if owned.lifecycle.state() != WorkState::CancelRequested {
                        return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal);
                    }
                    let Some(reason) = owned.lifecycle.cancellation_reason() else {
                        return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal);
                    };
                    cancellation_forwarded = true;
                    notice_sender.send_replace(Some(ToolCancellationNotice {
                        expected_work: WorkExpectation::for_snapshot(&owned.lifecycle),
                        reason,
                    }));
                    execute.await
                }
                result = &mut execute => result,
            };
            match result {
                Ok(_) if cancellation_forwarded => {
                    return ToolBatchOutcome::Terminal(WorkRunnerExit::TerminalCommitted);
                }
                Ok(_) => {
                    current_work = match self.load_owned(work_item.work_id(), runtime_id).await {
                        Ok(value) => value.lifecycle,
                        Err(_) => return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal),
                    };
                }
                Err(error) if error.kind() == ToolExecutionServiceErrorKind::OutcomeUnknown => {
                    return ToolBatchOutcome::Terminal(WorkRunnerExit::TerminalCommitted);
                }
                Err(error)
                    if cancellation_forwarded
                        && error.kind() == ToolExecutionServiceErrorKind::CancelledBeforeIntent =>
                {
                    return ToolBatchOutcome::Terminal(WorkRunnerExit::CancellationConfirmed);
                }
                Err(_) => return ToolBatchOutcome::Terminal(WorkRunnerExit::Abnormal),
            }
        }
        ToolBatchOutcome::Continue
    }

    fn wall_now(&self) -> Result<UtcTimestamp, AgentLoopError> {
        self.clock
            .utc_now()
            .map_err(|_| AgentLoopError::Clock)
            .and_then(|value| {
                UtcTimestamp::from_offset_datetime(value).map_err(|_| AgentLoopError::Clock)
            })
    }
}

impl WorkRunner for AgentLoop {
    fn start(
        &self,
        work: crate::ports::state_store::ClaimedWork,
        cancellation: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let loop_service = Arc::new(self.clone());
        Ok(Box::pin(async move {
            loop_service.run(work, cancellation).await
        }))
    }
}

enum ToolBatchOutcome {
    Continue,
    Terminal(WorkRunnerExit),
}

fn provider_failure_reason(
    kind: ProviderErrorKind,
    semantic_output_observed: bool,
    retries_exhausted: bool,
) -> WorkFailureReason {
    if semantic_output_observed {
        WorkFailureReason::InvalidModelOutput(ModelOutputFailure::ProviderFailed)
    } else if retries_exhausted {
        WorkFailureReason::ProviderExhausted
    } else if kind == ProviderErrorKind::TimeoutBeforeOutput {
        WorkFailureReason::Limit(LifecycleLimit::ModelInvocationTime)
    } else if kind == ProviderErrorKind::ContextError {
        WorkFailureReason::Limit(LifecycleLimit::Context)
    } else if kind == ProviderErrorKind::Authentication {
        WorkFailureReason::Definite(NormalizedError::authentication())
    } else {
        WorkFailureReason::Definite(NormalizedError::provider(
            crate::domain::Certainty::Definite,
            None,
        ))
    }
}

fn assistant_content(
    response: &ModelResponse,
    completion: WorkCompletionReason,
) -> Result<MessageContent, AgentLoopError> {
    let mut blocks = Vec::new();
    for item in response.output_items() {
        match (completion, item) {
            (WorkCompletionReason::Answered, ModelOutputItem::Text { content_parts }) => {
                blocks.push(
                    ContentBlock::text(
                        content_parts
                            .iter()
                            .map(crate::domain::ModelTextPart::as_str)
                            .collect::<String>(),
                    )
                    .map_err(|_| AgentLoopError::Content)?,
                );
            }
            (WorkCompletionReason::Answered, ModelOutputItem::StructuredData { data }) => {
                blocks.push(
                    ContentBlock::text(
                        serde_json::to_string(data).map_err(|_| AgentLoopError::Content)?,
                    )
                    .map_err(|_| AgentLoopError::Content)?,
                );
            }
            (WorkCompletionReason::Refused, ModelOutputItem::Refusal { content_parts }) => {
                blocks.push(
                    ContentBlock::text(
                        content_parts
                            .iter()
                            .map(crate::domain::ModelTextPart::as_str)
                            .collect::<String>(),
                    )
                    .map_err(|_| AgentLoopError::Content)?,
                );
            }
            (_, ModelOutputItem::ReasoningSummary { .. } | ModelOutputItem::ProviderOpaque(_)) => {}
            _ => return Err(AgentLoopError::Content),
        }
    }
    MessageContent::try_new(blocks).map_err(|_| AgentLoopError::Content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_agent_loop_limits_are_exact() {
        let limits = AgentLoopLimits::default().validate().unwrap();
        assert_eq!(limits.model_steps_per_work, 16);
        assert_eq!(limits.provider_attempts_per_work, 32);
        assert_eq!(limits.tool_calls_per_work, 32);
        assert_eq!(limits.work_duration, Duration::from_secs(30 * 60));
    }
}
