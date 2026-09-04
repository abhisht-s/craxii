//! Durable Stage 14 orchestration around the thin handlers and Workstation boundary.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tracing::Instrument;

use serde_json::json;

use crate::application::authority::{
    AuthorityEvaluationInput, AuthorityEvaluator, V0AuthorityConstraints,
};
use crate::application::tool_handlers::{
    ToolExecutionContext, ToolHandlerFuture, ToolHandlerObservation, resolve_handler,
};
use crate::application::tool_registry::{
    READ_FILE_PROJECTION_BYTES, ToolDefinition, ToolRegistry, ValidatedToolArguments,
    ValidatedToolInput, validate_arguments,
};
use crate::domain::{
    AgentStepNo, ArtifactEncoding, ArtifactId, ArtifactLogicalName, ArtifactMimeType,
    ArtifactProducer, ArtifactReference, ArtifactReferenceInput, ArtifactRetention,
    ArtifactStorageKey, AuthorityDecisionSnapshot, CanonicalByteCount, Certainty, CleanupStatus,
    CorrelationId, CraxiiId, ExecutionId, JournalEventId, LogicalPathReference, ModelInvocationId,
    MonotonicDuration, NormalizedError, OperationId, PrivilegeMode, ProjectionVersion,
    RuntimeInstanceId, SchemaVersion, Sha256Digest, ToolExecutionId, ToolExecutionState,
    ToolLifecycleReference, ToolName, ToolOrdinal, ToolResultClass, ToolVersion, UtcTimestamp,
    WorkCancellationReason, WorkInterruptionReason, WorkLifecycleSnapshot,
    WorkLifecycleSnapshotInput, WorkState, WorkTransitionGuard, WorkTransitionRequest,
    WorkspaceIdentity, WorkstationGeneration, WorkstationId, decide_work_transition,
};
use crate::ports::artifact_store::{ArtifactStore, BeginArtifactCapture, FinalizedArtifact};
use crate::ports::clock::{Clock, MonotonicInstant};
use crate::ports::state_store::{
    CommitToolDispatchIntentRequest, EventIntent, FinishToolExecutionRequest,
    MAX_TOOL_RESULT_JSON_BYTES, PreparedArtifact, PreparedToolExecution,
    RequestToolExecutionRequest, StateStoreError, ToolDispatchIntent, ToolExpectation,
    ToolOutputPolicy, ToolResultEvidence, ToolStateStore, ToolStreamCounts, ToolTerminalOutcome,
    WorkExpectation,
};
use crate::ports::workstation::{
    CapabilitiesRequest, ExecutionCancellationRequest, ExecutionCancellationState,
    ExecutionCapturePolicy, ExecutionResult, ExecutionResultKind, ExecutionStreamResult,
    HARD_EXECUTION_STREAM_CAPTURE_BYTES, HARD_EXECUTION_TIMEOUT_MS, Workstation, WorkstationError,
    WorkstationErrorKind,
};
use crate::ports::workstation_preparation::{
    PreparedCwdEvidence, PreparedCwdObjectType, WorkstationPreparation,
    WorkstationPreparationRequest,
};

/// V0 runtime ceilings injected from validated configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolRuntimeLimits {
    pub read_file_default_bytes: u64,
    pub read_file_max_bytes: u64,
    pub run_shell_command_max_bytes: u64,
    pub run_shell_default_timeout_ms: u64,
    pub run_shell_max_timeout_ms: u64,
    pub stdout_capture_bytes: u64,
    pub stderr_capture_bytes: u64,
    pub inline_model_result_bytes: u64,
    pub per_stream_projection_bytes: u64,
}

impl ToolRuntimeLimits {
    pub fn validate(self) -> Result<Self, ToolExecutionServiceError> {
        if self.read_file_default_bytes == 0
            || self.read_file_default_bytes > self.read_file_max_bytes
            || self.read_file_max_bytes > crate::ports::workstation::HARD_FILE_READ_MAX_BYTES
            || self.run_shell_command_max_bytes == 0
            || self.run_shell_command_max_bytes
                > crate::ports::workstation::HARD_EXECUTION_COMMAND_MAX_BYTES as u64
            || self.run_shell_default_timeout_ms == 0
            || self.run_shell_default_timeout_ms > self.run_shell_max_timeout_ms
            || self.run_shell_max_timeout_ms > HARD_EXECUTION_TIMEOUT_MS
            || self.stdout_capture_bytes > HARD_EXECUTION_STREAM_CAPTURE_BYTES
            || self.stderr_capture_bytes > HARD_EXECUTION_STREAM_CAPTURE_BYTES
            || self.inline_model_result_bytes == 0
            || self.per_stream_projection_bytes == 0
            || self.per_stream_projection_bytes > self.inline_model_result_bytes
        {
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::InvalidComposition,
            ));
        }
        Ok(self)
    }
}

/// A cancellation notice emitted only after the durable cancellation command wins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolCancellationNotice {
    pub expected_work: WorkExpectation,
    pub reason: WorkCancellationReason,
}

/// Complete trusted call context supplied by the future Stage 17 owner.
pub struct ToolExecutionCall {
    pub craxii_id: CraxiiId,
    pub work: WorkLifecycleSnapshot,
    pub runtime_instance_id: RuntimeInstanceId,
    pub source_model_invocation_id: ModelInvocationId,
    pub source_model_event_id: JournalEventId,
    pub agent_step_no: AgentStepNo,
    pub tool_ordinal: ToolOrdinal,
    pub provider_tool_call_id: Option<String>,
    pub tool_name: String,
    pub raw_arguments: Vec<u8>,
    pub correlation_id: CorrelationId,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub workspace: WorkspaceIdentity,
    pub work_deadline: MonotonicInstant,
    pub shutdown_deadline: Option<MonotonicInstant>,
    pub authority_constraints: V0AuthorityConstraints,
    pub cancellation: Option<tokio::sync::watch::Receiver<Option<ToolCancellationNotice>>>,
}

/// Stable model-facing terminal status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolResultStatus {
    Completed,
}

/// Safe normalized error projection for the canonical tool envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResultError {
    pub code: String,
    pub certainty: &'static str,
}

/// Provider-neutral bounded result returned only after durable terminal commit.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolExecutionResult {
    pub tool_execution_id: ToolExecutionId,
    pub execution_id: ExecutionId,
    pub tool_name: ToolName,
    pub tool_version: ToolVersion,
    pub schema_version: SchemaVersion,
    pub status: ToolResultStatus,
    pub result_class: ToolResultClass,
    pub effective_privilege: Option<PrivilegeMode>,
    pub summary: String,
    pub fields: Vec<(String, String)>,
    pub artifact_ids: Vec<ArtifactId>,
    pub truncated: bool,
    pub error: Option<ToolResultError>,
}

impl std::fmt::Debug for ToolExecutionResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolExecutionResult")
            .field("tool_execution_id", &self.tool_execution_id)
            .field("execution_id", &self.execution_id)
            .field("tool_name", &self.tool_name)
            .field("tool_version", &self.tool_version)
            .field("schema_version", &self.schema_version)
            .field("status", &self.status)
            .field("result_class", &self.result_class)
            .field("effective_privilege", &self.effective_privilege)
            .field("field_count", &self.fields.len())
            .field("artifact_count", &self.artifact_ids.len())
            .field("truncated", &self.truncated)
            .field("has_error", &self.error.is_some())
            .finish()
    }
}

/// Safe service failure classes. None authorizes automatic tool replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolExecutionServiceErrorKind {
    InvalidComposition,
    InvalidCallContext,
    CancelledBeforeIntent,
    StateStore,
    Clock,
    Artifact,
    OutcomeUnknown,
    HandlerPanickedBeforeHandoff,
    HandlerPanickedAfterPossibleHandoff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolExecutionServiceError {
    kind: ToolExecutionServiceErrorKind,
}

impl ToolExecutionServiceError {
    const fn new(kind: ToolExecutionServiceErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> ToolExecutionServiceErrorKind {
        self.kind
    }
}

impl std::fmt::Display for ToolExecutionServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("tool execution service failure")
    }
}

impl std::error::Error for ToolExecutionServiceError {}

impl From<StateStoreError> for ToolExecutionServiceError {
    fn from(_: StateStoreError) -> Self {
        Self::new(ToolExecutionServiceErrorKind::StateStore)
    }
}

/// Sole owner of tool persistence and handler dispatch ordering.
pub struct ToolExecutionService {
    registry: Arc<ToolRegistry>,
    authority: Arc<dyn AuthorityEvaluator>,
    state_store: Arc<dyn ToolStateStore>,
    workstation: Arc<dyn Workstation>,
    preparation: Arc<dyn WorkstationPreparation>,
    artifact_store: Arc<dyn ArtifactStore>,
    clock: Arc<dyn Clock>,
    limits: ToolRuntimeLimits,
}

