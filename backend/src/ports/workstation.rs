//! Dependency-neutral boundary for low-level workstation operations.

use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use crate::domain::{
    CanonicalByteCount, Certainty, ExecutionId, LogicalPathReference, MonotonicDuration,
    NormalizedError, OperationId, PrivilegeMode, ResolvedPathEvidence, Retryability, Sha256Digest,
    UtcTimestamp, WorkspaceId, WorkstationCapabilities, WorkstationGeneration, WorkstationId,
};
use crate::ports::clock::MonotonicInstant;

/// Default maximum accepted by the model-facing read-file request constructor.
pub const DEFAULT_FILE_READ_MAX_BYTES: u64 = 1_048_576;

/// Absolute LocalWorkstation read ceiling.
pub const HARD_FILE_READ_MAX_BYTES: u64 = 8_388_608;

/// Boxed future used by the Workstation port without an async-trait dependency.
pub type WorkstationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WorkstationError>> + Send + 'a>>;

/// Stable machine-boundary failure classes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkstationErrorKind {
    WorkstationUnavailable,
    GenerationMismatch,
    WorkspaceNotFound,
    InvalidPath,
    NotFound,
    PermissionDenied,
    BinaryContent,
    FileTooLarge,
    ChangedDuringRead,
    UnsupportedCapability,
    Timeout,
    Cancelled,
    IoError,
    InternalWorkstationError,
}

impl WorkstationErrorKind {
    /// Returns the exact stable internal code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::WorkstationUnavailable => "workstation_unavailable",
            Self::GenerationMismatch => "generation_mismatch",
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::InvalidPath => "invalid_path",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::BinaryContent => "binary_content",
            Self::FileTooLarge => "file_too_large",
            Self::ChangedDuringRead => "changed_during_read",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::IoError => "io_error",
            Self::InternalWorkstationError => "internal_workstation_error",
        }
    }

    /// Returns conservative advisory retryability without authorizing a retry.
    #[must_use]
    pub const fn retryability(self) -> Retryability {
        match self {
            Self::Timeout | Self::ChangedDuringRead => Retryability::Bounded,
            Self::WorkstationUnavailable | Self::PermissionDenied => Retryability::OperatorAction,
            Self::GenerationMismatch
            | Self::WorkspaceNotFound
            | Self::InvalidPath
            | Self::NotFound
            | Self::BinaryContent
            | Self::FileTooLarge
            | Self::UnsupportedCapability
            | Self::Cancelled
            | Self::IoError
            | Self::InternalWorkstationError => Retryability::Never,
        }
    }
}

/// Safe Workstation error with optional bounded file evidence and no raw path/I/O detail.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkstationError {
    kind: WorkstationErrorKind,
    byte_length: Option<CanonicalByteCount>,
    sha256: Option<Sha256Digest>,
}

impl WorkstationError {
    #[must_use]
    pub const fn new(kind: WorkstationErrorKind) -> Self {
        Self {
            kind,
            byte_length: None,
            sha256: None,
        }
    }

    #[must_use]
    pub const fn with_file_evidence(
        kind: WorkstationErrorKind,
        byte_length: CanonicalByteCount,
        sha256: Option<Sha256Digest>,
    ) -> Self {
        Self {
            kind,
            byte_length: Some(byte_length),
            sha256,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> WorkstationErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn retryability(&self) -> Retryability {
        self.kind.retryability()
    }

    #[must_use]
    pub const fn byte_length(&self) -> Option<CanonicalByteCount> {
        self.byte_length
    }

    #[must_use]
    pub const fn sha256(&self) -> Option<Sha256Digest> {
        self.sha256
    }

    /// Projects to the repository-wide safe normalized envelope.
    #[must_use]
    pub const fn normalized(&self) -> NormalizedError {
        NormalizedError::workstation_classified(self.retryability(), Certainty::Definite, None)
    }
}

impl Display for WorkstationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.kind.code())
    }
}

impl Debug for WorkstationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkstationError")
            .field("kind", &self.kind)
            .field("retryability", &self.retryability())
            .field("byte_length", &self.byte_length)
            .field("sha256", &self.sha256)
            .finish()
    }
}

impl std::error::Error for WorkstationError {}

/// Explicit identity for a capability observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilitiesRequest {
    pub operation_id: OperationId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
}

/// Capability observation paired to its caller operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitiesResult {
    pub operation_id: OperationId,
    pub capabilities: WorkstationCapabilities,
}

/// Complete bounded text-file request.
#[derive(Clone, Eq, PartialEq)]
pub struct FileReadRequest {
    pub operation_id: OperationId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    pub path: LogicalPathReference,
    pub max_bytes: u64,
    pub deadline: MonotonicInstant,
}

impl Debug for FileReadRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileReadRequest")
            .field("operation_id", &self.operation_id)
            .field("workstation_id", &self.workstation_id)
            .field("expected_generation", &self.expected_generation)
            .field("workspace_id", &self.workspace_id)
            .field("path", &self.path)
            .field("max_bytes", &self.max_bytes)
            .field("deadline", &self.deadline)
            .finish()
    }
}

