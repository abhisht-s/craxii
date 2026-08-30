//! Thin typed V0 handlers; persistence, authority, and retry are intentionally absent.

use std::future::Future;
use std::pin::Pin;

use crate::application::tool_registry::{HandlerIdentity, ValidatedToolInput};
use crate::domain::{
    ExecutionId, LogicalPathReference, MonotonicDuration, OperationId, PrivilegeMode, WorkId,
    WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::clock::MonotonicInstant;
use crate::ports::workstation::{
    ExecutionCapturePolicy, ExecutionCleanupPolicy, ExecutionRequest, ExecutionResult,
    ExecutionStdinPolicy, FileReadRequest, FileReadResult, Workstation, WorkstationError,
};
use crate::ports::workstation_preparation::PreparedCwdEvidence;

/// Boxed handler future without an async-trait dependency.
pub type ToolHandlerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolHandlerObservation, WorkstationError>> + Send + 'a>>;

/// Trusted service-injected execution facts that model arguments cannot override.
#[derive(Clone, Eq, PartialEq)]
pub struct ToolExecutionContext {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub work_id: WorkId,
    pub workstation_id: WorkstationId,
    pub workstation_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    pub requested_cwd: LogicalPathReference,
    pub prepared_cwd: PreparedCwdEvidence,
    pub effective_privilege: PrivilegeMode,
    pub timeout: MonotonicDuration,
    pub deadline: MonotonicInstant,
    pub capture: ExecutionCapturePolicy,
}

impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolExecutionContext")
            .field("operation_id", &self.operation_id)
            .field("execution_id", &self.execution_id)
            .field("work_id", &self.work_id)
            .field("workstation_id", &self.workstation_id)
            .field("workstation_generation", &self.workstation_generation)
            .field("workspace_id", &self.workspace_id)
            .field("requested_cwd", &"[REDACTED]")
            .field("prepared_cwd", &"[REDACTED]")
            .field("effective_privilege", &self.effective_privilege)
            .field("timeout", &self.timeout)
            .field("deadline", &self.deadline)
            .field("capture", &self.capture)
            .finish()
    }
}

/// Closed normalized handler observations before service-owned result mapping.
#[derive(Debug)]
pub enum ToolHandlerObservation {
    ReadFile(Box<FileReadResult>),
    RunShell(Box<ExecutionResult>),
}

/// Typed handler boundary. Implementations receive no store, journal, SQL, provider, or secrets.
pub trait ToolHandler: Send + Sync {
    fn invoke<'a>(
        &'a self,
        input: &'a ValidatedToolInput,
        context: ToolExecutionContext,
        workstation: &'a dyn Workstation,
    ) -> ToolHandlerFuture<'a>;
}

#[derive(Debug)]
struct ReadFileHandler;

#[derive(Debug)]
struct RunShellHandler;

static READ_FILE_HANDLER: ReadFileHandler = ReadFileHandler;
static RUN_SHELL_HANDLER: RunShellHandler = RunShellHandler;

pub(crate) fn resolve_handler(identity: HandlerIdentity) -> &'static dyn ToolHandler {
    match identity {
        HandlerIdentity::ReadFile => &READ_FILE_HANDLER,
        HandlerIdentity::RunShell => &RUN_SHELL_HANDLER,
    }
}

impl ToolHandler for ReadFileHandler {
    fn invoke<'a>(
        &'a self,
        input: &'a ValidatedToolInput,
        context: ToolExecutionContext,
        workstation: &'a dyn Workstation,
    ) -> ToolHandlerFuture<'a> {
        let input = match input {
            ValidatedToolInput::ReadFile(input) => input,
            ValidatedToolInput::RunShell(_) => return invalid_handler_input(),
        };
        let request = FileReadRequest {
            operation_id: context.operation_id,
            workstation_id: context.workstation_id,
            expected_generation: context.workstation_generation,
            workspace_id: context.workspace_id,
            path: input.path().clone(),
            max_bytes: input.max_bytes(),
            deadline: context.deadline,
        };
        Box::pin(async move {
            workstation
                .read_file(request)
                .await
                .map(Box::new)
                .map(ToolHandlerObservation::ReadFile)
        })
    }
}

impl ToolHandler for RunShellHandler {
    fn invoke<'a>(
        &'a self,
        input: &'a ValidatedToolInput,
        context: ToolExecutionContext,
        workstation: &'a dyn Workstation,
    ) -> ToolHandlerFuture<'a> {
        let input = match input {
            ValidatedToolInput::RunShell(input) => input,
            ValidatedToolInput::ReadFile(_) => return invalid_handler_input(),
        };
        let request = ExecutionRequest {
            operation_id: context.operation_id,
            execution_id: context.execution_id,
            work_id: context.work_id,
            workstation_id: context.workstation_id,
            expected_generation: context.workstation_generation,
            workspace_id: context.workspace_id,
            command: input.command().to_owned(),
            requested_cwd: context.requested_cwd,
            prepared_cwd: context.prepared_cwd,
            effective_privilege: context.effective_privilege,
            stdin: ExecutionStdinPolicy::Closed,
            timeout: context.timeout,
            deadline: context.deadline,
            capture: context.capture,
            cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
        };
        Box::pin(async move {
            workstation
                .execute(request)
                .await
                .map(Box::new)
                .map(ToolHandlerObservation::RunShell)
        })
    }
}

fn invalid_handler_input<'a>() -> ToolHandlerFuture<'a> {
    Box::pin(async {
        Err(WorkstationError::new(
            crate::ports::workstation::WorkstationErrorKind::InternalWorkstationError,
        ))
    })
}