impl std::fmt::Debug for ToolExecutionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolExecutionService")
            .field("registry_fingerprint", &self.registry.fingerprint())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ToolExecutionService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<ToolRegistry>,
        authority: Arc<dyn AuthorityEvaluator>,
        state_store: Arc<dyn ToolStateStore>,
        workstation: Arc<dyn Workstation>,
        preparation: Arc<dyn WorkstationPreparation>,
        artifact_store: Arc<dyn ArtifactStore>,
        clock: Arc<dyn Clock>,
        limits: ToolRuntimeLimits,
    ) -> Result<Self, ToolExecutionServiceError> {
        let limits = limits.validate()?;
        let semantic = registry.semantic_policy();
        if semantic.read_file_default_bytes != limits.read_file_default_bytes
            || semantic.read_file_max_bytes != limits.read_file_max_bytes
            || semantic.run_shell_command_max_bytes != limits.run_shell_command_max_bytes
            || semantic.run_shell_default_timeout_ms != limits.run_shell_default_timeout_ms
            || semantic.run_shell_max_timeout_ms != limits.run_shell_max_timeout_ms
        {
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::InvalidComposition,
            ));
        }
        Ok(Self {
            registry,
            authority,
            state_store,
            workstation,
            preparation,
            artifact_store,
            clock,
            limits,
        })
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Executes one logical call with durable requested/dispatch/outcome ordering and no retry.
    pub async fn execute_call(
        &self,
        call: ToolExecutionCall,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let span = tracing::info_span!(
            "tool_execution_service",
            craxii_id = %call.craxii_id,
            work_id = %call.work.work_id(),
            runtime_instance_id = %call.runtime_instance_id,
            source_model_invocation_id = %call.source_model_invocation_id,
            agent_step = call.agent_step_no.get(),
            tool_ordinal = call.tool_ordinal.get(),
            workstation_id = %call.workstation_id,
            workstation_generation = call.workstation_generation.get(),
            tool_execution_id = tracing::field::Empty,
            workstation_execution_id = tracing::field::Empty,
            tool_name = tracing::field::Empty,
            tool_version = tracing::field::Empty,
            tool_schema_version = tracing::field::Empty,
            arguments_sha256 = tracing::field::Empty,
            validation_result = tracing::field::Empty,
            authority_result = tracing::field::Empty,
            requested_privilege = tracing::field::Empty,
            effective_privilege = tracing::field::Empty,
            effective_timeout_ms = tracing::field::Empty,
            dispatch_intent_persisted = false,
            result_class = tracing::field::Empty,
            outcome_unknown = tracing::field::Empty,
            artifact_count = tracing::field::Empty,
            output_observed_bytes = tracing::field::Empty,
            stdout_observed_bytes = tracing::field::Empty,
            stdout_captured_bytes = tracing::field::Empty,
            stderr_observed_bytes = tracing::field::Empty,
            stderr_captured_bytes = tracing::field::Empty,
            exit_code = tracing::field::Empty,
            signal = tracing::field::Empty,
            timed_out = tracing::field::Empty,
            cancelled = tracing::field::Empty,
            cleanup_result = tracing::field::Empty,
            truncated = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
        );
        let started = Instant::now();
        let result = self.execute_call_inner(call).instrument(span.clone()).await;
        span.record(
            "duration_ms",
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
        match &result {
            Ok(value) => {
                span.record(
                    "tool_execution_id",
                    tracing::field::display(value.tool_execution_id),
                );
                span.record(
                    "workstation_execution_id",
                    tracing::field::display(value.execution_id),
                );
                span.record("tool_name", value.tool_name.as_str());
                if let Some(privilege) = value.effective_privilege {
                    span.record("effective_privilege", tracing::field::debug(privilege));
                }
                span.record("result_class", value.result_class.as_str());
                span.record("outcome_unknown", false);
                span.record(
                    "artifact_count",
                    u64::try_from(value.artifact_ids.len()).unwrap_or(u64::MAX),
                );
                span.record("truncated", value.truncated);
                tracing::info!(
                    event_name = "tool_execution_terminal",
                    tool_execution_id = %value.tool_execution_id,
                    execution_id = %value.execution_id,
                    result_class = value.result_class.as_str()
                );
            }
            Err(error) => {
                span.record("result_class", format!("{:?}", error.kind()).as_str());
                if matches!(
                    error.kind(),
                    ToolExecutionServiceErrorKind::OutcomeUnknown
                        | ToolExecutionServiceErrorKind::HandlerPanickedAfterPossibleHandoff
                ) {
                    span.record("outcome_unknown", true);
                }
                tracing::warn!(
                    event_name = "tool_execution_terminal",
                    result_class = ?error.kind()
                );
            }
        }
        result
    }

    async fn execute_call_inner(
        &self,
        mut call: ToolExecutionCall,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        validate_call_context(&call)?;
        if current_cancellation(&call.cancellation).is_some() {
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::CancelledBeforeIntent,
            ));
        }
        let invocation_start = self.clock.monotonic_now();
        if invocation_start >= effective_outer_deadline(call.work_deadline, call.shutdown_deadline)
        {
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::CancelledBeforeIntent,
            ));
        }
        let tool_name = ToolName::try_new(call.tool_name.clone()).map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext)
        })?;
        tracing::Span::current().record("tool_name", tool_name.as_str());
        let definition = self.registry.lookup(&tool_name);
        let validation =
            definition.map(|definition| validate_arguments(definition, &call.raw_arguments));
        let rejected = match validation.as_ref() {
            None => Some(ToolResultClass::UnknownTool),
            Some(Err(_)) => Some(ToolResultClass::ValidationRejection),
            Some(Ok(arguments)) if !within_runtime_limits(arguments, self.limits) => {
                Some(ToolResultClass::ValidationRejection)
            }
            Some(Ok(_)) => None,
        };
        tracing::Span::current().record(
            "validation_result",
            match rejected {
                Some(ToolResultClass::UnknownTool) => "unknown_tool",
                Some(_) => "rejected",
                None => "valid",
            },
        );
        let (tool_version, schema_version, arguments_json, arguments_sha256) =
            match validation.as_ref() {
                Some(Ok(arguments)) => (
                    definition
                        .expect("definition exists")
                        .implementation_version()
                        .clone(),
                    definition.expect("definition exists").schema_version(),
                    arguments.canonical_json().to_owned(),
                    arguments.sha256(),
                ),
                Some(Err(_)) => {
                    rejected_arguments_identity(definition.expect("definition exists"), false)
                }
                None => unresolved_arguments_identity(),
            };
        tracing::Span::current().record("tool_version", tool_version.as_str());
        tracing::Span::current().record("tool_schema_version", schema_version.get());
        tracing::Span::current().record(
            "arguments_sha256",
            tracing::field::display(arguments_sha256),
        );
        let tool_execution_id = ToolExecutionId::generate();
        let execution_id = ExecutionId::generate();
        tracing::Span::current().record(
            "tool_execution_id",
            tracing::field::display(tool_execution_id),
        );
        tracing::Span::current().record(
            "workstation_execution_id",
            tracing::field::display(execution_id),
        );
        let requested_cwd = match validation.as_ref().and_then(|value| value.as_ref().ok()) {
            Some(arguments) => requested_cwd(arguments, call.workspace.logical_root()),
            None => call.workspace.logical_root().clone(),
        };
        let requested_privilege = validation
            .as_ref()
            .and_then(|value| value.as_ref().ok())
            .map_or(PrivilegeMode::User, |arguments| {
                arguments.input().requested_privilege()
            });
        tracing::Span::current().record(
            "requested_privilege",
            tracing::field::debug(requested_privilege),
        );
        let requested_timeout_ms = validation
            .as_ref()
            .and_then(|value| value.as_ref().ok())
            .and_then(|arguments| arguments.input().requested_timeout_ms())
            .unwrap_or(self.limits.run_shell_default_timeout_ms)
            .min(self.limits.run_shell_max_timeout_ms);
        let (policy_timeout_ms, effective_deadline) = freeze_tool_deadline(
            requested_timeout_ms,
            definition,
            self.limits.run_shell_max_timeout_ms,
            invocation_start,
            effective_outer_deadline(call.work_deadline, call.shutdown_deadline),
        )?;
        let requested_at = self.wall_now()?;
        let lifecycle = ToolLifecycleReference::new(
            tool_execution_id,
            execution_id,
            call.work.work_id(),
            call.runtime_instance_id,
            call.source_model_invocation_id,
            call.agent_step_no,
            call.tool_ordinal,
        );
        let request_event_id = JournalEventId::generate();
        let waiting = decide_work_transition(
            &call.work,
            WorkTransitionGuard::for_snapshot(&call.work),
            WorkTransitionRequest::WaitForTool { tool_execution_id },
        )
        .map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext)
        })?
        .into_next();
        let output_policy = self.output_policy();
        self.state_store
            .request_tool_execution(RequestToolExecutionRequest {
                expected_work: WorkExpectation::for_snapshot(&call.work),
                tool: PreparedToolExecution {
                    lifecycle,
                    provider_tool_call_id: call.provider_tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    tool_version: tool_version.clone(),
                    tool_schema_version: schema_version.get(),
                    arguments_json,
                    arguments_sha256,
                    workstation_id: call.workstation_id,
                    workstation_generation: call.workstation_generation,
                    workspace_id: call.workspace.workspace_id(),
                    requested_cwd: requested_cwd.clone(),
                    requested_privilege,
                    timeout_ms: requested_timeout_ms,
                    output_policy,
                    requested_at,
                },
                work_next: waiting,
                tool_event: EventIntent {
                    event_id: request_event_id,
                    correlation_id: call.correlation_id,
                    causation_event_id: Some(call.source_model_event_id),
                },
                work_event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id: call.correlation_id,
                    causation_event_id: Some(request_event_id),
                },
            })
            .await?;
        #[cfg(feature = "test-failpoints")]
        crate::test_failpoints::reach(
            crate::test_failpoints::PhysicalHook::AfterToolRequestedCommit,
        );

        let waiting = waiting_snapshot(
            call.work.work_id(),
            call.runtime_instance_id,
            call.work.projection_version(),
            tool_execution_id,
        )?;
        if let Some(result_class) = rejected {
            return self
                .finish_predispatch_result(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    result_class,
                    None,
                    request_event_id,
                )
                .await;
        }
        let definition = definition.expect("validated known tool");
        let arguments = validation
            .expect("known validation")
            .expect("rejection handled");

        if self.clock.monotonic_now() >= effective_deadline {
            return self
                .finish_predispatch_workstation_error(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    WorkstationError::new(WorkstationErrorKind::Timeout),
                    request_event_id,
                )
                .await;
        }

        if let Some(notice) = current_cancellation(&call.cancellation) {
            return self
                .finish_cancelled_before_dispatch(
                    &call,
                    cancellation_snapshot(notice, tool_execution_id)?,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    request_event_id,
                )
                .await;
        }

        let capabilities_operation_id = OperationId::generate();
        let capabilities = match self
            .workstation
            .capabilities(CapabilitiesRequest {
                operation_id: capabilities_operation_id,
                workstation_id: call.workstation_id,
                expected_generation: call.workstation_generation,
            })
            .await
        {
            Ok(result) if result.operation_id == capabilities_operation_id => result.capabilities,
            Ok(_) => {
                return self
                    .finish_predispatch_workstation_error(
                        &call,
                        waiting,
                        tool_execution_id,
                        execution_id,
                        tool_name,
                        tool_version,
                        schema_version,
                        WorkstationError::new(WorkstationErrorKind::InternalWorkstationError),
                        request_event_id,
                    )
                    .await;
            }
            Err(error) => {
                return self
                    .finish_predispatch_workstation_error(
                        &call,
                        waiting,
                        tool_execution_id,
                        execution_id,
                        tool_name,
                        tool_version,
                        schema_version,
                        error,
                        request_event_id,
                    )
                    .await;
            }
        };
        let effective_timeout_ms = capability_bounded_timeout(
            policy_timeout_ms,
            definition,
            capabilities.limits().max_execution_timeout_ms(),
        );
        if effective_timeout_ms == 0 || self.clock.monotonic_now() >= effective_deadline {
            return self
                .finish_predispatch_workstation_error(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    WorkstationError::new(WorkstationErrorKind::Timeout),
                    request_event_id,
                )
                .await;
        }
        let effective_output_policy = ToolOutputPolicy {
            stdout_capture_limit: count(
                self.limits
                    .stdout_capture_bytes
                    .min(capabilities.limits().max_stdout_bytes()),
            )?,
            stderr_capture_limit: count(
                self.limits
                    .stderr_capture_bytes
                    .min(capabilities.limits().max_stderr_bytes()),
            )?,
            combined_inline_limit: count(self.limits.inline_model_result_bytes)?,
            per_stream_inline_limit: count(self.limits.per_stream_projection_bytes)?,
        };
        let authority = self.authority.evaluate(AuthorityEvaluationInput {
            craxii_id: call.craxii_id,
            work_id: call.work.work_id(),
            runtime_instance_id: call.runtime_instance_id,
            expected_workstation_id: call.workstation_id,
            expected_generation: call.workstation_generation,
            expected_workspace_id: call.workspace.workspace_id(),
            workspace_id: call.workspace.workspace_id(),
            definition: Some(definition),
            requested_tool_name: &tool_name,
            arguments_sha256: arguments.sha256(),
            canonical_argument_bytes: arguments.canonical_json().len(),
            requested_privilege,
            requested_timeout_ms: Some(effective_timeout_ms),
            requested_stdout_bytes: effective_output_policy.stdout_capture_limit.get(),
            requested_stderr_bytes: effective_output_policy.stderr_capture_limit.get(),
            work_cancelled: current_cancellation(&call.cancellation).is_some(),
            malformed_arguments: false,
            authority_widening_attempt: false,
            constraints: call.authority_constraints,
            capabilities: &capabilities,
        });
        tracing::Span::current().record(
            "authority_result",
            if authority.allowed() {
                "allowed"
            } else {
                "denied"
            },
        );
        tracing::Span::current().record(
            "effective_privilege",
            tracing::field::debug(authority.snapshot().effective_privilege()),
        );
        tracing::Span::current().record("effective_timeout_ms", effective_timeout_ms);
        if !authority.allowed() {
            return self
                .finish_predispatch_result(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    ToolResultClass::AuthorityDenial,
                    Some(authority.snapshot().clone()),
                    request_event_id,
                )
                .await;
        }
        if let Some(notice) = current_cancellation(&call.cancellation) {
            return self
                .finish_cancelled_before_dispatch(
                    &call,
                    cancellation_snapshot(notice, tool_execution_id)?,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    request_event_id,
                )
                .await;
        }

        let preparation_operation_id = OperationId::generate();
        let prepared = match self
            .preparation
            .prepare(WorkstationPreparationRequest {
                operation_id: preparation_operation_id,
                workstation_id: call.workstation_id,
                expected_generation: call.workstation_generation,
                workspace_id: call.workspace.workspace_id(),
                requested_cwd: requested_cwd.clone(),
                required_capability: definition.required_capability(),
                effective_privilege: authority.snapshot().effective_privilege(),
            })
            .await
        {
            Ok(prepared)
                if prepared.operation_id == preparation_operation_id
                    && prepared.prepared_cwd.resolved_cwd().workstation_id()
                        == call.workstation_id
                    && prepared
                        .prepared_cwd
                        .resolved_cwd()
                        .workstation_generation()
                        == call.workstation_generation
                    && prepared.prepared_cwd.resolved_cwd().workspace_id()
                        == call.workspace.workspace_id()
                    && prepared.prepared_cwd.resolved_cwd().requested_path() == &requested_cwd =>
            {
                prepared
            }
            Ok(_) => {
                return self
                    .finish_predispatch_workstation_error(
                        &call,
                        waiting,
                        tool_execution_id,
                        execution_id,
                        tool_name,
                        tool_version,
                        schema_version,
                        WorkstationError::new(WorkstationErrorKind::InternalWorkstationError),
                        request_event_id,
                    )
                    .await;
            }
            Err(error) => {
                return self
                    .finish_predispatch_workstation_error(
                        &call,
                        waiting,
                        tool_execution_id,
                        execution_id,
                        tool_name,
                        tool_version,
                        schema_version,
                        error,
                        request_event_id,
                    )
                    .await;
            }
        };
        if let Some(notice) = current_cancellation(&call.cancellation) {
            return self
                .finish_cancelled_before_dispatch(
                    &call,
                    cancellation_snapshot(notice, tool_execution_id)?,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    request_event_id,
                )
                .await;
        }
        if self.clock.monotonic_now() >= effective_deadline {
            return self
                .finish_predispatch_workstation_error(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    WorkstationError::new(WorkstationErrorKind::Timeout),
                    request_event_id,
                )
                .await;
        }

        let dispatch_at = self.wall_now()?;
        let dispatch_event_id = JournalEventId::generate();
        let dispatch_evidence_json =
            compose_dispatch_evidence(authority.evidence_json(), &prepared.prepared_cwd)?;
        self.state_store
            .commit_tool_dispatch_intent(CommitToolDispatchIntentRequest {
                expected_work: WorkExpectation::for_snapshot(&waiting),
                expected_tool: ToolExpectation {
                    tool_execution_id,
                    state: ToolExecutionState::Requested,
                },
                dispatch: ToolDispatchIntent {
                    authority: authority.snapshot().clone(),
                    dispatch_evidence_json,
                    effective_privilege: authority.snapshot().effective_privilege(),
                    prepared_cwd: prepared.prepared_cwd.clone(),
                    timeout_ms: effective_timeout_ms,
                    output_policy: effective_output_policy,
                    dispatch_intent_at: dispatch_at,
                },
                event: EventIntent {
                    event_id: dispatch_event_id,
                    correlation_id: call.correlation_id,
                    causation_event_id: Some(request_event_id),
                },
            })
            .await?;
        tracing::Span::current().record("dispatch_intent_persisted", true);
        #[cfg(feature = "test-failpoints")]
        crate::test_failpoints::reach(
            crate::test_failpoints::PhysicalHook::AfterToolDispatchIntentCommit,
        );

        if let Some(notice) = current_cancellation(&call.cancellation) {
            return self
                .finish_cancelled_after_dispatch_before_handoff(
                    &call,
                    cancellation_snapshot(notice, tool_execution_id)?,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    dispatch_event_id,
                    dispatch_at,
                )
                .await;
        }
        if self.clock.monotonic_now() >= effective_deadline {
            return self
                .finish_dispatched_workstation_error(
                    &call,
                    waiting,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    authority.snapshot().effective_privilege(),
                    dispatch_event_id,
                    dispatch_at,
                    WorkstationError::new(WorkstationErrorKind::Timeout),
                )
                .await;
        }

        let expected_invocation_cwd = requested_cwd.clone();
        let expected_read_path = match arguments.input() {
            ValidatedToolInput::ReadFile(input) => Some(input.path().clone()),
            ValidatedToolInput::RunShell(_) => None,
        };
        let expected_command_sha256 = match arguments.input() {
            ValidatedToolInput::RunShell(input) => {
                Some(Sha256Digest::hash_bytes(input.command().as_bytes()))
            }
            ValidatedToolInput::ReadFile(_) => None,
        };
        let handler_operation_id = OperationId::generate();
        let handler_context = ToolExecutionContext {
            operation_id: handler_operation_id,
            execution_id,
            work_id: call.work.work_id(),
            workstation_id: call.workstation_id,
            workstation_generation: call.workstation_generation,
            workspace_id: call.workspace.workspace_id(),
            requested_cwd,
            prepared_cwd: prepared.prepared_cwd.clone(),
            effective_privilege: authority.snapshot().effective_privilege(),
            timeout: MonotonicDuration::from_millis(effective_timeout_ms),
            deadline: effective_deadline,
            capture: ExecutionCapturePolicy {
                stdout_max_bytes: effective_output_policy.stdout_capture_limit.get(),
                stderr_max_bytes: effective_output_policy.stderr_capture_limit.get(),
            },
        };
        let handler = resolve_handler(definition.handler());
        let handler_future = guarded_handler_handoff(|| {
            handler.invoke(
                arguments.input(),
                handler_context,
                self.workstation.as_ref(),
            )
        });
        let handler_future = match handler_future {
            Ok(future) => future,
            Err(_) => {
                return self
                    .finish_postdispatch_definite_internal_rejection(
                        &call,
                        waiting,
                        tool_execution_id,
                        execution_id,
                        tool_name,
                        tool_version,
                        schema_version,
                        dispatch_event_id,
                        dispatch_at,
                    )
                    .await;
            }
        };
        let operation_started = self.clock.monotonic_now();
        let (observation, cancellation_notice, cancellation_unconfirmed) = self
            .await_handler(
                handler_future,
                &mut call.cancellation,
                matches!(arguments.input(), ValidatedToolInput::RunShell(_)),
                execution_id,
                call.workstation_id,
                call.workstation_generation,
            )
            .await;
        let operation_duration = self
            .clock
            .monotonic_now()
            .checked_duration_since(operation_started)
            .unwrap_or_default();
        let expected_work = cancellation_notice
            .map(|notice| cancellation_snapshot(notice, tool_execution_id))
            .transpose()?
            .unwrap_or(waiting);
        if matches!(
            &observation,
            Err(HandlerPollFailure::CancelledBeforeHandoff)
        ) {
            return self
                .finish_cancelled_after_dispatch_before_handoff(
                    &call,
                    expected_work,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    dispatch_event_id,
                    dispatch_at,
                )
                .await;
        }
        if observation
            .as_ref()
            .ok()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(|value| {
                !observation_identity_matches(value, handler_operation_id, execution_id)
            })
        {
            self.persist_outcome_unknown(
                &call,
                expected_work,
                tool_execution_id,
                dispatch_event_id,
                dispatch_at,
                NormalizedError::workstation(Certainty::OutcomeUnknown, None),
            )
            .await?;
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::OutcomeUnknown,
            ));
        }
        match observation {
            Err(HandlerPollFailure::Panicked) => {
                self.persist_outcome_unknown(
                    &call,
                    expected_work,
                    tool_execution_id,
                    dispatch_event_id,
                    dispatch_at,
                    NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                )
                .await?;
                Err(ToolExecutionServiceError::new(
                    ToolExecutionServiceErrorKind::HandlerPanickedAfterPossibleHandoff,
                ))
            }
            Err(HandlerPollFailure::CancelledBeforeHandoff) => {
                unreachable!("pre-handoff cancellation returned above")
            }
            Ok(Err(error))
                if error.certainty() == Certainty::OutcomeUnknown || cancellation_unconfirmed =>
            {
                self.persist_outcome_unknown(
                    &call,
                    expected_work,
                    tool_execution_id,
                    dispatch_event_id,
                    dispatch_at,
                    NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                )
                .await?;
                Err(ToolExecutionServiceError::new(
                    ToolExecutionServiceErrorKind::OutcomeUnknown,
                ))
            }
            Ok(Err(error)) => {
                self.finish_dispatched_workstation_error(
                    &call,
                    expected_work,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    authority.snapshot().effective_privilege(),
                    dispatch_event_id,
                    dispatch_at,
                    error,
                )
                .await
            }
            Ok(Ok(ToolHandlerObservation::ReadFile(result))) => {
                self.finish_read_file(
                    &call,
                    expected_work,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    dispatch_event_id,
                    dispatch_at,
                    *result,
                    operation_duration,
                    expected_read_path.expect("read handler has a read path"),
                )
                .await
            }
            Ok(Ok(ToolHandlerObservation::RunShell(result))) => {
                self.finish_run_shell(
                    &call,
                    expected_work,
                    tool_execution_id,
                    execution_id,
                    tool_name,
                    tool_version,
                    schema_version,
                    dispatch_event_id,
                    dispatch_at,
                    *result,
                    &expected_invocation_cwd,
                    prepared.prepared_cwd.resolved_cwd(),
                    authority.snapshot().effective_privilege(),
                    expected_command_sha256.expect("shell handler has a command hash"),
                )
                .await
            }
        }
    }

    fn output_policy(&self) -> ToolOutputPolicy {
        ToolOutputPolicy {
            stdout_capture_limit: count(self.limits.stdout_capture_bytes)
                .expect("validated capture limit"),
            stderr_capture_limit: count(self.limits.stderr_capture_bytes)
                .expect("validated capture limit"),
            combined_inline_limit: count(self.limits.inline_model_result_bytes)
                .expect("validated inline limit"),
            per_stream_inline_limit: count(self.limits.per_stream_projection_bytes)
                .expect("validated projection limit"),
        }
    }

    fn wall_now(&self) -> Result<UtcTimestamp, ToolExecutionServiceError> {
        self.clock
            .utc_now()
            .map_err(|_| ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Clock))
            .and_then(|value| {
                UtcTimestamp::from_offset_datetime(value).map_err(|_| {
                    ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Clock)
                })
            })
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_predispatch_result(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        result_class: ToolResultClass,
        authority: Option<AuthorityDecisionSnapshot>,
        causation: JournalEventId,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let summary = match result_class {
            ToolResultClass::UnknownTool => "Unknown tool.",
            ToolResultClass::ValidationRejection => "Tool arguments were rejected.",
            ToolResultClass::AuthorityDenial => "Tool authority was denied.",
            _ => "Tool request was rejected before dispatch.",
        };
        let error = match result_class {
            ToolResultClass::AuthorityDenial => NormalizedError::authority(),
            _ => NormalizedError::tool_validation(),
        };
        let result = ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class,
            effective_privilege: None,
            summary: summary.to_owned(),
            fields: Vec::new(),
            artifact_ids: Vec::new(),
            truncated: false,
            error: Some(ToolResultError {
                code: error.code().as_str().to_owned(),
                certainty: error.certainty().as_str(),
            }),
        };
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Requested,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::Completed,
                        predispatch_authority: authority,
                        started_at: None,
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: None,
                        cancelled: None,
                        cleanup_confirmed: None,
                        result: Some(ToolResultEvidence {
                            result_kind: result_class,
                            summary: result.summary.clone(),
                            fields: Vec::new(),
                        }),
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(error),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_predispatch_workstation_error(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        error: WorkstationError,
        causation: JournalEventId,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let code = error.kind().code().to_owned();
        let model_error = ToolResultError {
            code,
            certainty: error.certainty().as_str(),
        };
        let result = ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class: ToolResultClass::FileError,
            effective_privilege: None,
            summary: "Workstation preparation failed before dispatch.".to_owned(),
            fields: Vec::new(),
            artifact_ids: Vec::new(),
            truncated: false,
            error: Some(model_error),
        };
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Requested,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::Completed,
                        predispatch_authority: None,
                        started_at: None,
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: None,
                        cancelled: None,
                        cleanup_confirmed: None,
                        result: Some(ToolResultEvidence {
                            result_kind: ToolResultClass::FileError,
                            summary: result.summary.clone(),
                            fields: Vec::new(),
                        }),
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(error.normalized()),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_cancelled_before_dispatch(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        self.finish_cancellation(
            call,
            expected_work,
            ToolExecutionState::Requested,
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            causation,
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_cancelled_after_dispatch_before_handoff(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
        _dispatch_at: UtcTimestamp,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        self.finish_cancellation(
            call,
            expected_work,
            ToolExecutionState::Dispatching,
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            causation,
            None,
            Some(true),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_cancellation(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_state: ToolExecutionState,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
        started_at: Option<UtcTimestamp>,
        cleanup_confirmed: Option<bool>,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let result = ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class: ToolResultClass::Cancellation,
            effective_privilege: (tool_state == ToolExecutionState::Dispatching)
                .then_some(PrivilegeMode::User),
            summary: "Tool execution was cancelled before machine handoff.".to_owned(),
            fields: Vec::new(),
            artifact_ids: Vec::new(),
            truncated: false,
            error: Some(ToolResultError {
                code: "cancellation".to_owned(),
                certainty: "definite",
            }),
        };
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: tool_state,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::Completed,
                        predispatch_authority: None,
                        started_at,
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: (tool_state == ToolExecutionState::Dispatching).then_some(false),
                        cancelled: (tool_state == ToolExecutionState::Dispatching).then_some(true),
                        cleanup_confirmed,
                        result: Some(ToolResultEvidence {
                            result_kind: ToolResultClass::Cancellation,
                            summary: result.summary.clone(),
                            fields: Vec::new(),
                        }),
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(NormalizedError::cancellation(Certainty::Definite)),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_postdispatch_definite_internal_rejection(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
        _dispatch_at: UtcTimestamp,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let completed_at = self.wall_now()?;
        let summary = "Handler rejected the call before Workstation handoff.".to_owned();
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Dispatching,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::Completed,
                        predispatch_authority: None,
                        started_at: None,
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: Some(false),
                        cancelled: Some(false),
                        cleanup_confirmed: None,
                        result: Some(ToolResultEvidence {
                            result_kind: ToolResultClass::ValidationRejection,
                            summary: summary.clone(),
                            fields: Vec::new(),
                        }),
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(NormalizedError::internal_invariant()),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class: ToolResultClass::ValidationRejection,
            effective_privilege: Some(PrivilegeMode::User),
            summary,
            fields: Vec::new(),
            artifact_ids: Vec::new(),
            truncated: false,
            error: Some(ToolResultError {
                code: "internal_invariant".to_owned(),
                certainty: "definite",
            }),
        })
    }

    async fn await_handler(
        &self,
        future: ToolHandlerFuture<'_>,
        cancellation: &mut Option<tokio::sync::watch::Receiver<Option<ToolCancellationNotice>>>,
        cancellable_execution: bool,
        execution_id: ExecutionId,
        workstation_id: WorkstationId,
        generation: WorkstationGeneration,
    ) -> (
        Result<Result<ToolHandlerObservation, WorkstationError>, HandlerPollFailure>,
        Option<ToolCancellationNotice>,
        bool,
    ) {
        let handoff_started = Arc::new(AtomicBool::new(false));
        let mut future = CatchHandlerPanic {
            inner: future,
            handoff_started: Arc::clone(&handoff_started),
        };
        let mut notice = current_cancellation(cancellation);
        let mut cancellation_unconfirmed = false;
        let mut cancellation_sent = false;
        loop {
            if notice.is_some() && !handoff_started.load(Ordering::Acquire) {
                return (
                    Err(HandlerPollFailure::CancelledBeforeHandoff),
                    notice,
                    false,
                );
            }
            if notice.is_some() && cancellable_execution && !cancellation_sent {
                cancellation_sent = true;
                let cancellation_result = self
                    .workstation
                    .cancel_execution(ExecutionCancellationRequest {
                        operation_id: OperationId::generate(),
                        execution_id,
                        workstation_id,
                        expected_generation: generation,
                    })
                    .await;
                cancellation_unconfirmed = match cancellation_result {
                    Ok(result) => {
                        matches!(result.state, ExecutionCancellationState::CleanupUnconfirmed)
                    }
                    Err(_) => true,
                };
            }
            let Some(receiver) = cancellation.as_mut() else {
                return (future.await, notice, cancellation_unconfirmed);
            };
            tokio::select! {
                biased;
                changed = receiver.changed(), if notice.is_none() => {
                    if changed.is_err() {
                        return (future.await, notice, cancellation_unconfirmed);
                    }
                    notice = *receiver.borrow();
                }
                result = &mut future => return (result, notice, cancellation_unconfirmed),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_dispatched_workstation_error(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        effective_privilege: PrivilegeMode,
        causation: JournalEventId,
        _dispatch_at: UtcTimestamp,
        error: WorkstationError,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        let result_class = match error.kind() {
            WorkstationErrorKind::Timeout => ToolResultClass::Timeout,
            WorkstationErrorKind::Cancelled => ToolResultClass::Cancellation,
            WorkstationErrorKind::SpawnFailed => ToolResultClass::SpawnFailure,
            _ => ToolResultClass::FileError,
        };
        let cleanup_confirmed = matches!(
            result_class,
            ToolResultClass::Timeout | ToolResultClass::Cancellation
        )
        .then_some(true);
        let completed_at = self.wall_now()?;
        let summary = "The workstation returned a definite error.".to_owned();
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Dispatching,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::Completed,
                        predispatch_authority: None,
                        started_at: None,
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: Some(result_class == ToolResultClass::Timeout),
                        cancelled: Some(result_class == ToolResultClass::Cancellation),
                        cleanup_confirmed,
                        result: Some(ToolResultEvidence {
                            result_kind: result_class,
                            summary: summary.clone(),
                            fields: Vec::new(),
                        }),
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(error.normalized()),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class,
            effective_privilege: Some(effective_privilege),
            summary,
            fields: Vec::new(),
            artifact_ids: Vec::new(),
            truncated: false,
            error: Some(ToolResultError {
                code: error.kind().code().to_owned(),
                certainty: error.certainty().as_str(),
            }),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_read_file(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
        dispatch_at: UtcTimestamp,
        result: crate::ports::workstation::FileReadResult,
        duration: Duration,
        expected_path: LogicalPathReference,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        tracing::Span::current().record("output_observed_bytes", result.byte_length.get());
        let byte_length_matches = u64::try_from(result.text.len())
            .ok()
            .is_some_and(|length| length == result.byte_length.get());
        if result.truncated
            || !byte_length_matches
            || result.sha256 != Sha256Digest::hash_bytes(result.text.as_bytes())
            || result.requested_path != expected_path
            || result.resolved_path.requested_path() != &expected_path
            || result.resolved_path.workstation_id() != call.workstation_id
            || result.resolved_path.workstation_generation() != call.workstation_generation
            || result.resolved_path.workspace_id() != call.workspace.workspace_id()
        {
            self.persist_outcome_unknown(
                call,
                expected_work,
                tool_execution_id,
                causation,
                dispatch_at,
                NormalizedError::workstation(Certainty::OutcomeUnknown, None),
            )
            .await?;
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::OutcomeUnknown,
            ));
        }
        let projection_limit = usize::try_from(
            self.limits
                .per_stream_projection_bytes
                .min(READ_FILE_PROJECTION_BYTES as u64)
                .min(self.limits.inline_model_result_bytes),
        )
        .map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
        })?;
        let (projection, projection_omitted) = truncate_utf8(&result.text, projection_limit);
        let mut artifacts = Vec::new();
        let mut artifact_ids = Vec::new();
        if projection_omitted > 0 {
            let artifact_id = ArtifactId::generate();
            let artifact = (|| {
                let mut capture = self
                    .artifact_store
                    .begin_capture(BeginArtifactCapture {
                        artifact_id,
                        hard_capture_limit: result.byte_length,
                    })
                    .map_err(|_| {
                        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Artifact)
                    })?;
                capture.write_chunk(result.text.as_bytes()).map_err(|_| {
                    ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Artifact)
                })?;
                let finalized = capture.finalize().map_err(|_| {
                    ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Artifact)
                })?;
                prepared_artifact(
                    call,
                    tool_execution_id,
                    finalized,
                    "text/plain",
                    Some("utf-8"),
                    "read-file.txt",
                    self.wall_now()?,
                )
            })();
            let artifact = match artifact {
                Ok(artifact) => artifact,
                Err(error) => {
                    self.persist_outcome_unknown(
                        call,
                        expected_work,
                        tool_execution_id,
                        causation,
                        dispatch_at,
                        NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                    )
                    .await?;
                    return Err(error);
                }
            };
            artifacts.push(artifact);
            artifact_ids.push(artifact_id);
        }
        let mut fields = BTreeMap::new();
        fields.insert(
            "byte_length".to_owned(),
            result.byte_length.get().to_string(),
        );
        fields.insert("duration_ms".to_owned(), duration.as_millis().to_string());
        fields.insert(
            "projection_omitted_bytes".to_owned(),
            projection_omitted.to_string(),
        );
        fields.insert(
            "requested_path".to_owned(),
            result.requested_path.canonical().to_owned(),
        );
        fields.insert(
            "resolved_path".to_owned(),
            result.resolved_path.resolved_absolute_path().to_owned(),
        );
        fields.insert("sha256".to_owned(), result.sha256.to_string());
        if let Some(artifact_id) = artifact_ids.first() {
            fields.insert("artifact_id".to_owned(), artifact_id.to_string());
        }
        insert_projection_chunks(&mut fields, "text", &projection);
        let fields: Vec<_> = fields.into_iter().collect();
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        let outcome = ToolTerminalOutcome {
            state: ToolExecutionState::Completed,
            predispatch_authority: None,
            started_at: Some(dispatch_at),
            completed_at,
            exit_code: None,
            signal: None,
            timed_out: Some(false),
            cancelled: Some(false),
            cleanup_confirmed: None,
            result: Some(ToolResultEvidence {
                result_kind: ToolResultClass::Success,
                summary: "File read completed.".to_owned(),
                fields: fields.clone(),
            }),
            evidence_artifact_ids: artifact_ids.clone(),
            stdout_artifact_id: None,
            stderr_artifact_id: None,
            stdout_counts: None,
            stderr_counts: None,
            truncated: false,
            normalized_error: None,
        };
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Dispatching,
                    },
                    outcome,
                    artifacts,
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class: ToolResultClass::Success,
            effective_privilege: Some(PrivilegeMode::User),
            summary: "File read completed.".to_owned(),
            fields,
            artifact_ids,
            truncated: projection_omitted > 0,
            error: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_run_shell(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        tool_name: ToolName,
        tool_version: ToolVersion,
        schema_version: SchemaVersion,
        causation: JournalEventId,
        dispatch_at: UtcTimestamp,
        result: ExecutionResult,
        expected_cwd: &LogicalPathReference,
        expected_resolved_cwd: &crate::domain::ResolvedPathEvidence,
        expected_privilege: PrivilegeMode,
        expected_command_sha256: Sha256Digest,
    ) -> Result<ToolExecutionResult, ToolExecutionServiceError> {
        if let Some(value) = result.exit_code {
            tracing::Span::current().record("exit_code", value);
        }
        if let Some(value) = result.terminating_signal {
            tracing::Span::current().record("signal", value);
        }
        tracing::Span::current().record("timed_out", result.timed_out);
        tracing::Span::current().record("cancelled", result.cancelled);
        tracing::Span::current().record(
            "cleanup_result",
            if result.cleanup.confirmed() {
                "confirmed"
            } else {
                "unconfirmed"
            },
        );
        if result.certainty == Certainty::OutcomeUnknown
            || result.result_kind == ExecutionResultKind::CleanupFailed
            || !result.cleanup.confirmed()
            || &result.requested_cwd != expected_cwd
            || result.resolved_cwd.requested_path() != expected_cwd
            || result.resolved_cwd.workstation_id() != call.workstation_id
            || result.resolved_cwd.workstation_generation() != call.workstation_generation
            || result.resolved_cwd.workspace_id() != call.workspace.workspace_id()
            || &result.resolved_cwd != expected_resolved_cwd
            || result.effective_privilege != expected_privilege
            || result.command_sha256 != expected_command_sha256
        {
            self.persist_outcome_unknown(
                call,
                expected_work,
                tool_execution_id,
                causation,
                dispatch_at,
                NormalizedError::workstation(Certainty::OutcomeUnknown, None),
            )
            .await?;
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::OutcomeUnknown,
            ));
        }
        if result.execution_id != execution_id {
            self.persist_outcome_unknown(
                call,
                expected_work,
                tool_execution_id,
                causation,
                dispatch_at,
                NormalizedError::internal_invariant(),
            )
            .await?;
            return Err(ToolExecutionServiceError::new(
                ToolExecutionServiceErrorKind::OutcomeUnknown,
            ));
        }
        let result_class = execution_result_class(&result);
        #[cfg(feature = "test-failpoints")]
        crate::test_failpoints::reach(
            crate::test_failpoints::PhysicalHook::AfterToolProcessExitBeforeOutcomeCommit,
        );
        let summary = execution_summary(result_class).to_owned();
        let mut artifacts = Vec::new();
        let mut artifact_ids = Vec::new();
        let stdout_evidence = stream_evidence(
            call,
            tool_execution_id,
            result.stdout,
            "stdout.bin",
            self.limits.per_stream_projection_bytes,
            self.limits.inline_model_result_bytes,
            self.wall_now()?,
            &mut artifacts,
            &mut artifact_ids,
        );
        let (stdout_artifact_id, mut stdout_counts, stdout_projection, _) = match stdout_evidence {
            Ok(evidence) => evidence,
            Err(error) => {
                self.persist_outcome_unknown(
                    call,
                    expected_work,
                    tool_execution_id,
                    causation,
                    dispatch_at,
                    NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                )
                .await?;
                return Err(error);
            }
        };
        let remaining_inline = self
            .limits
            .inline_model_result_bytes
            .saturating_sub(stdout_projection.len() as u64);
        let stderr_evidence = stream_evidence(
            call,
            tool_execution_id,
            result.stderr,
            "stderr.bin",
            self.limits.per_stream_projection_bytes,
            remaining_inline,
            self.wall_now()?,
            &mut artifacts,
            &mut artifact_ids,
        );
        let (stderr_artifact_id, mut stderr_counts, stderr_projection, _) = match stderr_evidence {
            Ok(evidence) => evidence,
            Err(error) => {
                self.persist_outcome_unknown(
                    call,
                    expected_work,
                    tool_execution_id,
                    causation,
                    dispatch_at,
                    NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                )
                .await?;
                return Err(error);
            }
        };
        if let Some(counts) = stdout_counts {
            tracing::Span::current().record("stdout_observed_bytes", counts.observed.get());
            tracing::Span::current().record("stdout_captured_bytes", counts.captured.get());
        }
        if let Some(counts) = stderr_counts {
            tracing::Span::current().record("stderr_observed_bytes", counts.observed.get());
            tracing::Span::current().record("stderr_captured_bytes", counts.captured.get());
        }
        let mut fields = BTreeMap::new();
        fields.insert(
            "command_sha256".to_owned(),
            result.command_sha256.to_string(),
        );
        fields.insert(
            "duration_ms".to_owned(),
            result.duration.as_duration().as_millis().to_string(),
        );
        if let Some(exit_code) = result.exit_code {
            fields.insert("exit_code".to_owned(), exit_code.to_string());
        }
        if let Some(signal) = result.terminating_signal {
            fields.insert("signal".to_owned(), signal.to_string());
        }
        let bounded = bounded_shell_result_fields(
            fields,
            &stdout_projection,
            &stderr_projection,
            result_class,
            &summary,
        )?;
        let fields = bounded.fields;
        update_stream_projection_counts(&mut stdout_counts, bounded.stdout_returned)?;
        update_stream_projection_counts(&mut stderr_counts, bounded.stderr_returned)?;
        let normalized_error = result.error.as_ref().map(WorkstationError::normalized);
        let model_error = result.error.as_ref().map(|error| ToolResultError {
            code: error.kind().code().to_owned(),
            certainty: error.certainty().as_str(),
        });
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(&expected_work, tool_execution_id, None)?;
        let outcome = ToolTerminalOutcome {
            state: ToolExecutionState::Completed,
            predispatch_authority: None,
            started_at: result.start_observed.then_some(dispatch_at),
            completed_at,
            exit_code: result.exit_code,
            signal: result.terminating_signal,
            timed_out: Some(result.timed_out),
            cancelled: Some(result.cancelled),
            cleanup_confirmed: Some(true),
            result: Some(ToolResultEvidence {
                result_kind: result_class,
                summary: summary.clone(),
                fields: fields.clone(),
            }),
            evidence_artifact_ids: Vec::new(),
            stdout_artifact_id,
            stderr_artifact_id,
            stdout_counts,
            stderr_counts,
            truncated: stdout_counts.is_some_and(|counts| counts.observed > counts.captured)
                || stderr_counts.is_some_and(|counts| counts.observed > counts.captured),
            normalized_error,
        };
        let truncated = outcome.truncated
            || stdout_counts.is_some_and(|counts| counts.omitted.get() > 0)
            || stderr_counts.is_some_and(|counts| counts.omitted.get() > 0);
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Dispatching,
                    },
                    outcome,
                    artifacts,
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(ToolExecutionResult {
            tool_execution_id,
            execution_id,
            tool_name,
            tool_version,
            schema_version,
            status: ToolResultStatus::Completed,
            result_class,
            effective_privilege: Some(result.effective_privilege),
            summary,
            fields,
            artifact_ids,
            truncated,
            error: model_error,
        })
    }

    async fn persist_outcome_unknown(
        &self,
        call: &ToolExecutionCall,
        expected_work: WorkLifecycleSnapshot,
        tool_execution_id: ToolExecutionId,
        causation: JournalEventId,
        dispatch_at: UtcTimestamp,
        error: NormalizedError,
    ) -> Result<(), ToolExecutionServiceError> {
        tracing::Span::current().record("outcome_unknown", true);
        tracing::info!(
            event_name = "tool_outcome_unknown_observed",
            work_id = %call.work.work_id(),
            tool_execution_id = %tool_execution_id,
            intent_observed_at = %dispatch_at,
            certainty = "outcome_unknown",
            retryable = false,
            external_side_effect_may_have_occurred = true
        );
        let completed_at = self.wall_now()?;
        let work_next = terminal_work_next(
            &expected_work,
            tool_execution_id,
            Some(WorkInterruptionReason::ToolOutcomeUnknown),
        )?;
        self.state_store
            .finish_tool_execution(terminal_request(
                FinishToolExecutionRequest {
                    expected_work: WorkExpectation::for_snapshot(&expected_work),
                    expected_tool: ToolExpectation {
                        tool_execution_id,
                        state: ToolExecutionState::Dispatching,
                    },
                    outcome: ToolTerminalOutcome {
                        state: ToolExecutionState::OutcomeUnknown,
                        predispatch_authority: None,
                        started_at: Some(dispatch_at),
                        completed_at,
                        exit_code: None,
                        signal: None,
                        timed_out: None,
                        cancelled: None,
                        cleanup_confirmed: Some(false),
                        result: None,
                        evidence_artifact_ids: Vec::new(),
                        stdout_artifact_id: None,
                        stderr_artifact_id: None,
                        stdout_counts: None,
                        stderr_counts: None,
                        truncated: false,
                        normalized_error: Some(error),
                    },
                    artifacts: Vec::new(),
                    work_next,
                    tool_event: event(call, causation),
                    work_event: caused_work_event(call),
                },
                call,
                causation,
            ))
            .await?;
        Ok(())
    }
}