/// The only successful Stage 12 file type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkstationFileType {
    Regular,
}

/// The only successful Stage 12 text encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileEncoding {
    Utf8,
}

/// Exact complete Stage 12 text-file observation.
#[derive(Clone, Eq, PartialEq)]
pub struct FileReadResult {
    pub operation_id: OperationId,
    pub requested_path: LogicalPathReference,
    pub resolved_path: ResolvedPathEvidence,
    pub file_type: WorkstationFileType,
    pub byte_length: CanonicalByteCount,
    pub modified_at: Option<UtcTimestamp>,
    pub encoding: FileEncoding,
    pub sha256: Sha256Digest,
    pub text: String,
    pub truncated: bool,
}

impl Debug for FileReadResult {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileReadResult")
            .field("operation_id", &self.operation_id)
            .field("requested_path", &self.requested_path)
            .field("resolved_path", &self.resolved_path)
            .field("file_type", &self.file_type)
            .field("byte_length", &self.byte_length)
            .field("modified_at", &self.modified_at)
            .field("encoding", &self.encoding)
            .field("sha256", &self.sha256)
            .field("text", &"[REDACTED]")
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// Clean child-environment entry reserved for Stage 13 execution.
#[derive(Clone, Eq, PartialEq)]
pub struct ExecutionEnvironmentVariable {
    pub name: String,
    pub value: String,
}

/// Dependency-neutral child standard-input policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionStdinPolicy {
    Closed,
}

/// Independent bounded stdout/stderr capture policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionCapturePolicy {
    pub stdout_max_bytes: u64,
    pub stderr_max_bytes: u64,
}

/// Required foreground descendant cleanup policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionCleanupPolicy {
    ProcessGroupAndCgroup,
}

/// Canonical Stage 13-shaped execution request; unsupported by Stage 12.
#[derive(Clone, Eq, PartialEq)]
pub struct ExecutionRequest {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    pub command: String,
    pub requested_cwd: LogicalPathReference,
    pub effective_privilege: PrivilegeMode,
    pub environment: Vec<ExecutionEnvironmentVariable>,
    pub stdin: ExecutionStdinPolicy,
    pub timeout: MonotonicDuration,
    pub deadline: MonotonicInstant,
    pub capture: ExecutionCapturePolicy,
    pub cleanup: ExecutionCleanupPolicy,
}

/// Closed execution terminal classes reserved for Stage 13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionResultKind {
    Exited,
    Signalled,
    TimedOut,
    Cancelled,
    SpawnFailed,
    CleanupFailed,
}

/// Dependency-neutral execution result reserved for Stage 13.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub result_kind: ExecutionResultKind,
    pub exit_code: Option<i64>,
    pub terminating_signal: Option<i64>,
    pub duration: MonotonicDuration,
    pub cleanup_confirmed: bool,
}

/// Explicit identity for an execution inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionInspectionRequest {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
}

/// Closed inspection status reserved for Stage 13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionInspectionState {
    Running,
    Terminal,
}

/// Dependency-neutral inspection result reserved for Stage 13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionInspection {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub state: ExecutionInspectionState,
}

/// Explicit identity for execution cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionCancellationRequest {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
}

/// Closed cancellation status reserved for Stage 13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionCancellationState {
    Confirmed,
    AlreadyTerminal,
    NotFound,
    CleanupUnconfirmed,
}

/// Dependency-neutral cancellation result reserved for Stage 13.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancellationResult {
    pub operation_id: OperationId,
    pub execution_id: ExecutionId,
    pub state: ExecutionCancellationState,
}

/// Replaceable low-level machine boundary with exactly five normative methods.
pub trait Workstation: Send + Sync {
    fn capabilities(
        &self,
        request: CapabilitiesRequest,
    ) -> WorkstationFuture<'_, CapabilitiesResult>;

    fn read_file(&self, request: FileReadRequest) -> WorkstationFuture<'_, FileReadResult>;

    fn execute(&self, request: ExecutionRequest) -> WorkstationFuture<'_, ExecutionResult>;

    fn inspect_execution(
        &self,
        request: ExecutionInspectionRequest,
    ) -> WorkstationFuture<'_, ExecutionInspection>;