fn validate_call_context(call: &ToolExecutionCall) -> Result<(), ToolExecutionServiceError> {
    if call.work.state() != WorkState::Running
        || call.work.runtime_owner() != Some(call.runtime_instance_id)
        || call.work.current_attempt() != crate::domain::CurrentWorkAttempt::None
        || call.workspace.craxii_id() != call.craxii_id
        || call.workspace.workstation_id() != call.workstation_id
    {
        return Err(ToolExecutionServiceError::new(
            ToolExecutionServiceErrorKind::InvalidCallContext,
        ));
    }
    Ok(())
}

fn rejected_arguments_identity(
    definition: &ToolDefinition,
    unknown: bool,
) -> (ToolVersion, SchemaVersion, String, Sha256Digest) {
    let json = if unknown {
        r#"{"unknown_tool":true}"#
    } else {
        r#"{"validation_rejected":true}"#
    }
    .to_owned();
    (
        definition.implementation_version().clone(),
        definition.schema_version(),
        json.clone(),
        Sha256Digest::hash_bytes(json.as_bytes()),
    )
}

fn unresolved_arguments_identity() -> (ToolVersion, SchemaVersion, String, Sha256Digest) {
    let json = r#"{"unknown_tool":true}"#.to_owned();
    (
        ToolVersion::try_new("unresolved").expect("static unresolved version"),
        SchemaVersion::try_new(1).expect("one is positive"),
        json.clone(),
        Sha256Digest::hash_bytes(json.as_bytes()),
    )
}