    fn cancel_execution(
        &self,
        request: ExecutionCancellationRequest,
    ) -> WorkstationFuture<'_, CancellationResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        LogicalPathReference, WorkspaceCapabilityRef, WorkstationCapabilitiesInput,
        WorkstationCapabilityFlags, WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits,
    };

    struct FakeWorkstation {
        capabilities: WorkstationCapabilities,
    }

    impl Workstation for FakeWorkstation {
        fn capabilities(
            &self,
            request: CapabilitiesRequest,
        ) -> WorkstationFuture<'_, CapabilitiesResult> {
            let capabilities = self.capabilities.clone();
            Box::pin(async move {
                Ok(CapabilitiesResult {
                    operation_id: request.operation_id,
                    capabilities,
                })
            })
        }

        fn read_file(&self, request: FileReadRequest) -> WorkstationFuture<'_, FileReadResult> {
            Box::pin(async move {
                let bytes = b"fake";
                Ok(FileReadResult {
                    operation_id: request.operation_id,
                    requested_path: request.path.clone(),
                    resolved_path: ResolvedPathEvidence::try_new(
                        request.workstation_id,
                        request.expected_generation,
                        request.workspace_id,
                        request.path,
                        "/remote/fake",
                    )
                    .unwrap(),
                    file_type: WorkstationFileType::Regular,
                    byte_length: CanonicalByteCount::try_new(bytes.len() as u64).unwrap(),
                    modified_at: None,
                    encoding: FileEncoding::Utf8,
                    sha256: Sha256Digest::hash_bytes(bytes),
                    text: "fake".into(),
                    truncated: false,
                })
            })
        }

        fn execute(&self, _request: ExecutionRequest) -> WorkstationFuture<'_, ExecutionResult> {
            Box::pin(async {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            })
        }

        fn inspect_execution(
            &self,
            _request: ExecutionInspectionRequest,
        ) -> WorkstationFuture<'_, ExecutionInspection> {
            Box::pin(async {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            })
        }

        fn cancel_execution(
            &self,
            _request: ExecutionCancellationRequest,
        ) -> WorkstationFuture<'_, CancellationResult> {
            Box::pin(async {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            })
        }
    }

    fn capabilities() -> WorkstationCapabilities {
        let workstation_id = WorkstationId::generate();
        WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
            workstation_id,
            generation: WorkstationGeneration::try_new(1).unwrap(),
            cpu_architecture: "remote-arch".into(),
            os_release: "remote-os".into(),
            default_shell: LogicalPathReference::absolute("/bin/sh").unwrap(),
            flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
                filesystem_read: true,
                foreground_execute: false,
                cancel_execution: false,
                inspect_execution: false,
                privilege_user: true,
                privilege_administrative: false,
                process_group_cleanup: false,
                cgroup_cleanup: false,
            }),
            limits: WorkstationCapabilityLimits::try_new(0, 0, 0).unwrap(),
            workspaces: vec![
                WorkspaceCapabilityRef::try_new(
                    WorkspaceId::generate(),
                    LogicalPathReference::absolute("/workspace").unwrap(),
                )
                .unwrap(),
            ],
        })
        .unwrap()
    }

    #[tokio::test]
    async fn fake_proves_the_port_has_no_local_descriptor_or_path_handle_requirement() {
        let expected = capabilities();
        let fake: Box<dyn Workstation> = Box::new(FakeWorkstation {
            capabilities: expected.clone(),
        });
        let result = fake
            .capabilities(CapabilitiesRequest {
                operation_id: OperationId::generate(),
                workstation_id: expected.workstation_id(),
                expected_generation: expected.generation(),
            })
            .await
            .unwrap();
        assert_eq!(result.capabilities, expected);
    }

    #[test]
    fn stable_error_codes_and_retryability_are_exact_and_normalized() {
        let cases = [
            (
                WorkstationErrorKind::WorkstationUnavailable,
                "workstation_unavailable",
                Retryability::OperatorAction,
            ),
            (
                WorkstationErrorKind::GenerationMismatch,
                "generation_mismatch",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::WorkspaceNotFound,
                "workspace_not_found",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::InvalidPath,
                "invalid_path",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::NotFound,
                "not_found",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::PermissionDenied,
                "permission_denied",
                Retryability::OperatorAction,
            ),
            (
                WorkstationErrorKind::BinaryContent,
                "binary_content",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::FileTooLarge,
                "file_too_large",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::Timeout,
                "timeout",
                Retryability::Bounded,
            ),
            (
                WorkstationErrorKind::ChangedDuringRead,
                "changed_during_read",
                Retryability::Bounded,
            ),
            (
                WorkstationErrorKind::UnsupportedCapability,
                "unsupported_capability",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::Cancelled,
                "cancelled",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::IoError,
                "io_error",
                Retryability::Never,
            ),
            (
                WorkstationErrorKind::InternalWorkstationError,
                "internal_workstation_error",
                Retryability::Never,
            ),
        ];
        for (kind, code, retryability) in cases {
            let error = WorkstationError::new(kind);
            assert_eq!(error.to_string(), code);
            assert_eq!(error.retryability(), retryability);
            assert_eq!(error.normalized().retryability(), retryability);
        }
    }

    #[test]
    fn operation_and_execution_ids_are_distinct_uuidv7_domain_types() {
        use std::any::TypeId;

        let operation_id = OperationId::generate();
        let execution_id = ExecutionId::generate();
        assert_eq!(
            uuid::Uuid::parse_str(&operation_id.to_string())
                .unwrap()
                .get_version_num(),
            7
        );
        assert_eq!(
            uuid::Uuid::parse_str(&execution_id.to_string())
                .unwrap()
                .get_version_num(),
            7
        );
        assert_ne!(TypeId::of::<OperationId>(), TypeId::of::<ExecutionId>());
        assert_ne!(operation_id.to_string(), execution_id.to_string());
    }
}