fn requested_cwd(
    arguments: &ValidatedToolArguments,
    workspace_default: &LogicalPathReference,
) -> LogicalPathReference {
    match arguments.input() {
        ValidatedToolInput::ReadFile(_) => workspace_default.clone(),
        ValidatedToolInput::RunShell(input) => input
            .requested_cwd()
            .cloned()
            .unwrap_or_else(|| workspace_default.clone()),
    }
}

fn observation_identity_matches(
    observation: &ToolHandlerObservation,
    operation_id: OperationId,
    execution_id: ExecutionId,
) -> bool {
    match observation {
        ToolHandlerObservation::ReadFile(result) => result.operation_id == operation_id,
        ToolHandlerObservation::RunShell(result) => {
            result.operation_id == operation_id && result.execution_id == execution_id
        }
    }
}

fn guarded_handler_handoff<'a>(
    handoff: impl FnOnce() -> ToolHandlerFuture<'a>,
) -> Result<ToolHandlerFuture<'a>, ToolExecutionServiceError> {
    catch_unwind(AssertUnwindSafe(handoff)).map_err(|_| {
        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::HandlerPanickedBeforeHandoff)
    })
}

fn within_runtime_limits(arguments: &ValidatedToolArguments, limits: ToolRuntimeLimits) -> bool {
    match arguments.input() {
        ValidatedToolInput::ReadFile(input) => input.max_bytes() <= limits.read_file_max_bytes,
        ValidatedToolInput::RunShell(input) => {
            input.command().len() as u64 <= limits.run_shell_command_max_bytes
                && input.timeout_ms() <= limits.run_shell_max_timeout_ms
        }
    }
}

fn waiting_snapshot(
    work_id: crate::domain::WorkId,
    runtime_id: RuntimeInstanceId,
    previous: ProjectionVersion,
    tool_execution_id: ToolExecutionId,
) -> Result<WorkLifecycleSnapshot, ToolExecutionServiceError> {
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id,
        state: WorkState::WaitingOnTool,
        projection_version: previous.checked_increment().map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext)
        })?,
        runtime_owner: Some(runtime_id),
        current_attempt: crate::domain::CurrentWorkAttempt::Tool(tool_execution_id),
        cancellation_reason: None,
        terminal_reason: None,
    })
    .map_err(|_| ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext))
}

fn cancellation_snapshot(
    notice: ToolCancellationNotice,
    tool_execution_id: ToolExecutionId,
) -> Result<WorkLifecycleSnapshot, ToolExecutionServiceError> {
    if notice.expected_work.state != WorkState::CancelRequested
        || notice.expected_work.current_attempt
            != crate::domain::CurrentWorkAttempt::Tool(tool_execution_id)
        || notice.expected_work.runtime_owner.is_none()
    {
        return Err(ToolExecutionServiceError::new(
            ToolExecutionServiceErrorKind::InvalidCallContext,
        ));
    }
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id: notice.expected_work.work_id,
        state: WorkState::CancelRequested,
        projection_version: notice.expected_work.version,
        runtime_owner: notice.expected_work.runtime_owner,
        current_attempt: notice.expected_work.current_attempt,
        cancellation_reason: Some(notice.reason),
        terminal_reason: None,
    })
    .map_err(|_| ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext))
}

fn terminal_work_next(
    current: &WorkLifecycleSnapshot,
    tool_execution_id: ToolExecutionId,
    interruption: Option<WorkInterruptionReason>,
) -> Result<WorkLifecycleSnapshot, ToolExecutionServiceError> {
    let request = if let Some(reason) = interruption {
        WorkTransitionRequest::Interrupt { reason }
    } else if current.state() == WorkState::CancelRequested {
        WorkTransitionRequest::Cancel {
            reason: current.cancellation_reason().ok_or_else(|| {
                ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext)
            })?,
            cleanup_status: CleanupStatus::Confirmed,
        }
    } else {
        WorkTransitionRequest::ResumeFromTool { tool_execution_id }
    };
    decide_work_transition(current, WorkTransitionGuard::for_snapshot(current), request)
        .map(|decision| decision.into_next())
        .map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidCallContext)
        })
}

fn event(call: &ToolExecutionCall, causation: JournalEventId) -> EventIntent {
    EventIntent {
        event_id: JournalEventId::generate(),
        correlation_id: call.correlation_id,
        causation_event_id: Some(causation),
    }
}

fn caused_work_event(call: &ToolExecutionCall) -> EventIntent {
    EventIntent {
        event_id: JournalEventId::generate(),
        correlation_id: call.correlation_id,
        causation_event_id: None,
    }
}

fn terminal_request(
    mut request: FinishToolExecutionRequest,
    call: &ToolExecutionCall,
    causation: JournalEventId,
) -> FinishToolExecutionRequest {
    let tool_event = event(call, causation);
    request.tool_event = tool_event;
    request.work_event = EventIntent {
        event_id: JournalEventId::generate(),
        correlation_id: call.correlation_id,
        causation_event_id: Some(tool_event.event_id),
    };
    request
}

fn current_cancellation(
    cancellation: &Option<tokio::sync::watch::Receiver<Option<ToolCancellationNotice>>>,
) -> Option<ToolCancellationNotice> {
    cancellation
        .as_ref()
        .and_then(|receiver| *receiver.borrow())
}

fn compose_dispatch_evidence(
    authority_evidence_json: &str,
    prepared_cwd: &PreparedCwdEvidence,
) -> Result<String, ToolExecutionServiceError> {
    let mut evidence: serde_json::Value =
        serde_json::from_str(authority_evidence_json).map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
        })?;
    let object = evidence.as_object_mut().ok_or_else(|| {
        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
    })?;
    let resolved = prepared_cwd.resolved_cwd();
    let identity = prepared_cwd.object_identity();
    object.insert("version".to_owned(), json!(2));
    object.insert(
        "prepared_cwd".to_owned(),
        json!({
            "device": identity.device(),
            "inode": identity.inode(),
            "object_type": match identity.object_type() {
                PreparedCwdObjectType::Directory => "directory",
            },
            "requested_cwd": resolved.requested_path().canonical(),
            "resolved_cwd": resolved.resolved_absolute_path(),
            "version": 1,
            "workspace_id": resolved.workspace_id().to_string(),
            "workstation_generation": resolved.workstation_generation().get(),
            "workstation_id": resolved.workstation_id().to_string(),
        }),
    );
    serde_json::to_string(&canonicalize_json_value(evidence)).map_err(|_| {
        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
    })
}

fn canonicalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonicalize_json_value).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json_value(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

fn effective_outer_deadline(
    work: MonotonicInstant,
    shutdown: Option<MonotonicInstant>,
) -> MonotonicInstant {
    shutdown.map_or(work, |shutdown| work.min(shutdown))
}

fn freeze_tool_deadline(
    requested: u64,
    definition: Option<&ToolDefinition>,
    config_max: u64,
    invocation_start: MonotonicInstant,
    outer_deadline: MonotonicInstant,
) -> Result<(u64, MonotonicInstant), ToolExecutionServiceError> {
    let remaining = outer_deadline
        .checked_duration_since(invocation_start)
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    let hard = definition
        .and_then(ToolDefinition::hard_timeout_ms)
        .unwrap_or(HARD_EXECUTION_TIMEOUT_MS);
    let bounded_timeout = requested.min(hard).min(config_max);
    let timeout_deadline = monotonic_deadline(invocation_start, bounded_timeout)?;
    let deadline = timeout_deadline.min(outer_deadline);
    let effective = remaining.min(bounded_timeout);
    if effective == 0 {
        Err(ToolExecutionServiceError::new(
            ToolExecutionServiceErrorKind::CancelledBeforeIntent,
        ))
    } else {
        Ok((effective, deadline))
    }
}

fn capability_bounded_timeout(
    policy_timeout_ms: u64,
    definition: &ToolDefinition,
    workstation_max: u64,
) -> u64 {
    if definition.hard_timeout_ms().is_some() {
        policy_timeout_ms.min(workstation_max)
    } else {
        policy_timeout_ms
    }
}

fn monotonic_deadline(
    now: MonotonicInstant,
    timeout_ms: u64,
) -> Result<MonotonicInstant, ToolExecutionServiceError> {
    now.elapsed()
        .checked_add(Duration::from_millis(timeout_ms))
        .map(MonotonicInstant::from_elapsed)
        .ok_or_else(|| ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::Clock))
}

fn count(value: u64) -> Result<CanonicalByteCount, ToolExecutionServiceError> {
    CanonicalByteCount::try_new(value).map_err(|_| {
        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
    })
}

fn execution_result_class(result: &ExecutionResult) -> ToolResultClass {
    match result.result_kind {
        ExecutionResultKind::Exited if result.exit_code == Some(0) => ToolResultClass::Success,
        ExecutionResultKind::Exited => ToolResultClass::ProcessExit,
        ExecutionResultKind::Signaled => ToolResultClass::SignalTermination,
        ExecutionResultKind::TimedOut => ToolResultClass::Timeout,
        ExecutionResultKind::Cancelled => ToolResultClass::Cancellation,
        ExecutionResultKind::SpawnFailed => ToolResultClass::SpawnFailure,
        ExecutionResultKind::CleanupFailed => ToolResultClass::CleanupFailure,
    }
}

const fn execution_summary(class: ToolResultClass) -> &'static str {
    match class {
        ToolResultClass::Success => "Command completed successfully.",
        ToolResultClass::ProcessExit => "Command exited with a nonzero status.",
        ToolResultClass::SignalTermination => "Command terminated by signal.",
        ToolResultClass::Timeout => "Command timed out with cleanup confirmed.",
        ToolResultClass::Cancellation => "Command cancellation was confirmed.",
        ToolResultClass::SpawnFailure => "Command failed before process start.",
        ToolResultClass::CleanupFailure => "Command cleanup was not confirmed.",
        _ => "Command completed with a structured result.",
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_evidence(
    call: &ToolExecutionCall,
    tool_execution_id: ToolExecutionId,
    stream: Option<ExecutionStreamResult>,
    logical_name: &str,
    per_stream_limit: u64,
    combined_remaining: u64,
    created_at: UtcTimestamp,
    artifacts: &mut Vec<PreparedArtifact>,
    artifact_ids: &mut Vec<ArtifactId>,
) -> Result<(Option<ArtifactId>, Option<ToolStreamCounts>, String, u64), ToolExecutionServiceError>
{
    let Some(stream) = stream else {
        return Ok((None, None, String::new(), 0));
    };
    let projection_limit =
        usize::try_from(per_stream_limit.min(combined_remaining)).map_err(|_| {
            ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
        })?;
    let (projection, projection_omitted) = truncate_utf8(&stream.projection, projection_limit);
    let returned_inline = u64::try_from(projection.len()).map_err(|_| {
        ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
    })?;
    let counts = ToolStreamCounts {
        observed: count(stream.observed_bytes)?,
        captured: count(stream.captured_bytes)?,
        returned_inline: count(returned_inline)?,
        omitted: count(stream.observed_bytes.saturating_sub(returned_inline))?,
    };
    let artifact_id = if stream.captured_bytes > 0 {
        let id = stream.artifact.artifact_id();
        artifacts.push(prepared_artifact(
            call,
            tool_execution_id,
            stream.artifact,
            "application/octet-stream",
            None,
            logical_name,
            created_at,
        )?);
        artifact_ids.push(id);
        Some(id)
    } else {
        None
    };
    Ok((artifact_id, Some(counts), projection, projection_omitted))
}

fn prepared_artifact(
    call: &ToolExecutionCall,
    tool_execution_id: ToolExecutionId,
    finalized: FinalizedArtifact,
    mime_type: &str,
    encoding: Option<&str>,
    logical_name: &str,
    created_at: UtcTimestamp,
) -> Result<PreparedArtifact, ToolExecutionServiceError> {
    let artifact_id = finalized.artifact_id();
    Ok(PreparedArtifact {
        metadata: ArtifactReference::new(ArtifactReferenceInput {
            artifact_id,
            craxii_id: call.craxii_id,
            producing_work_id: Some(call.work.work_id()),
            producer: ArtifactProducer::Tool(tool_execution_id),
            storage_key: ArtifactStorageKey::from_digest(finalized.sha256()),
            sha256: finalized.sha256(),
            canonical_length: finalized.captured_byte_count(),
            observed_length: Some(finalized.observed_byte_count()),
            mime_type: ArtifactMimeType::try_new(mime_type).map_err(|_| {
                ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
            })?,
            encoding: encoding
                .map(ArtifactEncoding::try_new)
                .transpose()
                .map_err(|_| {
                    ToolExecutionServiceError::new(
                        ToolExecutionServiceErrorKind::InvalidComposition,
                    )
                })?,
            logical_name: Some(ArtifactLogicalName::try_new(logical_name).map_err(|_| {
                ToolExecutionServiceError::new(ToolExecutionServiceErrorKind::InvalidComposition)
            })?),
            retention: ArtifactRetention::CanonicalEvidence,
            truncated: finalized.truncated(),
            compression: None,
            created_at,
        }),
        finalized,
        event: EventIntent {
            event_id: JournalEventId::generate(),
            correlation_id: call.correlation_id,
            causation_event_id: None,
        },
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, u64) {
    if value.len() <= max_bytes {
        return (value.to_owned(), 0);
    }
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (
        value[..boundary].to_owned(),
        u64::try_from(value.len() - boundary).unwrap_or(u64::MAX),
    )
}

fn insert_projection_chunks(fields: &mut BTreeMap<String, String>, prefix: &str, value: &str) {
    let mut remaining = value;
    let mut index = 1_u64;
    while !remaining.is_empty() {
        let mut boundary = remaining.len().min(4_000);
        while boundary > 0 && !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        fields.insert(
            format!("{prefix}_{index:04}"),
            remaining[..boundary].to_owned(),
        );
        remaining = &remaining[boundary..];
        index += 1;
    }
}

struct BoundedShellResultFields {
    fields: Vec<(String, String)>,
    stdout_returned: u64,
    stderr_returned: u64,
}

fn bounded_shell_result_fields(
    fixed: BTreeMap<String, String>,
    stdout: &str,
    stderr: &str,
    result_class: ToolResultClass,
    summary: &str,
) -> Result<BoundedShellResultFields, ToolExecutionServiceError> {
    let maximum = stdout.len().saturating_add(stderr.len());
    let build = |budget: usize| {
        let stdout_budget = stdout.len().min(budget);
        let (stdout_projection, _) = truncate_utf8(stdout, stdout_budget);
        let stderr_budget = stderr
            .len()
            .min(budget.saturating_sub(stdout_projection.len()));
        let (stderr_projection, _) = truncate_utf8(stderr, stderr_budget);
        let mut fields = fixed.clone();
        fields.insert(
            "stderr_projection_omitted_bytes".to_owned(),
            stderr
                .len()
                .saturating_sub(stderr_projection.len())
                .to_string(),
        );
        fields.insert(
            "stdout_projection_omitted_bytes".to_owned(),
            stdout
                .len()
                .saturating_sub(stdout_projection.len())
                .to_string(),
        );
        insert_projection_chunks(&mut fields, "stderr", &stderr_projection);
        insert_projection_chunks(&mut fields, "stdout", &stdout_projection);
        (
            fields.into_iter().collect::<Vec<_>>(),
            stdout_projection.len(),
            stderr_projection.len(),
        )
    };

    let fits = |fields: &[(String, String)]| {
        serde_json::to_vec(&json!({
            "fields": fields,
            "result_kind": result_class.as_str(),
            "summary": summary,
            "version": 1,
        }))
        .is_ok_and(|bytes| bytes.len() <= MAX_TOOL_RESULT_JSON_BYTES)
    };
    let complete = build(maximum);
    if fits(&complete.0) {
        return Ok(BoundedShellResultFields {
            fields: complete.0,
            stdout_returned: complete.1 as u64,
            stderr_returned: complete.2 as u64,
        });
    }

    let mut low = 0_usize;
    let mut high = maximum;
    while low < high {
        let midpoint = low + (high - low).div_ceil(2);
        if fits(&build(midpoint).0) {
            low = midpoint;
        } else {
            high = midpoint - 1;
        }
    }
    let bounded = build(low);
    if !fits(&bounded.0) {
        return Err(ToolExecutionServiceError::new(
            ToolExecutionServiceErrorKind::InvalidComposition,
        ));
    }
    Ok(BoundedShellResultFields {
        fields: bounded.0,
        stdout_returned: bounded.1 as u64,
        stderr_returned: bounded.2 as u64,
    })
}

fn update_stream_projection_counts(
    counts: &mut Option<ToolStreamCounts>,
    returned_inline: u64,
) -> Result<(), ToolExecutionServiceError> {
    if let Some(counts) = counts {
        counts.returned_inline = count(returned_inline)?;
        counts.omitted = count(counts.observed.get().saturating_sub(returned_inline))?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandlerPollFailure {
    Panicked,
    CancelledBeforeHandoff,
}

struct CatchHandlerPanic<'a> {
    inner: ToolHandlerFuture<'a>,
    handoff_started: Arc<AtomicBool>,
}

impl Future for CatchHandlerPanic<'_> {
    type Output = Result<Result<ToolHandlerObservation, WorkstationError>, HandlerPollFailure>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.handoff_started.store(true, Ordering::Release);
        match catch_unwind(AssertUnwindSafe(|| self.inner.as_mut().poll(context))) {
            Ok(Poll::Ready(result)) => Poll::Ready(Ok(result)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(Err(HandlerPollFailure::Panicked)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use time::OffsetDateTime;

    use super::*;
    use crate::application::authority::V0AuthorityEvaluator;
    use crate::domain::{
        CurrentWorkAttempt, LogicalPathReference, ResolvedPathEvidence, WorkspaceCapabilityRef,
        WorkstationCapabilities, WorkstationCapabilitiesInput, WorkstationCapabilityFlags,
        WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits,
    };
    use crate::ports::artifact_store::{
        ArtifactCapture, ArtifactObjectReference, ArtifactOrphanReport, ArtifactStoreError,
        ArtifactStoreErrorKind,
    };
    use crate::ports::clock::TestClock;
    use crate::ports::state_store::{CommitReceipt, StateStoreErrorKind, StateStoreFuture};
    use crate::ports::workstation::{
        CancellationResult, CapabilitiesResult, ExecutionCleanupEvidence, ExecutionInspection,
        ExecutionInspectionRequest, ExecutionResultKind, ExecutionStreamResult, FileEncoding,
        FileReadResult, WorkstationFileType, WorkstationFuture,
    };
    use crate::ports::workstation_preparation::{
        PreparedCwdObjectIdentity, WorkstationPreparationFuture, WorkstationPreparationResult,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MockMode {
        ReadSuccess,
        ShellExitZero,
        ShellExitNonzero,
        ShellSignaled,
        ShellTimedOut,
        ShellSpawnFailed,
        ShellCleanupFailed,
        ShellPanic,
        ShellIdentityMismatch,
        CancellationAfterDispatch,
        ActiveCancellation,
    }

    struct CancellationPlan {
        sender: tokio::sync::watch::Sender<Option<ToolCancellationNotice>>,
        notice: Mutex<Option<ToolCancellationNotice>>,
        trigger_after_request: bool,
        trigger_after_dispatch: bool,
        trigger_during_execute: bool,
        cancellation_issued: tokio::sync::Notify,
    }

    struct MockToolStore {
        log: Arc<Mutex<Vec<&'static str>>>,
        fail_request: bool,
        fail_dispatch: bool,
        fail_finish: bool,
        seen_calls: Mutex<HashSet<(ModelInvocationId, ToolOrdinal)>>,
        cancellation: Option<Arc<CancellationPlan>>,
        artifact_counts: Mutex<Vec<usize>>,
        outcome_states: Mutex<Vec<ToolExecutionState>>,
        dispatch_authority_evidence: Mutex<Vec<String>>,
        clock: Arc<TestClock>,
        request_advance: Mutex<Option<Duration>>,
        dispatch_advance: Mutex<Option<Duration>>,
        result_json_lengths: Mutex<Vec<usize>>,
        stream_counts: Mutex<Vec<(Option<ToolStreamCounts>, Option<ToolStreamCounts>)>>,
    }

    impl ToolStateStore for MockToolStore {
        fn request_tool_execution(
            &self,
            request: RequestToolExecutionRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                self.log.lock().unwrap().push("requested_commit");
                if self.fail_request {
                    return Err(StateStoreError::new(StateStoreErrorKind::Storage));
                }
                let key = (
                    request.tool.lifecycle.source_model_invocation_id(),
                    request.tool.lifecycle.tool_ordinal(),
                );
                if !self.seen_calls.lock().unwrap().insert(key) {
                    return Err(StateStoreError::new(
                        StateStoreErrorKind::IdempotencyConflict,
                    ));
                }
                if let Some(duration) = *self.request_advance.lock().unwrap() {
                    self.clock.advance_monotonic(duration).unwrap();
                }
                if let Some(plan) = &self.cancellation {
                    let notice = ToolCancellationNotice {
                        expected_work: WorkExpectation {
                            work_id: request.work_next.work_id(),
                            state: WorkState::CancelRequested,
                            version: request
                                .work_next
                                .projection_version()
                                .checked_increment()
                                .unwrap(),
                            runtime_owner: request.work_next.runtime_owner(),
                            current_attempt: request.work_next.current_attempt(),
                            cancellation_reason: request.work_next.cancellation_reason(),
                        },
                        reason: WorkCancellationReason::UserRequest,
                    };
                    *plan.notice.lock().unwrap() = Some(notice);
                    if plan.trigger_after_request {
                        plan.sender.send_replace(Some(notice));
                    }
                }
                Ok(commit())
            })
        }

        fn commit_tool_dispatch_intent(
            &self,
            request: CommitToolDispatchIntentRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                self.log.lock().unwrap().push("dispatch_commit");
                if self.fail_dispatch {
                    Err(StateStoreError::new(StateStoreErrorKind::Storage))
                } else {
                    if let Some(duration) = *self.dispatch_advance.lock().unwrap() {
                        self.clock.advance_monotonic(duration).unwrap();
                    }
                    self.dispatch_authority_evidence
                        .lock()
                        .unwrap()
                        .push(request.dispatch.dispatch_evidence_json);
                    if let Some(plan) = &self.cancellation
                        && plan.trigger_after_dispatch
                    {
                        let notice = plan.notice.lock().unwrap().expect("request stored notice");
                        plan.sender.send_replace(Some(notice));
                    }
                    Ok(commit())
                }
            })
        }

        fn finish_tool_execution(
            &self,
            request: FinishToolExecutionRequest,
        ) -> StateStoreFuture<'_, CommitReceipt> {
            Box::pin(async move {
                self.log.lock().unwrap().push("outcome_commit");
                self.artifact_counts
                    .lock()
                    .unwrap()
                    .push(request.artifacts.len());
                self.outcome_states
                    .lock()
                    .unwrap()
                    .push(request.outcome.state);
                if let Some(result) = &request.outcome.result {
                    self.result_json_lengths.lock().unwrap().push(
                        serde_json::to_vec(&json!({
                            "fields": result.fields,
                            "result_kind": result.result_kind.as_str(),
                            "summary": result.summary,
                            "version": 1,
                        }))
                        .unwrap()
                        .len(),
                    );
                }
                self.stream_counts
                    .lock()
                    .unwrap()
                    .push((request.outcome.stdout_counts, request.outcome.stderr_counts));
                if request.work_event.causation_event_id != Some(request.tool_event.event_id) {
                    return Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant));
                }
                if self.fail_finish {
                    Err(StateStoreError::new(StateStoreErrorKind::Storage))
                } else {
                    Ok(commit())
                }
            })
        }
    }

    fn commit() -> CommitReceipt {
        CommitReceipt {
            committed_version: None,
            events: None,
        }
    }

    struct MockWorkstation {
        log: Arc<Mutex<Vec<&'static str>>>,
        capabilities: WorkstationCapabilities,
        mode: MockMode,
        cancellation: Option<Arc<CancellationPlan>>,
        operation_ids: Mutex<HashSet<OperationId>>,
        execution_ids: Mutex<Vec<ExecutionId>>,
        cancelled_execution_ids: Mutex<Vec<ExecutionId>>,
        execution_requests: Mutex<Vec<ObservedExecutionRequest>>,
        custom_streams: Mutex<Option<(Vec<u8>, Vec<u8>)>>,
        clock: Arc<TestClock>,
        capabilities_advance: Mutex<Option<Duration>>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ObservedExecutionRequest {
        timeout_ms: u64,
        deadline: MonotonicInstant,
        requested_cwd: LogicalPathReference,
        effective_privilege: PrivilegeMode,
    }

    impl MockWorkstation {
        fn observe_operation(&self, operation_id: OperationId) {
            assert!(self.operation_ids.lock().unwrap().insert(operation_id));
        }
    }

    impl Workstation for MockWorkstation {
        fn capabilities(
            &self,
            request: CapabilitiesRequest,
        ) -> WorkstationFuture<'_, CapabilitiesResult> {
            self.observe_operation(request.operation_id);
            self.log.lock().unwrap().push("capabilities");
            if let Some(duration) = *self.capabilities_advance.lock().unwrap() {
                self.clock.advance_monotonic(duration).unwrap();
            }
            let result = CapabilitiesResult {
                operation_id: request.operation_id,
                capabilities: self.capabilities.clone(),
            };
            Box::pin(async move { Ok(result) })
        }

        fn read_file(
            &self,
            request: crate::ports::workstation::FileReadRequest,
        ) -> WorkstationFuture<'_, FileReadResult> {
            self.observe_operation(request.operation_id);
            self.log.lock().unwrap().push("read_file");
            let text = if self.mode == MockMode::ReadSuccess {
                "hello, 世界\0".to_owned()
            } else {
                "x".repeat(40_000)
            };
            let byte_length = count(text.len() as u64).unwrap();
            let result = FileReadResult {
                operation_id: request.operation_id,
                requested_path: request.path.clone(),
                resolved_path: ResolvedPathEvidence::try_new(
                    request.workstation_id,
                    request.expected_generation,
                    request.workspace_id,
                    request.path,
                    "/workspace/file.txt",
                )
                .unwrap(),
                file_type: WorkstationFileType::Regular,
                byte_length,
                modified_at: None,
                encoding: FileEncoding::Utf8,
                sha256: Sha256Digest::hash_bytes(text.as_bytes()),
                text,
                truncated: false,
            };
            Box::pin(async move { Ok(result) })
        }

        fn execute(
            &self,
            request: crate::ports::workstation::ExecutionRequest,
        ) -> WorkstationFuture<'_, ExecutionResult> {
            self.observe_operation(request.operation_id);
            self.execution_ids
                .lock()
                .unwrap()
                .push(request.execution_id);
            self.execution_requests
                .lock()
                .unwrap()
                .push(ObservedExecutionRequest {
                    timeout_ms: u64::try_from(request.timeout.as_duration().as_millis()).unwrap(),
                    deadline: request.deadline,
                    requested_cwd: request.requested_cwd.clone(),
                    effective_privilege: request.effective_privilege,
                });
            self.log.lock().unwrap().push("execute");
            let mode = self.mode;
            let cancellation = self.cancellation.clone();
            let custom_streams = self.custom_streams.lock().unwrap().take();
            Box::pin(async move {
                if mode == MockMode::ShellPanic {
                    panic!("test handler poll panic");
                }
                if mode == MockMode::ActiveCancellation
                    && let Some(plan) = cancellation
                    && plan.trigger_during_execute
                {
                    let notice = plan.notice.lock().unwrap().expect("request stored notice");
                    plan.sender.send_replace(Some(notice));
                    plan.cancellation_issued.notified().await;
                }
                let mut result = execution_result(request, mode);
                if let Some((stdout, stderr)) = custom_streams {
                    result.stdout = Some(stream_result(&stdout));
                    result.stderr = Some(stream_result(&stderr));
                }
                if mode == MockMode::ShellIdentityMismatch {
                    result.operation_id = OperationId::generate();
                }
                Ok(result)
            })
        }

        fn inspect_execution(
            &self,
            _request: ExecutionInspectionRequest,
        ) -> WorkstationFuture<'_, ExecutionInspection> {
            Box::pin(async {
                Err(WorkstationError::new(
                    WorkstationErrorKind::InspectionNotFound,
                ))
            })
        }

        fn cancel_execution(
            &self,
            request: ExecutionCancellationRequest,
        ) -> WorkstationFuture<'_, CancellationResult> {
            self.observe_operation(request.operation_id);
            self.log.lock().unwrap().push("cancel_execution");
            self.cancelled_execution_ids
                .lock()
                .unwrap()
                .push(request.execution_id);
            if let Some(plan) = &self.cancellation {
                plan.cancellation_issued.notify_waiters();
            }
            Box::pin(async move {
                Ok(CancellationResult {
                    operation_id: request.operation_id,
                    execution_id: request.execution_id,
                    state: ExecutionCancellationState::Confirmed,
                })
            })
        }
    }

    fn execution_result(
        request: crate::ports::workstation::ExecutionRequest,
        mode: MockMode,
    ) -> ExecutionResult {
        let (kind, exit_code, signal, timed_out, cancelled, started, error, certainty, cleanup) =
            match mode {
                MockMode::ShellExitZero | MockMode::ShellIdentityMismatch => (
                    ExecutionResultKind::Exited,
                    Some(0),
                    None,
                    false,
                    false,
                    true,
                    None,
                    Certainty::Definite,
                    true,
                ),
                MockMode::ShellExitNonzero => (
                    ExecutionResultKind::Exited,
                    Some(7),
                    None,
                    false,
                    false,
                    true,
                    None,
                    Certainty::Definite,
                    true,
                ),
                MockMode::ShellSignaled => (
                    ExecutionResultKind::Signaled,
                    None,
                    Some(15),
                    false,
                    false,
                    true,
                    None,
                    Certainty::Definite,
                    true,
                ),
                MockMode::ShellTimedOut => (
                    ExecutionResultKind::TimedOut,
                    None,
                    None,
                    true,
                    false,
                    true,
                    None,
                    Certainty::Definite,
                    true,
                ),
                MockMode::ShellSpawnFailed => (
                    ExecutionResultKind::SpawnFailed,
                    None,
                    None,
                    false,
                    false,
                    false,
                    Some(WorkstationError::new(WorkstationErrorKind::SpawnFailed)),
                    Certainty::Definite,
                    true,
                ),
                MockMode::ActiveCancellation => (
                    ExecutionResultKind::Cancelled,
                    None,
                    None,
                    false,
                    true,
                    true,
                    None,
                    Certainty::Definite,
                    true,
                ),
                MockMode::ShellCleanupFailed => (
                    ExecutionResultKind::CleanupFailed,
                    None,
                    None,
                    false,
                    false,
                    true,
                    Some(WorkstationError::uncertain(
                        WorkstationErrorKind::CleanupFailed,
                    )),
                    Certainty::OutcomeUnknown,
                    false,
                ),
                MockMode::ReadSuccess
                | MockMode::ShellPanic
                | MockMode::CancellationAfterDispatch => unreachable!(),
            };
        let resolved = ResolvedPathEvidence::try_new(
            request.workstation_id,
            request.expected_generation,
            request.workspace_id,
            request.requested_cwd.clone(),
            "/workspace",
        )
        .unwrap();
        let stdout = started.then(|| stream_result(b"stdout\0bytes"));
        let stderr = started.then(|| stream_result(b"stderr"));
        ExecutionResult {
            operation_id: request.operation_id,
            execution_id: request.execution_id,
            start_observed: started,
            requested_cwd: request.requested_cwd,
            resolved_cwd: resolved,
            effective_privilege: request.effective_privilege,
            command_sha256: Sha256Digest::hash_bytes(request.command.as_bytes()),
            result_kind: kind,
            exit_code,
            terminating_signal: signal,
            timed_out,
            cancelled,
            duration: MonotonicDuration::from_millis(12),
            stdout,
            stderr,
            cleanup: ExecutionCleanupEvidence {
                direct_child_reaped: cleanup,
                stdout_drain_joined: cleanup,
                stderr_drain_joined: cleanup,
                process_group_empty: cleanup,
                cgroup_empty: None,
                cgroup_removed: None,
            },
            error,
            certainty,
        }
    }

    fn stream_result(bytes: &[u8]) -> ExecutionStreamResult {
        let artifact = finalized(ArtifactId::generate(), bytes, bytes.len() as u64);
        ExecutionStreamResult {
            artifact,
            projection: String::from_utf8_lossy(bytes).into_owned(),
            projection_had_utf8_replacement: false,
            observed_bytes: bytes.len() as u64,
            captured_bytes: bytes.len() as u64,
            omitted_bytes: 0,
            projection_omitted_bytes: 0,
            observed_count_saturated: false,
            truncated: false,
        }
    }

    struct MockPreparation {
        log: Arc<Mutex<Vec<&'static str>>>,
        clock: Arc<TestClock>,
        advance: Arc<Mutex<Option<Duration>>>,
    }

    impl WorkstationPreparation for MockPreparation {
        fn prepare(
            &self,
            request: WorkstationPreparationRequest,
        ) -> WorkstationPreparationFuture<'_, WorkstationPreparationResult> {
            self.log.lock().unwrap().push("prepare");
            if let Some(duration) = *self.advance.lock().unwrap() {
                self.clock.advance_monotonic(duration).unwrap();
            }
            let result = WorkstationPreparationResult {
                operation_id: request.operation_id,
                prepared_cwd: PreparedCwdEvidence::new(
                    ResolvedPathEvidence::try_new(
                        request.workstation_id,
                        request.expected_generation,
                        request.workspace_id,
                        request.requested_cwd,
                        "/workspace",
                    )
                    .unwrap(),
                    PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory)
                        .unwrap(),
                ),
            };
            Box::pin(async move { Ok(result) })
        }
    }

    #[derive(Default)]
    struct MemoryArtifactStore;

    impl ArtifactStore for MemoryArtifactStore {
        fn begin_capture(
            &self,
            request: BeginArtifactCapture,
        ) -> Result<Box<dyn ArtifactCapture>, ArtifactStoreError> {
            Ok(Box::new(MemoryCapture {
                id: request.artifact_id,
                limit: request.hard_capture_limit.get(),
                bytes: Vec::new(),
                observed: 0,
            }))
        }

        fn verify(&self, _artifact: &ArtifactObjectReference) -> Result<(), ArtifactStoreError> {
            Ok(())
        }

        fn read_verified(
            &self,
            _artifact: &ArtifactObjectReference,
        ) -> Result<Vec<u8>, ArtifactStoreError> {
            Ok(Vec::new())
        }

        fn scan_orphans(
            &self,
            _referenced: &std::collections::BTreeSet<ArtifactStorageKey>,
            _observed_at: UtcTimestamp,
        ) -> Result<ArtifactOrphanReport, ArtifactStoreError> {
            Ok(ArtifactOrphanReport {
                referenced_final_count: 0,
                orphans: Vec::new(),
            })
        }
    }

    struct FailingArtifactStore;

    impl ArtifactStore for FailingArtifactStore {
        fn begin_capture(
            &self,
            _request: BeginArtifactCapture,
        ) -> Result<Box<dyn ArtifactCapture>, ArtifactStoreError> {
            Err(ArtifactStoreError::new(ArtifactStoreErrorKind::Storage))
        }

        fn verify(&self, _artifact: &ArtifactObjectReference) -> Result<(), ArtifactStoreError> {
            Ok(())
        }

        fn read_verified(
            &self,
            _artifact: &ArtifactObjectReference,
        ) -> Result<Vec<u8>, ArtifactStoreError> {
            Ok(Vec::new())
        }

        fn scan_orphans(
            &self,
            _referenced: &std::collections::BTreeSet<ArtifactStorageKey>,
            _observed_at: UtcTimestamp,
        ) -> Result<ArtifactOrphanReport, ArtifactStoreError> {
            Ok(ArtifactOrphanReport {
                referenced_final_count: 0,
                orphans: Vec::new(),
            })
        }
    }

    struct MemoryCapture {
        id: ArtifactId,
        limit: u64,
        bytes: Vec<u8>,
        observed: u64,
    }

    impl ArtifactCapture for MemoryCapture {
        fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), ArtifactStoreError> {
            self.observed = self.observed.saturating_add(chunk.len() as u64);
            let remaining = self.limit.saturating_sub(self.bytes.len() as u64) as usize;
            self.bytes
                .extend_from_slice(&chunk[..remaining.min(chunk.len())]);
            Ok(())
        }

        fn finalize(self: Box<Self>) -> Result<FinalizedArtifact, ArtifactStoreError> {
            Ok(finalized(self.id, &self.bytes, self.observed))
        }
    }

    fn finalized(id: ArtifactId, bytes: &[u8], observed: u64) -> FinalizedArtifact {
        let digest = Sha256Digest::hash_bytes(bytes);
        FinalizedArtifact::from_durable_publication(
            id,
            ArtifactStorageKey::from_digest(digest),
            digest,
            count(bytes.len() as u64).unwrap(),
            count(observed).unwrap(),
            observed > bytes.len() as u64,
        )
    }

    struct Fixture {
        service: ToolExecutionService,
        store: Arc<MockToolStore>,
        workstation: Arc<MockWorkstation>,
        craxii_id: CraxiiId,
        workstation_id: WorkstationId,
        workspace_id: crate::domain::WorkspaceId,
        runtime_id: RuntimeInstanceId,
        clock: Arc<TestClock>,
        preparation_advance: Arc<Mutex<Option<Duration>>>,
        cancellation_receiver: Option<tokio::sync::watch::Receiver<Option<ToolCancellationNotice>>>,
    }

    impl Fixture {
        fn new(mode: MockMode) -> Self {
            Self::with_options(mode, false, false, false, false, false)
        }

        fn with_options(
            mode: MockMode,
            admin: bool,
            fail_request: bool,
            fail_dispatch: bool,
            fail_finish: bool,
            trigger_after_request: bool,
        ) -> Self {
            let log = Arc::new(Mutex::new(Vec::new()));
            let craxii_id = CraxiiId::generate();
            let workstation_id = WorkstationId::generate();
            let workspace_id = crate::domain::WorkspaceId::generate();
            let runtime_id = RuntimeInstanceId::generate();
            let capabilities = capabilities(workstation_id, workspace_id, admin);
            let clock = Arc::new(TestClock::new(
                OffsetDateTime::from_unix_timestamp(1_780_000_000).unwrap(),
                Duration::from_secs(10),
            ));
            let (sender, receiver) = tokio::sync::watch::channel(None);
            let use_cancellation = trigger_after_request
                || matches!(
                    mode,
                    MockMode::ActiveCancellation | MockMode::CancellationAfterDispatch
                );
            let cancellation = use_cancellation.then(|| {
                Arc::new(CancellationPlan {
                    sender,
                    notice: Mutex::new(None),
                    trigger_after_request,
                    trigger_after_dispatch: mode == MockMode::CancellationAfterDispatch,
                    trigger_during_execute: mode == MockMode::ActiveCancellation,
                    cancellation_issued: tokio::sync::Notify::new(),
                })
            });
            let store = Arc::new(MockToolStore {
                log: Arc::clone(&log),
                fail_request,
                fail_dispatch,
                fail_finish,
                seen_calls: Mutex::new(HashSet::new()),
                cancellation: cancellation.clone(),
                artifact_counts: Mutex::new(Vec::new()),
                outcome_states: Mutex::new(Vec::new()),
                dispatch_authority_evidence: Mutex::new(Vec::new()),
                clock: Arc::clone(&clock),
                request_advance: Mutex::new(None),
                dispatch_advance: Mutex::new(None),
                result_json_lengths: Mutex::new(Vec::new()),
                stream_counts: Mutex::new(Vec::new()),
            });
            let workstation = Arc::new(MockWorkstation {
                log: Arc::clone(&log),
                capabilities,
                mode,
                cancellation,
                operation_ids: Mutex::new(HashSet::new()),
                execution_ids: Mutex::new(Vec::new()),
                cancelled_execution_ids: Mutex::new(Vec::new()),
                execution_requests: Mutex::new(Vec::new()),
                custom_streams: Mutex::new(None),
                clock: Arc::clone(&clock),
                capabilities_advance: Mutex::new(None),
            });
            let preparation_advance = Arc::new(Mutex::new(None));
            let limits = test_limits();
            let registry = Arc::new(
                ToolRegistry::v0(crate::application::tool_registry::ToolSemanticPolicy {
                    read_file_default_bytes: limits.read_file_default_bytes,
                    read_file_max_bytes: limits.read_file_max_bytes,
                    run_shell_command_max_bytes: limits.run_shell_command_max_bytes,
                    run_shell_default_timeout_ms: limits.run_shell_default_timeout_ms,
                    run_shell_max_timeout_ms: limits.run_shell_max_timeout_ms,
                })
                .unwrap(),
            );
            let authority: Arc<dyn AuthorityEvaluator> = Arc::new(V0AuthorityEvaluator);
            let state_store: Arc<dyn ToolStateStore> = store.clone();
            let workstation_port: Arc<dyn Workstation> = workstation.clone();
            let preparation: Arc<dyn WorkstationPreparation> = Arc::new(MockPreparation {
                log,
                clock: Arc::clone(&clock),
                advance: Arc::clone(&preparation_advance),
            });
            let artifacts: Arc<dyn ArtifactStore> = Arc::new(MemoryArtifactStore);
            let clock_port: Arc<dyn Clock> = clock.clone();
            let service = ToolExecutionService::new(
                registry,
                authority,
                state_store,
                workstation_port,
                preparation,
                artifacts,
                clock_port,
                limits,
            )
            .unwrap();
            Self {
                service,
                store,
                workstation,
                craxii_id,
                workstation_id,
                workspace_id,
                runtime_id,
                clock,
                preparation_advance,
                cancellation_receiver: use_cancellation.then_some(receiver),
            }
        }

        fn call(&self, tool: &str, arguments: &[u8], ordinal: i64) -> ToolExecutionCall {
            let workspace = WorkspaceIdentity::try_new(crate::domain::WorkspaceIdentityInput {
                workspace_id: self.workspace_id,
                craxii_id: self.craxii_id,
                workstation_id: self.workstation_id,
                logical_name: "primary".into(),
                logical_root: LogicalPathReference::absolute("/workspace").unwrap(),
                created_at: "2026-08-30T00:00:00.000000Z".parse().unwrap(),
            })
            .unwrap();
            ToolExecutionCall {
                craxii_id: self.craxii_id,
                work: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                    work_id: crate::domain::WorkId::generate(),
                    state: WorkState::Running,
                    projection_version: ProjectionVersion::try_new(4).unwrap(),
                    runtime_owner: Some(self.runtime_id),
                    current_attempt: CurrentWorkAttempt::None,
                    cancellation_reason: None,
                    terminal_reason: None,
                })
                .unwrap(),
                runtime_instance_id: self.runtime_id,
                source_model_invocation_id: ModelInvocationId::generate(),
                source_model_event_id: JournalEventId::generate(),
                agent_step_no: AgentStepNo::try_new(1).unwrap(),
                tool_ordinal: ToolOrdinal::try_new(ordinal).unwrap(),
                provider_tool_call_id: Some(format!("call-{ordinal}")),
                tool_name: tool.to_owned(),
                raw_arguments: arguments.to_vec(),
                correlation_id: CorrelationId::generate(),
                workstation_id: self.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                workspace,
                work_deadline: MonotonicInstant::from_elapsed(Duration::from_secs(1_000)),
                shutdown_deadline: None,
                authority_constraints: V0AuthorityConstraints::default(),
                cancellation: self.cancellation_receiver.clone(),
            }
        }

        fn log(&self) -> Vec<&'static str> {
            self.store.log.lock().unwrap().clone()
        }
    }

    fn capabilities(
        workstation_id: WorkstationId,
        workspace_id: crate::domain::WorkspaceId,
        admin: bool,
    ) -> WorkstationCapabilities {
        WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
            workstation_id,
            generation: WorkstationGeneration::try_new(1).unwrap(),
            cpu_architecture: "aarch64".into(),
            os_release: "macos".into(),
            default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
            flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
                filesystem_read: true,
                foreground_execute: true,
                cancel_execution: true,
                inspect_execution: true,
                privilege_user: true,
                privilege_administrative: admin,
                process_group_cleanup: true,
                cgroup_cleanup: false,
            }),
            limits: WorkstationCapabilityLimits::try_new(900_000, 8_388_608, 8_388_608).unwrap(),
            workspaces: vec![
                WorkspaceCapabilityRef::try_new(
                    workspace_id,
                    LogicalPathReference::absolute("/workspace").unwrap(),
                )
                .unwrap(),
            ],
        })
        .unwrap()
    }

    fn test_limits() -> ToolRuntimeLimits {
        ToolRuntimeLimits {
            read_file_default_bytes: 1_048_576,
            read_file_max_bytes: 8_388_608,
            run_shell_command_max_bytes: 65_536,
            run_shell_default_timeout_ms: 120_000,
            run_shell_max_timeout_ms: 900_000,
            stdout_capture_bytes: 8_388_608,
            stderr_capture_bytes: 8_388_608,
            inline_model_result_bytes: 65_536,
            per_stream_projection_bytes: 32_768,
        }
    }

    #[tokio::test]
    async fn read_file_orders_both_commits_before_machine_action_and_commits_result_before_return()
    {
        let fixture = Fixture::new(MockMode::ReadSuccess);
        let result = fixture
            .service
            .execute_call(fixture.call("read_file", br#"{"path":"file.txt"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Success);
        assert!(result.fields.iter().any(|(key, _)| key == "sha256"));
        assert_eq!(
            fixture.log(),
            [
                "requested_commit",
                "capabilities",
                "prepare",
                "dispatch_commit",
                "read_file",
                "outcome_commit"
            ]
        );
        assert_eq!(fixture.workstation.operation_ids.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn validation_unknown_admin_and_request_failure_never_dispatch() {
        for (name, arguments) in [
            ("unknown_tool", br#"{}"#.as_slice()),
            ("read_file", br#"{"path":"x","path":"y"}"#.as_slice()),
            (
                "run_shell",
                br#"{"command":"x","timeout_seconds":0}"#.as_slice(),
            ),
        ] {
            let fixture = Fixture::new(MockMode::ReadSuccess);
            let result = fixture
                .service
                .execute_call(fixture.call(name, arguments, 1))
                .await
                .unwrap();
            assert!(matches!(
                result.result_class,
                ToolResultClass::UnknownTool | ToolResultClass::ValidationRejection
            ));
            assert_eq!(fixture.log(), ["requested_commit", "outcome_commit"]);
        }
        let admin_denied = Fixture::new(MockMode::ShellExitZero);
        let result = admin_denied
            .service
            .execute_call(admin_denied.call(
                "run_shell",
                br#"{"command":"true","privilege":"administrative"}"#,
                1,
            ))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::AuthorityDenial);
        assert_eq!(
            admin_denied.log(),
            ["requested_commit", "capabilities", "outcome_commit"]
        );

        let request_failure =
            Fixture::with_options(MockMode::ShellExitZero, false, true, false, false, false);
        assert_eq!(
            request_failure
                .service
                .execute_call(request_failure.call("run_shell", br#"{"command":"true"}"#, 1))
                .await
                .unwrap_err()
                .kind(),
            ToolExecutionServiceErrorKind::StateStore
        );
        assert_eq!(request_failure.log(), ["requested_commit"]);
    }

    #[tokio::test]
    async fn run_shell_maps_definite_result_classes_without_retry() {
        for (mode, expected) in [
            (MockMode::ShellExitZero, ToolResultClass::Success),
            (MockMode::ShellExitNonzero, ToolResultClass::ProcessExit),
            (MockMode::ShellSignaled, ToolResultClass::SignalTermination),
            (MockMode::ShellTimedOut, ToolResultClass::Timeout),
            (MockMode::ShellSpawnFailed, ToolResultClass::SpawnFailure),
        ] {
            let fixture = Fixture::new(mode);
            let result = fixture
                .service
                .execute_call(fixture.call("run_shell", br#"{"command":"printf ok"}"#, 1))
                .await
                .unwrap();
            assert_eq!(result.result_class, expected);
            assert_eq!(fixture.workstation.execution_ids.lock().unwrap().len(), 1);
            assert_eq!(
                fixture
                    .log()
                    .iter()
                    .filter(|item| **item == "execute")
                    .count(),
                1
            );
            assert_eq!(
                fixture.store.artifact_counts.lock().unwrap()[0],
                if mode == MockMode::ShellSpawnFailed {
                    0
                } else {
                    2
                }
            );
        }
    }

    #[tokio::test]
    async fn run_shell_effective_deadline_is_the_minimum_and_never_widens_privilege_or_cwd() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let mut call = fixture.call(
            "run_shell",
            br#"{"command":"true","cwd":"cwd","timeout_seconds":900}"#,
            1,
        );
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_secs(15));
        let result = fixture.service.execute_call(call).await.unwrap();
        assert_eq!(result.result_class, ToolResultClass::Success);
        let requests = fixture.workstation.execution_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].timeout_ms, 5_000);
        assert_eq!(
            requests[0].deadline,
            MonotonicInstant::from_elapsed(Duration::from_secs(15))
        );
        assert_eq!(
            requests[0].requested_cwd,
            LogicalPathReference::workspace_relative("cwd").unwrap()
        );
        assert_eq!(requests[0].effective_privilege, PrivilegeMode::User);
    }

    #[tokio::test]
    async fn preparation_and_dispatch_consume_one_frozen_absolute_deadline_budget() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.preparation_advance.lock().unwrap() = Some(Duration::from_secs(20));
        *fixture.store.dispatch_advance.lock().unwrap() = Some(Duration::from_secs(30));
        let result = fixture
            .service
            .execute_call(fixture.call(
                "run_shell",
                br#"{"command":"true","timeout_seconds":120}"#,
                1,
            ))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Success);
        let requests = fixture.workstation.execution_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].timeout_ms, 120_000);
        assert_eq!(
            requests[0].deadline,
            MonotonicInstant::from_elapsed(Duration::from_secs(130))
        );
        assert_eq!(
            fixture.clock.monotonic_now().elapsed(),
            Duration::from_secs(60)
        );
        assert_eq!(
            requests[0]
                .deadline
                .checked_duration_since(fixture.clock.monotonic_now()),
            Some(Duration::from_secs(70))
        );
    }

    #[tokio::test]
    async fn requested_capability_preparation_and_dispatch_all_consume_the_frozen_budget() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.store.request_advance.lock().unwrap() = Some(Duration::from_millis(30));
        *fixture.workstation.capabilities_advance.lock().unwrap() = Some(Duration::from_millis(20));
        *fixture.preparation_advance.lock().unwrap() = Some(Duration::from_millis(60));
        *fixture.store.dispatch_advance.lock().unwrap() = Some(Duration::from_millis(40));
        let mut call = fixture.call(
            "run_shell",
            br#"{"command":"true","timeout_seconds":120}"#,
            1,
        );
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_millis(10_200));
        let result = fixture.service.execute_call(call).await.unwrap();
        assert_eq!(result.result_class, ToolResultClass::Success);
        let requests = fixture.workstation.execution_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].deadline,
            MonotonicInstant::from_elapsed(Duration::from_millis(10_200))
        );
        assert!(
            requests[0]
                .deadline
                .checked_duration_since(fixture.clock.monotonic_now())
                .is_some_and(|remaining| remaining <= Duration::from_millis(50))
        );
    }

    #[tokio::test]
    async fn deadline_expiry_during_requested_persistence_prevents_capability_and_machine_action() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.store.request_advance.lock().unwrap() = Some(Duration::from_millis(201));
        let mut call = fixture.call("run_shell", br#"{"command":"true"}"#, 1);
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_millis(10_200));
        let result = fixture.service.execute_call(call).await.unwrap();
        assert_eq!(result.result_class, ToolResultClass::FileError);
        assert_eq!(result.error.unwrap().code, "timeout");
        assert_eq!(fixture.log(), ["requested_commit", "outcome_commit"]);
    }

    #[tokio::test]
    async fn deadline_expiry_during_capability_acquisition_prevents_preparation_and_dispatch() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.workstation.capabilities_advance.lock().unwrap() =
            Some(Duration::from_millis(201));
        let mut call = fixture.call("run_shell", br#"{"command":"true"}"#, 1);
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_millis(10_200));
        let result = fixture.service.execute_call(call).await.unwrap();
        assert_eq!(result.result_class, ToolResultClass::FileError);
        assert_eq!(result.error.unwrap().code, "timeout");
        assert_eq!(
            fixture.log(),
            ["requested_commit", "capabilities", "outcome_commit"]
        );
    }

    #[tokio::test]
    async fn deadline_expiry_during_preparation_prevents_dispatch_and_machine_action() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.preparation_advance.lock().unwrap() = Some(Duration::from_millis(201));
        let mut call = fixture.call("run_shell", br#"{"command":"true"}"#, 1);
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_millis(10_200));
        let result = fixture.service.execute_call(call).await.unwrap();
        assert_eq!(result.result_class, ToolResultClass::FileError);
        assert_eq!(result.error.unwrap().code, "timeout");
        assert_eq!(
            fixture.log(),
            [
                "requested_commit",
                "capabilities",
                "prepare",
                "outcome_commit"
            ]
        );
    }

    #[tokio::test]
    async fn active_shutdown_absolute_deadline_shortens_the_same_frozen_deadline() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let mut call = fixture.call(
            "run_shell",
            br#"{"command":"true","timeout_seconds":900}"#,
            1,
        );
        call.work_deadline = MonotonicInstant::from_elapsed(Duration::from_secs(30));
        call.shutdown_deadline = Some(MonotonicInstant::from_elapsed(Duration::from_secs(12)));
        fixture.service.execute_call(call).await.unwrap();
        let requests = fixture.workstation.execution_requests.lock().unwrap();
        assert_eq!(requests[0].timeout_ms, 2_000);
        assert_eq!(
            requests[0].deadline,
            MonotonicInstant::from_elapsed(Duration::from_secs(12))
        );
    }

    #[tokio::test]
    async fn deadline_expiring_during_dispatch_prevents_machine_handoff() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.preparation_advance.lock().unwrap() = Some(Duration::from_secs(20));
        *fixture.store.dispatch_advance.lock().unwrap() = Some(Duration::from_secs(50));
        let result = fixture
            .service
            .execute_call(fixture.call(
                "run_shell",
                br#"{"command":"true","timeout_seconds":60}"#,
                1,
            ))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Timeout);
        assert!(!fixture.log().contains(&"execute"));
        assert_eq!(
            fixture.log(),
            [
                "requested_commit",
                "capabilities",
                "prepare",
                "dispatch_commit",
                "outcome_commit"
            ]
        );
    }

    #[tokio::test]
    async fn dispatch_failure_and_outcome_failure_do_not_repeat_machine_action() {
        let dispatch_failure =
            Fixture::with_options(MockMode::ShellExitZero, false, false, true, false, false);
        assert!(
            dispatch_failure
                .service
                .execute_call(dispatch_failure.call("run_shell", br#"{"command":"true"}"#, 1))
                .await
                .is_err()
        );
        assert!(!dispatch_failure.log().contains(&"execute"));

        let outcome_failure =
            Fixture::with_options(MockMode::ShellExitZero, false, false, false, true, false);
        assert!(
            outcome_failure
                .service
                .execute_call(outcome_failure.call("run_shell", br#"{"command":"true"}"#, 1))
                .await
                .is_err()
        );
        assert_eq!(
            outcome_failure
                .log()
                .iter()
                .filter(|item| **item == "execute")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn duplicate_logical_call_is_rejected_before_a_second_dispatch() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let first = fixture.call("run_shell", br#"{"command":"true"}"#, 1);
        let source = first.source_model_invocation_id;
        fixture.service.execute_call(first).await.unwrap();

        let mut duplicate = fixture.call("run_shell", br#"{"command":"true"}"#, 1);
        duplicate.source_model_invocation_id = source;
        assert_eq!(
            fixture
                .service
                .execute_call(duplicate)
                .await
                .unwrap_err()
                .kind(),
            ToolExecutionServiceErrorKind::StateStore
        );
        assert_eq!(
            fixture
                .log()
                .iter()
                .filter(|entry| **entry == "execute")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cleanup_ambiguity_and_handler_panic_commit_outcome_unknown_without_redispatch() {
        for mode in [
            MockMode::ShellCleanupFailed,
            MockMode::ShellPanic,
            MockMode::ShellIdentityMismatch,
        ] {
            let fixture = Fixture::new(mode);
            let error = fixture
                .service
                .execute_call(fixture.call("run_shell", br#"{"command":"true"}"#, 1))
                .await
                .unwrap_err();
            assert!(matches!(
                error.kind(),
                ToolExecutionServiceErrorKind::OutcomeUnknown
                    | ToolExecutionServiceErrorKind::HandlerPanickedAfterPossibleHandoff
            ));
            assert_eq!(
                fixture
                    .log()
                    .iter()
                    .filter(|item| **item == "execute")
                    .count(),
                1
            );
            assert_eq!(fixture.log().last(), Some(&"outcome_commit"));
        }
    }

    #[test]
    fn handler_panic_before_handoff_is_caught_without_a_workstation_call() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let error = match guarded_handler_handoff(|| panic!("panic before Workstation handoff")) {
            Ok(_) => panic!("handler panic unexpectedly crossed the handoff guard"),
            Err(error) => error,
        };
        assert_eq!(
            error.kind(),
            ToolExecutionServiceErrorKind::HandlerPanickedBeforeHandoff
        );
        assert!(fixture.workstation.operation_ids.lock().unwrap().is_empty());
        assert!(fixture.log().is_empty());
    }

    #[tokio::test]
    async fn cancellation_before_intent_requested_and_active_execution_are_distinct() {
        let before = Fixture::new(MockMode::ShellExitZero);
        let (sender, receiver) = tokio::sync::watch::channel(Some(ToolCancellationNotice {
            expected_work: WorkExpectation {
                work_id: crate::domain::WorkId::generate(),
                state: WorkState::CancelRequested,
                version: ProjectionVersion::try_new(1).unwrap(),
                runtime_owner: Some(before.runtime_id),
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: Some(WorkCancellationReason::UserRequest),
            },
            reason: WorkCancellationReason::UserRequest,
        }));
        let mut call = before.call("run_shell", br#"{"command":"true"}"#, 1);
        call.cancellation = Some(receiver);
        assert_eq!(
            before.service.execute_call(call).await.unwrap_err().kind(),
            ToolExecutionServiceErrorKind::CancelledBeforeIntent
        );
        drop(sender);
        assert!(before.log().is_empty());

        let requested =
            Fixture::with_options(MockMode::ShellExitZero, false, false, false, false, true);
        let result = requested
            .service
            .execute_call(requested.call("run_shell", br#"{"command":"true"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Cancellation);
        assert_eq!(requested.log(), ["requested_commit", "outcome_commit"]);

        let after_dispatch = Fixture::new(MockMode::CancellationAfterDispatch);
        let result = after_dispatch
            .service
            .execute_call(after_dispatch.call("run_shell", br#"{"command":"true"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Cancellation);
        assert_eq!(
            after_dispatch.log(),
            [
                "requested_commit",
                "capabilities",
                "prepare",
                "dispatch_commit",
                "outcome_commit"
            ]
        );

        let active = Fixture::new(MockMode::ActiveCancellation);
        let result = active
            .service
            .execute_call(active.call("run_shell", br#"{"command":"sleep 1"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Cancellation);
        assert_eq!(
            active
                .log()
                .iter()
                .filter(|item| **item == "execute")
                .count(),
            1
        );
        assert_eq!(
            active
                .log()
                .iter()
                .filter(|item| **item == "cancel_execution")
                .count(),
            1
        );
        assert_eq!(
            active
                .workstation
                .cancelled_execution_ids
                .lock()
                .unwrap()
                .as_slice(),
            active.workstation.execution_ids.lock().unwrap().as_slice()
        );
    }

    #[tokio::test]
    async fn cancellation_between_handler_future_construction_and_first_poll_never_hands_off() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let definition = &fixture.service.registry.definitions()[1];
        let arguments = validate_arguments(definition, br#"{"command":"true"}"#).unwrap();
        let operation_id = OperationId::generate();
        let execution_id = ExecutionId::generate();
        let requested_cwd = LogicalPathReference::absolute("/workspace").unwrap();
        let prepared_cwd = PreparedCwdEvidence::new(
            ResolvedPathEvidence::try_new(
                fixture.workstation_id,
                WorkstationGeneration::try_new(1).unwrap(),
                fixture.workspace_id,
                requested_cwd.clone(),
                "/workspace",
            )
            .unwrap(),
            PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory).unwrap(),
        );
        let context = ToolExecutionContext {
            operation_id,
            execution_id,
            work_id: crate::domain::WorkId::generate(),
            workstation_id: fixture.workstation_id,
            workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
            workspace_id: fixture.workspace_id,
            requested_cwd,
            prepared_cwd,
            effective_privilege: PrivilegeMode::User,
            timeout: MonotonicDuration::from_millis(1_000),
            deadline: MonotonicInstant::from_elapsed(Duration::from_secs(30)),
            capture: ExecutionCapturePolicy {
                stdout_max_bytes: 1,
                stderr_max_bytes: 1,
            },
        };
        let future = resolve_handler(definition.handler()).invoke(
            arguments.input(),
            context,
            fixture.workstation.as_ref(),
        );
        let notice = ToolCancellationNotice {
            expected_work: WorkExpectation {
                work_id: crate::domain::WorkId::generate(),
                state: WorkState::CancelRequested,
                version: ProjectionVersion::try_new(1).unwrap(),
                runtime_owner: Some(fixture.runtime_id),
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: Some(WorkCancellationReason::UserRequest),
            },
            reason: WorkCancellationReason::UserRequest,
        };
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let mut cancellation = Some(receiver);
        sender.send_replace(Some(notice));

        let (observation, observed_notice, _) = fixture
            .service
            .await_handler(
                future,
                &mut cancellation,
                true,
                execution_id,
                fixture.workstation_id,
                WorkstationGeneration::try_new(1).unwrap(),
            )
            .await;
        assert!(matches!(
            observation,
            Err(HandlerPollFailure::CancelledBeforeHandoff)
        ));
        assert_eq!(observed_notice, Some(notice));
        assert!(fixture.workstation.execution_ids.lock().unwrap().is_empty());
        assert!(
            fixture
                .workstation
                .cancelled_execution_ids
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn large_read_is_generic_artifact_backed_without_stream_column_reuse() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let result = fixture
            .service
            .execute_call(fixture.call("read_file", br#"{"path":"large.txt"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.artifact_ids.len(), 1);
        assert!(result.truncated);
        assert_eq!(fixture.store.artifact_counts.lock().unwrap()[0], 1);
    }

    #[tokio::test]
    async fn dispatch_persists_complete_canonical_authority_evidence_without_command_content() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        let sentinel = "secret-command-sentinel";
        fixture
            .service
            .execute_call(fixture.call(
                "run_shell",
                format!(r#"{{"command":"printf '{sentinel}'"}}"#).as_bytes(),
                1,
            ))
            .await
            .unwrap();
        let evidence = fixture.store.dispatch_authority_evidence.lock().unwrap();
        assert_eq!(evidence.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&evidence[0]).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), evidence[0]);
        assert_eq!(parsed["decision"], "allow");
        assert_eq!(parsed["reason_code"], "allowed");
        assert_eq!(parsed["requested_privilege"], "user");
        assert_eq!(parsed["effective_privilege"], "user");
        assert_eq!(parsed["workstation_generation"], 1);
        assert_eq!(parsed["tool_name"], "run_shell");
        assert!(
            parsed["capabilities"]["foreground_execute"]
                .as_bool()
                .unwrap()
        );
        assert!(!evidence[0].contains(sentinel));
    }

    #[test]
    fn serialization_aware_projection_handles_adversarial_json_escaping_at_the_v3_ceiling() {
        let cases = [
            ("quotes", "\"".repeat(32_768), "\"".repeat(32_768)),
            ("backslashes", "\\".repeat(32_768), "\\".repeat(32_768)),
            ("controls", "\u{1}".repeat(32_768), "\u{1}".repeat(32_768)),
            ("unicode", "界".repeat(10_922), "é".repeat(16_384)),
            (
                "mixed",
                format!("{}{}", "\"".repeat(16_384), "\\".repeat(16_384)),
                format!("{}{}", "\u{2}".repeat(16_384), "界".repeat(5_461)),
            ),
        ];
        for (name, stdout, stderr) in cases {
            let bounded = bounded_shell_result_fields(
                BTreeMap::from([("duration_ms".to_owned(), "1".to_owned())]),
                &stdout,
                &stderr,
                ToolResultClass::Success,
                "Command completed successfully.",
            )
            .unwrap();
            let serialized = serde_json::to_vec(&json!({
                "fields": bounded.fields,
                "result_kind": "success",
                "summary": "Command completed successfully.",
                "version": 1,
            }))
            .unwrap();
            assert!(
                serialized.len() <= MAX_TOOL_RESULT_JSON_BYTES,
                "{name} serialized to {} bytes",
                serialized.len()
            );
            assert!(bounded.stdout_returned <= stdout.len() as u64);
            assert!(bounded.stderr_returned <= stderr.len() as u64);
            if name == "controls" {
                assert!(
                    bounded.stdout_returned + bounded.stderr_returned
                        < (stdout.len() + stderr.len()) as u64
                );
                assert!(serialized.len() > MAX_TOOL_RESULT_JSON_BYTES - 8_192);
            }
        }
    }

    #[tokio::test]
    async fn definite_shell_outcome_commits_when_control_output_would_escape_above_ceiling() {
        let fixture = Fixture::new(MockMode::ShellExitZero);
        *fixture.workstation.custom_streams.lock().unwrap() = Some((
            vec![1_u8; crate::ports::workstation::EXECUTION_STREAM_PROJECTION_BYTES],
            vec![2_u8; crate::ports::workstation::EXECUTION_STREAM_PROJECTION_BYTES],
        ));
        let result = fixture
            .service
            .execute_call(fixture.call("run_shell", br#"{"command":"true"}"#, 1))
            .await
            .unwrap();
        assert_eq!(result.result_class, ToolResultClass::Success);
        assert_eq!(result.artifact_ids.len(), 2);
        assert!(result.truncated);
        assert_eq!(
            fixture.store.outcome_states.lock().unwrap().as_slice(),
            [ToolExecutionState::Completed]
        );
        assert!(fixture.store.result_json_lengths.lock().unwrap()[0] <= MAX_TOOL_RESULT_JSON_BYTES);
        let counts = fixture.store.stream_counts.lock().unwrap();
        let (stdout, stderr) = counts[0];
        for counts in [stdout.unwrap(), stderr.unwrap()] {
            assert_eq!(
                counts.omitted.get(),
                counts.observed.get() - counts.returned_inline.get()
            );
            assert_eq!(counts.captured.get(), 32_768);
        }
    }

    #[tokio::test]
    async fn artifact_finalization_failure_after_handoff_is_durable_outcome_unknown() {
        let mut fixture = Fixture::new(MockMode::ShellExitZero);
        fixture.service.artifact_store = Arc::new(FailingArtifactStore);
        let error = fixture
            .service
            .execute_call(fixture.call("read_file", br#"{"path":"large.txt"}"#, 1))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ToolExecutionServiceErrorKind::Artifact);
        assert_eq!(
            fixture.store.outcome_states.lock().unwrap().as_slice(),
            [ToolExecutionState::OutcomeUnknown]
        );
        assert_eq!(fixture.store.artifact_counts.lock().unwrap()[0], 0);
        assert_eq!(fixture.log().last(), Some(&"outcome_commit"));
    }

    #[test]
    fn invalid_runtime_limit_composition_fails_closed() {
        let mut limits = test_limits();
        limits.run_shell_max_timeout_ms = HARD_EXECUTION_TIMEOUT_MS + 1;
        assert_eq!(
            limits.validate().unwrap_err().kind(),
            ToolExecutionServiceErrorKind::InvalidComposition
        );
    }

    #[test]
    fn artifact_error_class_is_constructible_without_raw_storage_detail() {
        assert_eq!(
            ArtifactStoreError::new(ArtifactStoreErrorKind::Storage).kind(),
            ArtifactStoreErrorKind::Storage
        );
    }
}
