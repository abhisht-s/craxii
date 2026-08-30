//! Production local implementation of the dependency-neutral Workstation boundary.

use std::fmt::{Debug, Formatter};
use std::fs::{Metadata, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
#[cfg(target_os = "linux")]
use std::path::Component;

use time::OffsetDateTime;

mod execution;

use execution::{ExecutionCwd, ExecutionRuntime, ExecutionRuntimeConfig};

use crate::domain::{
    CanonicalByteCount, LogicalPathKind, LogicalPathReference, PrivilegeMode, ResolvedPathEvidence,
    Sha256Digest, UtcTimestamp, WorkspaceCapabilityRef, WorkspaceId, WorkspaceIdentity,
    WorkstationCapabilities, WorkstationCapabilitiesInput, WorkstationCapabilityFlags,
    WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits, WorkstationGeneration,
    WorkstationId, WorkstationIdentity,
};
use crate::ports::artifact_store::ArtifactStore;
use crate::ports::clock::Clock;
use crate::ports::workstation::{
    CancellationResult, CapabilitiesRequest, CapabilitiesResult, ExecutionCancellationRequest,
    ExecutionInspection, ExecutionInspectionRequest, ExecutionRequest, ExecutionResult,
    FileEncoding, FileReadRequest, FileReadResult, HARD_EXECUTION_STREAM_CAPTURE_BYTES,
    HARD_EXECUTION_TIMEOUT_MS, HARD_FILE_READ_MAX_BYTES, Workstation, WorkstationError,
    WorkstationErrorKind, WorkstationFileType, WorkstationFuture,
};
use crate::ports::workstation_preparation::{
    PreparedCwdEvidence, PreparedCwdObjectIdentity, PreparedCwdObjectType,
    RequiredWorkstationCapability, WorkstationPreparation, WorkstationPreparationFuture,
    WorkstationPreparationRequest, WorkstationPreparationResult,
};

const READ_BUFFER_BYTES: usize = 16_384;

#[cfg(test)]
type ReadHook = Arc<dyn Fn(ReadHookPoint, &Path) + Send + Sync>;

/// Explicit construction dependencies and Stage 13 host policy.
pub struct LocalWorkstationOptions {
    pub default_shell: LogicalPathReference,
    pub configured_workspace_root: PathBuf,
    pub read_hard_limit: u64,
    pub artifact_store: Arc<dyn ArtifactStore>,
    pub administrative_enabled: bool,
    pub delegated_cgroup_root: Option<PathBuf>,
    pub clock: Arc<dyn Clock>,
}

/// The sole production adapter for model-directed local machine primitives.
#[derive(Clone)]
pub struct LocalWorkstation {
    workstation_id: WorkstationId,
    generation: WorkstationGeneration,
    workspace_id: WorkspaceId,
    logical_workspace_root: LogicalPathReference,
    resolved_workspace_root: PathBuf,
    read_hard_limit: u64,
    capabilities: WorkstationCapabilities,
    clock: Arc<dyn Clock>,
    execution: Arc<ExecutionRuntime>,
    #[cfg(test)]
    read_hook: Option<ReadHook>,
}

impl LocalWorkstation {
    /// Binds verified durable identity to one explicit configured local workspace root.
    pub fn new(
        workstation: &WorkstationIdentity,
        workspace: &WorkspaceIdentity,
        options: LocalWorkstationOptions,
    ) -> Result<Self, WorkstationError> {
        let LocalWorkstationOptions {
            default_shell,
            configured_workspace_root,
            read_hard_limit,
            artifact_store,
            administrative_enabled,
            delegated_cgroup_root,
            clock,
        } = options;
        if workspace.workstation_id() != workstation.workstation_id()
            || read_hard_limit == 0
            || read_hard_limit > HARD_FILE_READ_MAX_BYTES
        {
            return Err(WorkstationError::new(
                WorkstationErrorKind::InternalWorkstationError,
            ));
        }
        if !configured_workspace_root.is_absolute() {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }

        let resolved_workspace_root =
            std::fs::canonicalize(&configured_workspace_root).map_err(map_constructor_error)?;
        if !resolved_workspace_root.is_absolute()
            || resolved_workspace_root
                .to_str()
                .is_none_or(|path| path.len() > crate::domain::MAX_LOGICAL_PATH_BYTES)
        {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        let root_metadata =
            std::fs::metadata(&resolved_workspace_root).map_err(map_constructor_error)?;
        if !root_metadata.is_dir() {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }

        let support = observe_execution_support(
            Path::new(default_shell.canonical()),
            administrative_enabled,
            delegated_cgroup_root.as_deref(),
        );
        let capabilities = stage13_capabilities(
            workstation.workstation_id(),
            workstation.generation(),
            workspace.workspace_id(),
            workspace.logical_root().clone(),
            default_shell,
            support.clone(),
        )?;
        let execution = ExecutionRuntime::new(
            artifact_store,
            Arc::clone(&clock),
            ExecutionRuntimeConfig {
                shell: PathBuf::from(capabilities.default_shell().canonical()),
                administrative_capable: support.administrative,
                cgroup_root: support.cgroup_root,
            },
        );

        Ok(Self {
            workstation_id: workstation.workstation_id(),
            generation: workstation.generation(),
            workspace_id: workspace.workspace_id(),
            logical_workspace_root: workspace.logical_root().clone(),
            resolved_workspace_root,
            read_hard_limit,
            capabilities,
            clock,
            execution,
            #[cfg(test)]
            read_hook: None,
        })
    }

    #[must_use]
    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }

    #[must_use]
    pub const fn generation(&self) -> WorkstationGeneration {
        self.generation
    }

    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    #[must_use]
    pub const fn logical_workspace_root(&self) -> &LogicalPathReference {
        &self.logical_workspace_root
    }

    #[must_use]
    pub fn capabilities_snapshot(&self) -> &WorkstationCapabilities {
        &self.capabilities
    }

    fn validate_identity(
        &self,
        workstation_id: WorkstationId,
        expected_generation: WorkstationGeneration,
    ) -> Result<(), WorkstationError> {
        if workstation_id != self.workstation_id {
            return Err(WorkstationError::new(
                WorkstationErrorKind::WorkstationUnavailable,
            ));
        }
        if expected_generation != self.generation {
            return Err(WorkstationError::new(
                WorkstationErrorKind::GenerationMismatch,
            ));
        }
        Ok(())
    }

    fn validate_workspace(&self, workspace_id: WorkspaceId) -> Result<(), WorkstationError> {
        if workspace_id != self.workspace_id {
            return Err(WorkstationError::new(
                WorkstationErrorKind::WorkspaceNotFound,
            ));
        }
        Ok(())
    }

    fn deadline_expired(&self, deadline: crate::ports::clock::MonotonicInstant) -> bool {
        self.clock.monotonic_now() >= deadline
    }

    /// Shared existing-target resolver for reads and future local workstation operations.
    fn resolve_existing_path(
        &self,
        requested: &LogicalPathReference,
    ) -> Result<ResolvedTarget, WorkstationError> {
        let candidate = match requested.kind() {
            LogicalPathKind::WorkspaceRelative => {
                self.resolved_workspace_root.join(requested.canonical())
            }
            LogicalPathKind::Absolute => PathBuf::from(requested.canonical()),
        };
        let resolved = std::fs::canonicalize(candidate).map_err(map_path_error)?;
        if is_recognized_pseudo_filesystem(&resolved) {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        let resolved_text = resolved
            .to_str()
            .ok_or_else(|| WorkstationError::new(WorkstationErrorKind::InvalidPath))?;
        let evidence = ResolvedPathEvidence::try_new(
            self.workstation_id,
            self.generation,
            self.workspace_id,
            requested.clone(),
            resolved_text,
        )
        .map_err(|_| WorkstationError::new(WorkstationErrorKind::InvalidPath))?;
        Ok(ResolvedTarget {
            physical_path: resolved,
            evidence,
        })
    }

    fn read_blocking(&self, request: FileReadRequest) -> Result<FileReadResult, WorkstationError> {
        if self.deadline_expired(request.deadline) {
            return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
        }
        let target = self.resolve_existing_path(&request.path)?;
        if self.deadline_expired(request.deadline) {
            return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
        }

        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK);
        let mut file = options
            .open(&target.physical_path)
            .map_err(map_path_error)?;
        let initial_metadata = file.metadata().map_err(map_io_error)?;
        ensure_regular_file(&initial_metadata)?;
        let initial = MetadataSnapshot::capture(&initial_metadata);
        if initial.len > request.max_bytes {
            let length = CanonicalByteCount::try_new(initial.len).map_err(|_| {
                WorkstationError::new(WorkstationErrorKind::InternalWorkstationError)
            })?;
            return Err(WorkstationError::with_file_evidence(
                WorkstationErrorKind::FileTooLarge,
                length,
                None,
            ));
        }

        #[cfg(test)]
        self.fire_read_hook(ReadHookPoint::AfterOpenBeforeRead, &target.physical_path);

        if self.deadline_expired(request.deadline) {
            return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
        }
        let capacity = usize::try_from(initial.len)
            .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))?;
        let mut bytes = Vec::with_capacity(capacity);
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        #[cfg(test)]
        let mut first_chunk = true;
        loop {
            if self.deadline_expired(request.deadline) {
                return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
            }
            let count = file.read(&mut buffer).map_err(map_io_error)?;
            if count == 0 {
                break;
            }
            let next_length = bytes.len().checked_add(count).ok_or_else(|| {
                WorkstationError::new(WorkstationErrorKind::InternalWorkstationError)
            })?;
            if next_length as u64 > request.max_bytes || next_length as u64 > self.read_hard_limit {
                return Err(WorkstationError::new(
                    WorkstationErrorKind::ChangedDuringRead,
                ));
            }
            bytes.extend_from_slice(&buffer[..count]);
            #[cfg(test)]
            if first_chunk {
                self.fire_read_hook(ReadHookPoint::AfterFirstChunk, &target.physical_path);
            }
            #[cfg(test)]
            {
                first_chunk = false;
            }
        }

        let final_metadata = file.metadata().map_err(map_io_error)?;
        ensure_regular_file(&final_metadata)?;
        let final_snapshot = MetadataSnapshot::capture(&final_metadata);
        if initial != final_snapshot || bytes.len() as u64 != initial.len {
            return Err(WorkstationError::new(
                WorkstationErrorKind::ChangedDuringRead,
            ));
        }
        if self.deadline_expired(request.deadline) {
            return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
        }

        let byte_length = CanonicalByteCount::try_new(bytes.len() as u64)
            .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))?;
        let sha256 = Sha256Digest::hash_bytes(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| {
            WorkstationError::with_file_evidence(
                WorkstationErrorKind::BinaryContent,
                byte_length,
                Some(sha256),
            )
        })?;

        Ok(FileReadResult {
            operation_id: request.operation_id,
            requested_path: request.path,
            resolved_path: target.evidence,
            file_type: WorkstationFileType::Regular,
            byte_length,
            modified_at: initial.modified_at,
            encoding: FileEncoding::Utf8,
            sha256,
            text,
            truncated: false,
        })
    }

    fn prepare_committed_execution_cwd(
        &self,
        request: &ExecutionRequest,
    ) -> Result<ExecutionCwd, WorkstationError> {
        let committed = request.prepared_cwd.resolved_cwd();
        if committed.workstation_id() != request.workstation_id
            || committed.workstation_generation() != request.expected_generation
            || committed.workspace_id() != request.workspace_id
            || committed.requested_path() != &request.requested_cwd
        {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }

        let current = self.resolve_execution_cwd_evidence(&request.requested_cwd)?;
        if &current != committed {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }

        let committed_path = PathBuf::from(committed.resolved_absolute_path());
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW);
        let directory = options.open(&committed_path).map_err(map_path_error)?;
        let metadata = directory.metadata().map_err(map_io_error)?;
        if !metadata.is_dir() {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        let opened_identity = prepared_cwd_object_identity(&metadata)?;
        if opened_identity != request.prepared_cwd.object_identity() {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        Ok(ExecutionCwd {
            directory,
            evidence: committed.clone(),
        })
    }

    fn prepare_execution_cwd(
        &self,
        requested: &LogicalPathReference,
    ) -> Result<PreparedCwdEvidence, WorkstationError> {
        let target = self.resolve_existing_path(requested)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW);
        let directory = options
            .open(&target.physical_path)
            .map_err(map_path_error)?;
        let metadata = directory.metadata().map_err(map_io_error)?;
        if !metadata.is_dir() {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        Ok(PreparedCwdEvidence::new(
            target.evidence,
            prepared_cwd_object_identity(&metadata)?,
        ))
    }

    fn resolve_execution_cwd_evidence(
        &self,
        requested: &LogicalPathReference,
    ) -> Result<ResolvedPathEvidence, WorkstationError> {
        let target = self.resolve_existing_path(requested)?;
        if !std::fs::metadata(&target.physical_path)
            .map_err(map_path_error)?
            .is_dir()
        {
            return Err(WorkstationError::new(WorkstationErrorKind::InvalidPath));
        }
        Ok(target.evidence)
    }

    /// Closes admission and propagates the one original Stage 10 shutdown deadline.
    pub fn begin_execution_shutdown(&self, deadline: tokio::time::Instant) {
        self.execution.begin_shutdown(deadline);
    }

    /// Reaps, drains, verifies, and joins every owned execution under the Stage 10 deadline.
    pub async fn shutdown_executions_before(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkstationError> {
        self.execution.shutdown_before(deadline).await
    }

    #[cfg(test)]
    fn with_read_hook(
        mut self,
        hook: impl Fn(ReadHookPoint, &Path) + Send + Sync + 'static,
    ) -> Self {
        self.read_hook = Some(Arc::new(hook));
        self
    }

    #[cfg(test)]
    fn with_execution_shell(mut self, shell: PathBuf) -> Self {
        Arc::get_mut(&mut self.execution)
            .expect("test construction has one execution-runtime owner")
            .set_shell_for_test(shell);
        self
    }

    #[cfg(test)]
    fn set_leader_observer(&mut self, observer: Arc<dyn execution::LeaderObserver>) {
        Arc::get_mut(&mut self.execution)
            .expect("test construction has one execution-runtime owner")
            .set_leader_observer_for_test(observer);
    }

    #[cfg(test)]
    fn with_execution_gate(
        self,
        point: execution::ExecutionTestPoint,
        arrived: Arc<tokio::sync::Barrier>,
        release: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.execution
            .set_execution_gate_for_test(point, arrived, release);
        self
    }

    #[cfg(test)]
    fn execution_lifecycle_events(&self) -> Vec<&'static str> {
        self.execution.lifecycle_events()
    }

    #[cfg(test)]
    fn fire_read_hook(&self, point: ReadHookPoint, path: &Path) {
        if let Some(hook) = &self.read_hook {
            hook(point, path);
        }
    }
}

impl Debug for LocalWorkstation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalWorkstation")
            .field("workstation_id", &self.workstation_id)
            .field("generation", &self.generation)
            .field("workspace_id", &self.workspace_id)
            .field("logical_workspace_root", &"[REDACTED]")
            .field("resolved_workspace_root", &"[REDACTED]")
            .field("read_hard_limit", &self.read_hard_limit)
            .field("execution_runtime", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Workstation for LocalWorkstation {
    fn capabilities(
        &self,
        request: CapabilitiesRequest,
    ) -> WorkstationFuture<'_, CapabilitiesResult> {
        let result = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .map(|()| CapabilitiesResult {
                operation_id: request.operation_id,
                capabilities: self.capabilities.clone(),
            });
        Box::pin(async move { result })
    }

    fn read_file(&self, request: FileReadRequest) -> WorkstationFuture<'_, FileReadResult> {
        let identity = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| self.validate_workspace(request.workspace_id));
        if let Err(error) = identity {
            return Box::pin(async move { Err(error) });
        }
        if request.max_bytes == 0
            || request.max_bytes > self.read_hard_limit
            || request.max_bytes > HARD_FILE_READ_MAX_BYTES
        {
            return Box::pin(async {
                Err(WorkstationError::new(WorkstationErrorKind::FileTooLarge))
            });
        }
        if self.deadline_expired(request.deadline) {
            return Box::pin(async { Err(WorkstationError::new(WorkstationErrorKind::Timeout)) });
        }

        let adapter = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || adapter.read_blocking(request))
                .await
                .map_err(|_| {
                    WorkstationError::new(WorkstationErrorKind::InternalWorkstationError)
                })?
        })
    }

    fn execute(&self, request: ExecutionRequest) -> WorkstationFuture<'_, ExecutionResult> {
        let validation = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| self.validate_workspace(request.workspace_id));
        if let Err(error) = validation {
            return Box::pin(async move { Err(error) });
        }
        let cwd = self.prepare_committed_execution_cwd(&request);
        let runtime = Arc::clone(&self.execution);
        Box::pin(async move { runtime.execute(request, cwd?).await })
    }

    fn inspect_execution(
        &self,
        request: ExecutionInspectionRequest,
    ) -> WorkstationFuture<'_, ExecutionInspection> {
        let result = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| {
                self.execution
                    .inspect(request.operation_id, request.execution_id)
            });
        Box::pin(async move { result })
    }

    fn cancel_execution(
        &self,
        request: ExecutionCancellationRequest,
    ) -> WorkstationFuture<'_, CancellationResult> {
        let validation =
            self.validate_identity(request.workstation_id, request.expected_generation);
        let runtime = Arc::clone(&self.execution);
        Box::pin(async move {
            validation?;
            Ok(runtime
                .cancel(request.operation_id, request.execution_id)
                .await)
        })
    }
}

impl WorkstationPreparation for LocalWorkstation {
    fn prepare(
        &self,
        request: WorkstationPreparationRequest,
    ) -> WorkstationPreparationFuture<'_, WorkstationPreparationResult> {
        let validation = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| self.validate_workspace(request.workspace_id))
            .and_then(|()| {
                let flags = self.capabilities.flags();
                let capability_available = match request.required_capability {
                    RequiredWorkstationCapability::FilesystemRead => flags.filesystem_read(),
                    RequiredWorkstationCapability::ForegroundExecute => flags.foreground_execute(),
                };
                let privilege_available = match request.effective_privilege {
                    PrivilegeMode::User => flags.privilege_user(),
                    PrivilegeMode::Administrative => flags.privilege_administrative(),
                };
                if capability_available && privilege_available {
                    Ok(())
                } else {
                    Err(WorkstationError::new(
                        WorkstationErrorKind::UnsupportedCapability,
                    ))
                }
            });
        if let Err(error) = validation {
            return Box::pin(async move { Err(error) });
        }
        let adapter = self.clone();
        Box::pin(async move {
            let prepared_cwd = tokio::task::spawn_blocking(move || {
                adapter.prepare_execution_cwd(&request.requested_cwd)
            })
            .await
            .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))??;
            Ok(WorkstationPreparationResult {
                operation_id: request.operation_id,
                prepared_cwd,
            })
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalExecutionSupport {
    pub(crate) foreground: bool,
    pub(crate) administrative: bool,
    pub(crate) process_group: bool,
    pub(crate) cgroup: bool,
    cgroup_root: Option<PathBuf>,
}

/// Performs safe host probes for the Stage 13 capability snapshot.
pub(crate) fn observe_execution_support(
    shell: &Path,
    administrative_enabled: bool,
    delegated_cgroup_root: Option<&Path>,
) -> LocalExecutionSupport {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    let shell_available = shell == Path::new("/bin/bash")
        && std::fs::metadata(shell).is_ok_and(|metadata| {
            metadata.is_file() && {
                #[cfg(unix)]
                {
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    false
                }
            }
        });
    let cgroup_root = execution::probe_cgroup_root(delegated_cgroup_root);
    let cgroup = cgroup_root.is_some();
    let foreground = shell_available && (cfg!(target_os = "macos") || cgroup);
    let administrative =
        foreground && execution::probe_admin(administrative_enabled, cgroup_root.as_deref());
    LocalExecutionSupport {
        foreground,
        administrative,
        process_group: foreground,
        cgroup,
        cgroup_root,
    }
}

/// Constructs the exact truthful Stage 13 local snapshot.
pub(crate) fn stage13_capabilities(
    workstation_id: WorkstationId,
    generation: WorkstationGeneration,
    workspace_id: WorkspaceId,
    logical_workspace_root: LogicalPathReference,
    default_shell: LogicalPathReference,
    support: LocalExecutionSupport,
) -> Result<WorkstationCapabilities, WorkstationError> {
    WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
        workstation_id,
        generation,
        cpu_architecture: std::env::consts::ARCH.to_owned(),
        os_release: std::env::consts::OS.to_owned(),
        default_shell,
        flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
            filesystem_read: true,
            foreground_execute: support.foreground,
            cancel_execution: support.foreground,
            inspect_execution: support.foreground,
            privilege_user: true,
            privilege_administrative: support.administrative,
            process_group_cleanup: support.process_group,
            cgroup_cleanup: support.cgroup,
        }),
        limits: WorkstationCapabilityLimits::try_new(
            if support.foreground {
                HARD_EXECUTION_TIMEOUT_MS
            } else {
                0
            },
            if support.foreground {
                HARD_EXECUTION_STREAM_CAPTURE_BYTES
            } else {
                0
            },
            if support.foreground {
                HARD_EXECUTION_STREAM_CAPTURE_BYTES
            } else {
                0
            },
        )
        .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))?,
        workspaces: vec![
            WorkspaceCapabilityRef::try_new(workspace_id, logical_workspace_root).map_err(
                |_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError),
            )?,
        ],
    })
    .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))
}

struct ResolvedTarget {
    physical_path: PathBuf,
    evidence: ResolvedPathEvidence,
}

#[cfg(unix)]
fn prepared_cwd_object_identity(
    metadata: &Metadata,
) -> Result<PreparedCwdObjectIdentity, WorkstationError> {
    PreparedCwdObjectIdentity::try_new(
        metadata.dev(),
        metadata.ino(),
        PreparedCwdObjectType::Directory,
    )
}

#[cfg(not(unix))]
fn prepared_cwd_object_identity(
    _metadata: &Metadata,
) -> Result<PreparedCwdObjectIdentity, WorkstationError> {
    Err(WorkstationError::new(
        WorkstationErrorKind::UnsupportedCapability,
    ))
}

#[derive(Eq, PartialEq)]
struct MetadataSnapshot {
    len: u64,
    modified_at: Option<UtcTimestamp>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
}

impl MetadataSnapshot {
    fn capture(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified_at: metadata.modified().ok().and_then(canonical_system_time),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
            #[cfg(unix)]
            modified_seconds: metadata.mtime(),
            #[cfg(unix)]
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}

fn ensure_regular_file(metadata: &Metadata) -> Result<(), WorkstationError> {
    let file_type = metadata.file_type();
    if file_type.is_file() {
        return Ok(());
    }
    #[cfg(unix)]
    let _known_special = file_type.is_dir()
        || file_type.is_fifo()
        || file_type.is_socket()
        || file_type.is_char_device()
        || file_type.is_block_device();
    Err(WorkstationError::new(WorkstationErrorKind::InvalidPath))
}

fn canonical_system_time(value: SystemTime) -> Option<UtcTimestamp> {
    let nanoseconds = match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok()?,
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()?
            .checked_neg()?,
    };
    let instant = OffsetDateTime::from_unix_timestamp_nanos(nanoseconds).ok()?;
    UtcTimestamp::from_offset_datetime(instant).ok()
}

fn is_recognized_pseudo_filesystem(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut components = path.components();
        let _root = components.next();
        return matches!(
            components.next(),
            Some(Component::Normal(component)) if component == "proc" || component == "sys"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

fn map_constructor_error(error: std::io::Error) -> WorkstationError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            WorkstationError::new(WorkstationErrorKind::PermissionDenied)
        }
        std::io::ErrorKind::NotFound => {
            WorkstationError::new(WorkstationErrorKind::WorkstationUnavailable)
        }
        _ => WorkstationError::new(WorkstationErrorKind::WorkstationUnavailable),
    }
}

fn map_path_error(error: std::io::Error) -> WorkstationError {
    if matches!(
        error.raw_os_error(),
        Some(
            nix::libc::ELOOP
                | nix::libc::ENOTDIR
                | nix::libc::ENXIO
                | nix::libc::ENODEV
                | nix::libc::EOPNOTSUPP,
        )
    ) {
        return WorkstationError::new(WorkstationErrorKind::InvalidPath);
    }
    match error.kind() {
        std::io::ErrorKind::NotFound => WorkstationError::new(WorkstationErrorKind::NotFound),
        std::io::ErrorKind::PermissionDenied => {
            WorkstationError::new(WorkstationErrorKind::PermissionDenied)
        }
        std::io::ErrorKind::InvalidInput => {
            WorkstationError::new(WorkstationErrorKind::InvalidPath)
        }
        _ => WorkstationError::new(WorkstationErrorKind::IoError),
    }
}

fn map_io_error(error: std::io::Error) -> WorkstationError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            WorkstationError::new(WorkstationErrorKind::PermissionDenied)
        }
        _ => WorkstationError::new(WorkstationErrorKind::IoError),
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadHookPoint {
    AfterOpenBeforeRead,
    AfterFirstChunk,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::Path;
    #[cfg(target_os = "macos")]
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use nix::sys::signal::kill;
    use nix::sys::stat::Mode;
    use nix::unistd::{Pid, mkfifo};

    use super::*;
    use crate::adapters::artifacts::LocalArtifactStore;
    use crate::domain::{
        Certainty, CraxiiId, ExecutionId, HostingProvider, MonotonicDuration, OperationId,
        PrivilegeMode, WorkspaceIdentityInput, WorkstationIdentityInput,
    };
    use crate::ports::clock::{MonotonicInstant, TestClock};
    use crate::ports::workstation::{
        DEFAULT_FILE_READ_MAX_BYTES, ExecutionCancellationState, ExecutionCapturePolicy,
        ExecutionCleanupPolicy, ExecutionResultKind, ExecutionStdinPolicy,
        HARD_EXECUTION_COMMAND_MAX_BYTES,
    };

    const AT: &str = "2026-08-29T01:02:03.456789Z";
    static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn prepared_cwd_evidence(
        workstation_id: WorkstationId,
        generation: WorkstationGeneration,
        workspace_id: WorkspaceId,
        requested_cwd: LogicalPathReference,
        physical_path: &Path,
    ) -> PreparedCwdEvidence {
        let canonical = std::fs::canonicalize(physical_path).unwrap();
        let metadata = std::fs::metadata(&canonical).unwrap();
        PreparedCwdEvidence::new(
            ResolvedPathEvidence::try_new(
                workstation_id,
                generation,
                workspace_id,
                requested_cwd,
                canonical.to_str().unwrap(),
            )
            .unwrap(),
            prepared_cwd_object_identity(&metadata).unwrap(),
        )
    }

    fn open_descriptors_for_directory(path: &Path) -> usize {
        let expected = fs::metadata(path).unwrap();
        fs::read_dir("/dev/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
            .filter(|descriptor| {
                let mut observed = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
                // SAFETY: `observed` points to writable storage for `fstat`; its bytes are read
                // only when `fstat` reports success for the enumerated live descriptor.
                let succeeded =
                    unsafe { nix::libc::fstat(*descriptor, observed.as_mut_ptr()) } == 0;
                succeeded && {
                    // SAFETY: successful `fstat` initialized the complete `stat` value.
                    let observed = unsafe { observed.assume_init() };
                    observed.st_dev as u64 == expected.dev() && observed.st_ino == expected.ino()
                }
            })
            .count()
    }

    #[cfg(target_os = "macos")]
    struct InterruptedLeaderObserver {
        calls: AtomicUsize,
    }

    #[cfg(target_os = "macos")]
    impl InterruptedLeaderObserver {
        const fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[cfg(target_os = "macos")]
    impl execution::LeaderObserver for InterruptedLeaderObserver {
        fn observe(&self, _pid: i32) -> std::io::Result<execution::LeaderObservationStatus> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(execution::LeaderObservationStatus::Interrupted)
        }
    }

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "craxii-stage12-workstation-test-{}-{}",
                std::process::id(),
                uuid::Uuid::now_v7()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn workspace(&self) -> PathBuf {
            self.0.join("configured-workspace")
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        _root: TestRoot,
        workspace_root: PathBuf,
        workstation: LocalWorkstation,
        artifact_store: Arc<LocalArtifactStore>,
        clock: Arc<TestClock>,
    }

    impl Fixture {
        fn new(read_hard_limit: u64) -> Self {
            Self::with_logical_root(read_hard_limit, "/logical/primary")
        }

        fn with_logical_root(read_hard_limit: u64, logical_root: &str) -> Self {
            Self::with_execution_target(read_hard_limit, logical_root, false, None)
        }

        fn with_execution_target(
            read_hard_limit: u64,
            logical_root: &str,
            administrative_enabled: bool,
            delegated_cgroup_root: Option<PathBuf>,
        ) -> Self {
            let root = TestRoot::new();
            let workspace_root = root.workspace();
            fs::create_dir(&workspace_root).unwrap();
            fs::create_dir(workspace_root.join("cwd")).unwrap();
            let workstation_id = WorkstationId::generate();
            let workspace_id = WorkspaceId::generate();
            let craxii_id = CraxiiId::generate();
            let generation = WorkstationGeneration::try_new(7).unwrap();
            let workstation_identity = WorkstationIdentity::try_new(WorkstationIdentityInput {
                workstation_id,
                craxii_id,
                generation,
                hosting_provider: HostingProvider::try_new("unclassified").unwrap(),
                provider_instance_id: None,
                image_id: None,
                provisioning_revision: None,
                cpu_architecture: std::env::consts::ARCH.into(),
                os_release: std::env::consts::OS.into(),
                created_at: AT.parse().unwrap(),
            })
            .unwrap();
            let workspace = WorkspaceIdentity::try_new(WorkspaceIdentityInput {
                workspace_id,
                craxii_id,
                workstation_id,
                logical_name: "primary".into(),
                logical_root: LogicalPathReference::absolute(logical_root).unwrap(),
                created_at: AT.parse().unwrap(),
            })
            .unwrap();
            let clock = Arc::new(TestClock::new(
                OffsetDateTime::UNIX_EPOCH,
                Duration::from_secs(1),
            ));
            let artifact_store =
                Arc::new(LocalArtifactStore::initialize(&root.0.join("artifacts")).unwrap());
            let workstation = LocalWorkstation::new(
                &workstation_identity,
                &workspace,
                LocalWorkstationOptions {
                    default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
                    configured_workspace_root: workspace_root.clone(),
                    read_hard_limit,
                    artifact_store: artifact_store.clone(),
                    administrative_enabled,
                    delegated_cgroup_root,
                    clock: clock.clone(),
                },
            )
            .unwrap();
            Self {
                _root: root,
                workspace_root,
                workstation,
                artifact_store,
                clock,
            }
        }

        fn request(&self, path: LogicalPathReference, max_bytes: u64) -> FileReadRequest {
            FileReadRequest {
                operation_id: OperationId::generate(),
                workstation_id: self.workstation.workstation_id(),
                expected_generation: self.workstation.generation(),
                workspace_id: self.workstation.workspace_id(),
                path,
                max_bytes,
                deadline: MonotonicInstant::from_elapsed(Duration::from_secs(30)),
            }
        }

        fn relative(&self, path: &str, max_bytes: u64) -> FileReadRequest {
            self.request(
                LogicalPathReference::workspace_relative(path).unwrap(),
                max_bytes,
            )
        }

        async fn read_relative(
            &self,
            path: &str,
            max_bytes: u64,
        ) -> Result<FileReadResult, WorkstationError> {
            self.workstation
                .read_file(self.relative(path, max_bytes))
                .await
        }

        fn execution_request(&self, command: impl Into<String>) -> ExecutionRequest {
            let requested_cwd = LogicalPathReference::workspace_relative("cwd").unwrap();
            ExecutionRequest {
                operation_id: OperationId::generate(),
                execution_id: ExecutionId::generate(),
                work_id: crate::domain::WorkId::generate(),
                workstation_id: self.workstation.workstation_id(),
                expected_generation: self.workstation.generation(),
                workspace_id: self.workstation.workspace_id(),
                command: command.into(),
                requested_cwd: requested_cwd.clone(),
                prepared_cwd: prepared_cwd_evidence(
                    self.workstation.workstation_id(),
                    self.workstation.generation(),
                    self.workstation.workspace_id(),
                    requested_cwd,
                    &self.workspace_root.join("cwd"),
                ),
                effective_privilege: PrivilegeMode::User,
                stdin: ExecutionStdinPolicy::Closed,
                timeout: MonotonicDuration::from_millis(10_000),
                deadline: MonotonicInstant::from_elapsed(Duration::from_secs(60)),
                capture: ExecutionCapturePolicy {
                    stdout_max_bytes: HARD_EXECUTION_STREAM_CAPTURE_BYTES,
                    stderr_max_bytes: HARD_EXECUTION_STREAM_CAPTURE_BYTES,
                },
                cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
            }
        }

        fn preparation_request(&self) -> WorkstationPreparationRequest {
            WorkstationPreparationRequest {
                operation_id: OperationId::generate(),
                workstation_id: self.workstation.workstation_id(),
                expected_generation: self.workstation.generation(),
                workspace_id: self.workstation.workspace_id(),
                requested_cwd: LogicalPathReference::workspace_relative("cwd").unwrap(),
                required_capability: RequiredWorkstationCapability::ForegroundExecute,
                effective_privilege: PrivilegeMode::User,
            }
        }

        async fn prepared_execution_request(
            &self,
            requested_cwd: LogicalPathReference,
            command: &str,
        ) -> ExecutionRequest {
            let prepared = self
                .workstation
                .prepare(WorkstationPreparationRequest {
                    requested_cwd: requested_cwd.clone(),
                    ..self.preparation_request()
                })
                .await
                .unwrap();
            let mut request = self.execution_request(command);
            request.requested_cwd = requested_cwd;
            request.prepared_cwd = prepared.prepared_cwd;
            request
        }

        async fn execute(&self, command: impl Into<String>) -> ExecutionResult {
            self.workstation
                .execute(self.execution_request(command))
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn preparation_resolves_bound_cwd_without_creating_machine_side_effects() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let marker = fixture
            .workspace_root
            .join("preparation-must-not-create-this");
        let request = fixture.preparation_request();
        let operation_id = request.operation_id;
        let result = fixture.workstation.prepare(request).await.unwrap();
        assert_eq!(result.operation_id, operation_id);
        assert_eq!(
            result.prepared_cwd.resolved_cwd().requested_path(),
            &LogicalPathReference::workspace_relative("cwd").unwrap()
        );
        assert_eq!(
            result.prepared_cwd.resolved_cwd().resolved_absolute_path(),
            fs::canonicalize(fixture.workspace_root.join("cwd"))
                .unwrap()
                .to_str()
                .unwrap()
        );
        let identity = result.prepared_cwd.object_identity();
        let metadata = fs::metadata(fixture.workspace_root.join("cwd")).unwrap();
        assert_eq!(identity.device(), metadata.dev());
        assert_eq!(identity.inode(), metadata.ino());
        assert_eq!(identity.object_type(), PreparedCwdObjectType::Directory);
        assert!(!marker.exists());
        assert!(directory_names(&fixture._root.0.join("artifacts/sha256")).is_empty());
    }

    #[tokio::test]
    async fn committed_cwd_drift_or_disappearance_fails_before_spawn() {
        for drift in ["retarget", "disappear"] {
            let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
            let old = fixture.workspace_root.join("old-target");
            let new = fixture.workspace_root.join("new-target");
            fs::create_dir(&old).unwrap();
            fs::create_dir(&new).unwrap();
            let logical = fixture.workspace_root.join("linked-cwd");
            symlink(&old, &logical).unwrap();
            let request = fixture
                .prepared_execution_request(
                    LogicalPathReference::workspace_relative("linked-cwd").unwrap(),
                    "pwd > should-not-exist",
                )
                .await;
            assert_eq!(
                request.prepared_cwd.resolved_cwd().resolved_absolute_path(),
                std::fs::canonicalize(&old).unwrap().to_str().unwrap()
            );

            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let workstation = fixture.workstation.clone();
            let execute_barrier = Arc::clone(&barrier);
            let execution = tokio::spawn(async move {
                execute_barrier.wait().await;
                workstation.execute(request).await
            });
            fs::remove_file(&logical).unwrap();
            if drift == "retarget" {
                symlink(&new, &logical).unwrap();
            } else {
                fs::remove_dir(&old).unwrap();
            }
            barrier.wait().await;
            let error = execution.await.unwrap().unwrap_err();
            assert!(matches!(
                error.kind(),
                WorkstationErrorKind::InvalidPath | WorkstationErrorKind::NotFound
            ));
            assert!(fixture.workstation.execution_lifecycle_events().is_empty());
            assert!(!fixture.workspace_root.join("should-not-exist").exists());
        }
    }

    #[tokio::test]
    async fn same_path_directory_recreation_is_rejected_by_object_identity_before_spawn() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let cwd = fixture.workspace_root.join("cwd");
        let request = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "touch same-path-marker",
            )
            .await;
        let prepared_identity = request.prepared_cwd.object_identity();

        let arrived = Arc::new(tokio::sync::Barrier::new(2));
        let workstation = fixture.workstation.clone();
        let task_barrier = Arc::clone(&arrived);
        let execution = tokio::spawn(async move {
            task_barrier.wait().await;
            workstation.execute(request).await
        });
        fs::remove_dir(&cwd).unwrap();
        fs::create_dir(fixture.workspace_root.join("inode-reuse-guard")).unwrap();
        fs::create_dir(&cwd).unwrap();
        let replacement = prepared_cwd_object_identity(&fs::metadata(&cwd).unwrap()).unwrap();
        assert_ne!(replacement, prepared_identity);
        arrived.wait().await;

        assert_eq!(
            execution.await.unwrap().unwrap_err().kind(),
            WorkstationErrorKind::InvalidPath
        );
        assert!(!cwd.join("same-path-marker").exists());
        assert!(fixture.workstation.execution_lifecycle_events().is_empty());
    }

    #[tokio::test]
    async fn directory_path_replaced_by_live_distinct_object_is_rejected_before_spawn() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let cwd = fixture.workspace_root.join("cwd");
        let retained = fixture.workspace_root.join("prepared-cwd-retained");
        let request = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "touch replacement-marker",
            )
            .await;
        fs::rename(&cwd, &retained).unwrap();
        fs::create_dir(&cwd).unwrap();
        assert_ne!(
            request.prepared_cwd.object_identity(),
            prepared_cwd_object_identity(&fs::metadata(&cwd).unwrap()).unwrap()
        );
        assert_eq!(
            fixture
                .workstation
                .execute(request)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );
        assert!(!cwd.join("replacement-marker").exists());
        assert!(fixture.workstation.execution_lifecycle_events().is_empty());
    }

    #[tokio::test]
    async fn prepared_directory_disappearance_is_rejected_before_spawn() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let cwd = fixture.workspace_root.join("cwd");
        let request = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "touch disappeared-marker",
            )
            .await;
        fs::remove_dir(&cwd).unwrap();
        assert_eq!(
            fixture
                .workstation
                .execute(request)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::NotFound
        );
        assert!(fixture.workstation.execution_lifecycle_events().is_empty());
    }

    #[tokio::test]
    async fn prepared_cwd_descriptor_is_closed_after_success_failure_and_future_cancellation() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let cwd = fixture.workspace_root.join("cwd");
        assert_eq!(open_descriptors_for_directory(&cwd), 0);

        let success = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "true",
            )
            .await;
        let execution = fixture.workstation.execute(success);
        assert_eq!(open_descriptors_for_directory(&cwd), 1);
        let result = execution.await;
        if cfg!(target_os = "macos") {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().kind(),
                WorkstationErrorKind::UnsupportedCapability
            );
        }
        assert_eq!(open_descriptors_for_directory(&cwd), 0);

        let mut failure = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "true",
            )
            .await;
        let identity = failure.prepared_cwd.object_identity();
        failure.prepared_cwd = PreparedCwdEvidence::new(
            failure.prepared_cwd.resolved_cwd().clone(),
            PreparedCwdObjectIdentity::try_new(
                identity.device(),
                identity.inode().checked_add(1).unwrap(),
                PreparedCwdObjectType::Directory,
            )
            .unwrap(),
        );
        assert_eq!(
            fixture
                .workstation
                .execute(failure)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );
        assert_eq!(open_descriptors_for_directory(&cwd), 0);

        let cancelled = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "true",
            )
            .await;
        let future = fixture.workstation.execute(cancelled);
        assert_eq!(open_descriptors_for_directory(&cwd), 1);
        drop(future);
        assert_eq!(open_descriptors_for_directory(&cwd), 0);
    }

    #[tokio::test]
    async fn unchanged_committed_cwd_executes_exact_target_and_binding_mismatch_is_definite() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let marker = fixture.workspace_root.join("cwd/exact-target.txt");
        let request = fixture
            .prepared_execution_request(
                LogicalPathReference::workspace_relative("cwd").unwrap(),
                "pwd > exact-target.txt",
            )
            .await;
        if cfg!(target_os = "macos") {
            let result = fixture.workstation.execute(request.clone()).await.unwrap();
            assert_eq!(
                result.resolved_cwd,
                request.prepared_cwd.resolved_cwd().clone()
            );
            assert!(marker.exists());
        }

        let mut wrong_generation = request.clone();
        wrong_generation.expected_generation = WorkstationGeneration::try_new(1).unwrap();
        assert_eq!(
            fixture
                .workstation
                .execute(wrong_generation)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::GenerationMismatch
        );
        let mut wrong_workspace = request;
        wrong_workspace.workspace_id = WorkspaceId::generate();
        assert_eq!(
            fixture
                .workstation
                .execute(wrong_workspace)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::WorkspaceNotFound
        );
    }

    #[tokio::test]
    async fn preparation_rejects_stale_generation_and_unavailable_admin_before_resolution() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let mut stale = fixture.preparation_request();
        stale.expected_generation = WorkstationGeneration::try_new(8).unwrap();
        assert_eq!(
            fixture.workstation.prepare(stale).await.unwrap_err().kind(),
            WorkstationErrorKind::GenerationMismatch
        );

        let mut admin = fixture.preparation_request();
        admin.effective_privilege = PrivilegeMode::Administrative;
        assert_eq!(
            fixture.workstation.prepare(admin).await.unwrap_err().kind(),
            WorkstationErrorKind::UnsupportedCapability
        );
    }

    async fn wait_for_path(path: &Path) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    fn assert_stable_process_group_order(events: &[&'static str]) {
        let leader_observed = events
            .iter()
            .position(|event| *event == "leader_terminal_observed")
            .expect("the direct child terminal state is observed without reaping");
        let cleanup_finished = events
            .iter()
            .position(|event| *event == "descendant_cleanup_finished")
            .expect("descendant cleanup is proved while leader identity is stable");
        let released_for_reap = events
            .iter()
            .position(|event| *event == "leader_identity_released_for_reap")
            .expect("stable group ownership is explicitly consumed before reap");
        let leader_reaped = events
            .iter()
            .position(|event| *event == "leader_reaped")
            .expect("the direct child is reaped");
        assert!(leader_observed < cleanup_finished);
        assert!(cleanup_finished < released_for_reap);
        assert!(released_for_reap < leader_reaped);
        let signals: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| (*event == "process_group_signalled").then_some(index))
            .collect();
        assert!(!signals.is_empty());
        assert!(signals.into_iter().all(|index| index < released_for_reap));
        assert!(
            events[leader_reaped + 1..]
                .iter()
                .all(|event| *event != "process_group_signalled")
        );
    }

    #[test]
    fn construction_binds_exact_identity_explicit_root_and_no_current_directory() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        assert_eq!(fixture.workstation.generation().get(), 7);
        assert_eq!(
            fixture.workstation.logical_workspace_root().canonical(),
            "/logical/primary"
        );
        assert_ne!(
            fixture.workstation.resolved_workspace_root,
            std::env::current_dir().unwrap()
        );
        let debug = format!("{:?}", fixture.workstation);
        assert!(!debug.contains(fixture.workspace_root.to_str().unwrap()));
        assert!(debug.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn identity_generation_and_workspace_guards_precede_path_io() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let missing = LogicalPathReference::workspace_relative("does-not-exist").unwrap();

        let mut request = fixture.request(missing.clone(), 16);
        request.workstation_id = WorkstationId::generate();
        assert_eq!(
            fixture
                .workstation
                .read_file(request)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::WorkstationUnavailable
        );

        let mut request = fixture.request(missing.clone(), 16);
        request.expected_generation = WorkstationGeneration::try_new(8).unwrap();
        assert_eq!(
            fixture
                .workstation
                .read_file(request)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::GenerationMismatch
        );

        let mut request = fixture.request(missing, 16);
        request.workspace_id = WorkspaceId::generate();
        assert_eq!(
            fixture
                .workstation
                .read_file(request)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::WorkspaceNotFound
        );

        for (workstation_id, generation, expected) in [
            (
                WorkstationId::generate(),
                fixture.workstation.generation(),
                WorkstationErrorKind::WorkstationUnavailable,
            ),
            (
                fixture.workstation.workstation_id(),
                WorkstationGeneration::try_new(8).unwrap(),
                WorkstationErrorKind::GenerationMismatch,
            ),
        ] {
            assert_eq!(
                fixture
                    .workstation
                    .capabilities(CapabilitiesRequest {
                        operation_id: OperationId::generate(),
                        workstation_id,
                        expected_generation: generation,
                    })
                    .await
                    .unwrap_err()
                    .kind(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn capabilities_are_exact_truthful_stage13_runtime_facts() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let operation_id = OperationId::generate();
        let result = fixture
            .workstation
            .capabilities(CapabilitiesRequest {
                operation_id,
                workstation_id: fixture.workstation.workstation_id(),
                expected_generation: fixture.workstation.generation(),
            })
            .await
            .unwrap();
        assert_eq!(result.operation_id, operation_id);
        let capabilities = result.capabilities;
        let flags = capabilities.flags();
        assert!(flags.filesystem_read());
        assert!(flags.privilege_user());
        assert_eq!(flags.foreground_execute(), cfg!(target_os = "macos"));
        assert_eq!(flags.cancel_execution(), cfg!(target_os = "macos"));
        assert_eq!(flags.inspect_execution(), cfg!(target_os = "macos"));
        assert!(!flags.privilege_administrative());
        assert_eq!(flags.process_group_cleanup(), cfg!(target_os = "macos"));
        assert!(!flags.cgroup_cleanup());
        let expected_timeout = if cfg!(target_os = "macos") {
            900_000
        } else {
            0
        };
        let expected_capture = if cfg!(target_os = "macos") {
            8_388_608
        } else {
            0
        };
        assert_eq!(
            capabilities.limits().max_execution_timeout_ms(),
            expected_timeout
        );
        assert_eq!(capabilities.limits().max_stdout_bytes(), expected_capture);
        assert_eq!(capabilities.limits().max_stderr_bytes(), expected_capture);
        assert_eq!(capabilities.cpu_architecture(), std::env::consts::ARCH);
        assert_eq!(capabilities.os_release(), std::env::consts::OS);
        assert_eq!(capabilities.workspaces().len(), 1);
        assert_eq!(
            capabilities.workspaces()[0].workspace_id(),
            fixture.workstation.workspace_id()
        );
        assert_eq!(
            capabilities.workspaces()[0].logical_root(),
            fixture.workstation.logical_workspace_root()
        );
    }

    #[tokio::test]
    async fn normal_utf8_empty_multibyte_bom_newlines_nul_and_hashes_are_exact() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let cases: [(&str, &[u8]); 7] = [
            ("normal.txt", b"hello workstation\n"),
            ("empty.txt", b""),
            ("unicode.txt", "資料 café 🦀".as_bytes()),
            ("bom.txt", b"\xef\xbb\xbfsource"),
            ("crlf.txt", b"one\r\ntwo\r\n"),
            ("lf.txt", b"one\ntwo\n"),
            ("nul.txt", b"left\0right"),
        ];
        for (name, bytes) in cases {
            let path = fixture.workspace_root.join(name);
            fs::write(&path, bytes).unwrap();
            let before = fs::metadata(&path).unwrap();
            let result = fixture.read_relative(name, 1_024).await.unwrap();
            let after = fs::metadata(&path).unwrap();
            assert_eq!(result.operation_id.to_string().len(), 36);
            assert_eq!(result.requested_path.canonical(), name);
            assert_eq!(result.file_type, WorkstationFileType::Regular);
            assert_eq!(result.encoding, FileEncoding::Utf8);
            assert_eq!(result.byte_length.get(), bytes.len() as u64);
            assert_eq!(result.sha256, Sha256Digest::hash_bytes(bytes));
            assert_eq!(result.text.as_bytes(), bytes);
            assert!(!result.truncated);
            assert_eq!(
                result.resolved_path.resolved_absolute_path(),
                fs::canonicalize(&path).unwrap().to_str().unwrap()
            );
            assert_eq!(before.len(), after.len());
            assert_eq!(before.mode(), after.mode());
            assert_eq!(before.mtime(), after.mtime());
            assert_eq!(before.mtime_nsec(), after.mtime_nsec());
        }
    }

    #[tokio::test]
    async fn absolute_nested_unicode_and_control_character_paths_are_supported_and_redacted() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let nested = fixture.workspace_root.join("資料/nested");
        fs::create_dir_all(&nested).unwrap();
        let path = nested.join("line\nbreak.txt");
        fs::write(&path, "exact").unwrap();

        let relative = fixture
            .read_relative("資料//./nested/line\nbreak.txt", 16)
            .await
            .unwrap();
        assert_eq!(relative.text, "exact");
        let absolute = fixture
            .workstation
            .read_file(fixture.request(
                LogicalPathReference::absolute(path.to_str().unwrap()).unwrap(),
                16,
            ))
            .await
            .unwrap();
        assert_eq!(absolute.text, "exact");
        for debug in [format!("{relative:?}"), format!("{absolute:?}")] {
            assert!(!debug.contains("line\nbreak.txt"));
            assert!(!debug.contains("exact"));
            assert!(debug.contains("[REDACTED]"));
        }
    }

    #[tokio::test]
    async fn invalid_utf8_returns_only_safe_binary_length_and_digest_evidence() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let bytes = [0xff, 0xfe, 0x80, 0x00];
        fs::write(fixture.workspace_root.join("binary.bin"), bytes).unwrap();
        let error = fixture.read_relative("binary.bin", 16).await.unwrap_err();
        assert_eq!(error.kind(), WorkstationErrorKind::BinaryContent);
        assert_eq!(error.byte_length().unwrap().get(), bytes.len() as u64);
        assert_eq!(error.sha256(), Some(Sha256Digest::hash_bytes(&bytes)));
        let debug = format!("{error:?}");
        assert!(!debug.contains("binary.bin"));
    }

    #[tokio::test]
    async fn exact_request_and_hard_limits_succeed_while_oversize_and_sparse_fail() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        assert_eq!(DEFAULT_FILE_READ_MAX_BYTES, 1_048_576);
        fs::write(fixture.workspace_root.join("exact.txt"), b"12345678").unwrap();
        assert_eq!(
            fixture
                .read_relative("exact.txt", 8)
                .await
                .unwrap()
                .byte_length
                .get(),
            8
        );
        assert_eq!(
            fixture
                .read_relative("exact.txt", 7)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::FileTooLarge
        );

        let hard_bytes = vec![b'x'; HARD_FILE_READ_MAX_BYTES as usize];
        fs::write(fixture.workspace_root.join("hard.txt"), &hard_bytes).unwrap();
        assert_eq!(
            fixture
                .read_relative("hard.txt", HARD_FILE_READ_MAX_BYTES)
                .await
                .unwrap()
                .byte_length
                .get(),
            HARD_FILE_READ_MAX_BYTES
        );
        assert_eq!(
            fixture
                .read_relative("hard.txt", HARD_FILE_READ_MAX_BYTES + 1)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::FileTooLarge
        );
        assert_eq!(
            fixture
                .read_relative("exact.txt", 0)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::FileTooLarge
        );

        let sparse = fs::File::create(fixture.workspace_root.join("sparse.bin")).unwrap();
        sparse.set_len(HARD_FILE_READ_MAX_BYTES + 1).unwrap();
        assert_eq!(
            fixture
                .read_relative("sparse.bin", HARD_FILE_READ_MAX_BYTES)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::FileTooLarge
        );
    }

    #[tokio::test]
    async fn missing_regular_target_is_not_found() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        assert_eq!(
            fixture
                .read_relative("missing.txt", DEFAULT_FILE_READ_MAX_BYTES)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::NotFound
        );
    }

    #[tokio::test]
    async fn directory_fifo_socket_and_character_device_reject_without_blocking() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        fs::create_dir(fixture.workspace_root.join("directory")).unwrap();
        assert_eq!(
            fixture
                .read_relative("directory", 16)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );

        let fifo = fixture.workspace_root.join("pipe");
        mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        assert_eq!(
            fixture.read_relative("pipe", 16).await.unwrap_err().kind(),
            WorkstationErrorKind::InvalidPath
        );

        let socket_path = std::env::temp_dir().join(format!("cx12-{}.sock", uuid::Uuid::now_v7()));
        let _socket = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        symlink(&socket_path, fixture.workspace_root.join("socket")).unwrap();
        assert_eq!(
            fixture
                .read_relative("socket", 16)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );
        fs::remove_file(&socket_path).unwrap();

        let result = fixture
            .workstation
            .read_file(fixture.request(LogicalPathReference::absolute("/dev/null").unwrap(), 16));
        assert_eq!(
            result.await.unwrap_err().kind(),
            WorkstationErrorKind::InvalidPath
        );
    }

    #[tokio::test]
    async fn symlinks_inside_outside_and_chains_succeed_broken_and_loops_fail() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        fs::write(fixture.workspace_root.join("inside.txt"), "inside").unwrap();
        symlink("inside.txt", fixture.workspace_root.join("inside-link")).unwrap();
        assert_eq!(
            fixture.read_relative("inside-link", 16).await.unwrap().text,
            "inside"
        );

        let outside = fixture._root.0.join("outside.txt");
        fs::write(&outside, "outside").unwrap();
        symlink(&outside, fixture.workspace_root.join("outside-link")).unwrap();
        symlink("outside-link", fixture.workspace_root.join("chain-one")).unwrap();
        symlink("chain-one", fixture.workspace_root.join("chain-two")).unwrap();
        assert_eq!(
            fixture
                .read_relative("outside-link", 16)
                .await
                .unwrap()
                .text,
            "outside"
        );
        assert_eq!(
            fixture.read_relative("chain-two", 16).await.unwrap().text,
            "outside"
        );

        symlink("missing", fixture.workspace_root.join("broken")).unwrap();
        assert_eq!(
            fixture
                .read_relative("broken", 16)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::NotFound
        );
        symlink("loop-b", fixture.workspace_root.join("loop-a")).unwrap();
        symlink("loop-a", fixture.workspace_root.join("loop-b")).unwrap();
        assert_eq!(
            fixture
                .read_relative("loop-a", 16)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );
    }

    #[tokio::test]
    async fn permission_denial_is_classified_when_the_environment_enforces_mode_bits() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let path = fixture.workspace_root.join("unreadable.txt");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let direct_denied = OpenOptions::new()
            .read(true)
            .open(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
        let result = fixture.read_relative("unreadable.txt", 16).await;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        if direct_denied {
            assert_eq!(
                result.unwrap_err().kind(),
                WorkstationErrorKind::PermissionDenied
            );
        } else {
            assert_eq!(result.unwrap().text, "secret");
        }
    }

    #[tokio::test]
    async fn replacement_after_open_returns_one_complete_original_file_object() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let path = fixture.workspace_root.join("replace.txt");
        fs::write(&path, "original-opened-object").unwrap();
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_hook = fired.clone();
        let workstation = fixture
            .workstation
            .clone()
            .with_read_hook(move |point, path| {
                if point == ReadHookPoint::AfterOpenBeforeRead
                    && !fired_for_hook.swap(true, Ordering::SeqCst)
                {
                    fs::rename(path, path.with_extension("opened")).unwrap();
                    fs::write(path, "replacement-path-object").unwrap();
                }
            });
        let result = workstation
            .read_file(fixture.relative("replace.txt", 128))
            .await
            .unwrap();
        assert_eq!(result.text, "original-opened-object");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "replacement-path-object"
        );
    }

    #[tokio::test]
    async fn deterministic_mutation_growth_and_shrink_are_changed_during_read() {
        for (name, mutate) in [("mutate", 0_u8), ("grow", 1_u8), ("shrink", 2_u8)] {
            let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
            let path = fixture.workspace_root.join(name);
            fs::write(&path, vec![b'a'; READ_BUFFER_BYTES * 2]).unwrap();
            let fired = Arc::new(AtomicBool::new(false));
            let fired_for_hook = fired.clone();
            let workstation = fixture
                .workstation
                .clone()
                .with_read_hook(move |point, path| {
                    let target_point = if mutate == 0 {
                        ReadHookPoint::AfterOpenBeforeRead
                    } else {
                        ReadHookPoint::AfterFirstChunk
                    };
                    if point == target_point && !fired_for_hook.swap(true, Ordering::SeqCst) {
                        match mutate {
                            0 => fs::write(path, vec![b'b'; READ_BUFFER_BYTES * 2]).unwrap(),
                            1 => {
                                use std::io::Write as _;
                                OpenOptions::new()
                                    .append(true)
                                    .open(path)
                                    .unwrap()
                                    .write_all(b"growth")
                                    .unwrap();
                            }
                            2 => OpenOptions::new()
                                .write(true)
                                .open(path)
                                .unwrap()
                                .set_len(1)
                                .unwrap(),
                            _ => unreachable!(),
                        }
                    }
                });
            assert_eq!(
                workstation
                    .read_file(fixture.relative(name, 64 * 1_024))
                    .await
                    .unwrap_err()
                    .kind(),
                WorkstationErrorKind::ChangedDuringRead,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn expired_and_inflight_deadlines_are_honest_without_cancellation_claims() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let mut expired = fixture.relative("missing", 16);
        expired.deadline = MonotonicInstant::from_elapsed(Duration::from_secs(1));
        assert_eq!(
            fixture
                .workstation
                .read_file(expired)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::Timeout
        );

        fs::write(
            fixture.workspace_root.join("deadline.txt"),
            vec![b'x'; READ_BUFFER_BYTES * 2],
        )
        .unwrap();
        let clock = fixture.clock.clone();
        let workstation = fixture.workstation.clone().with_read_hook(move |point, _| {
            if point == ReadHookPoint::AfterFirstChunk {
                clock.set_monotonic(Duration::from_secs(31)).unwrap();
            }
        });
        assert_eq!(
            workstation
                .read_file(fixture.relative("deadline.txt", 64 * 1_024))
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::Timeout
        );
    }

    #[tokio::test]
    async fn read_has_no_target_or_directory_side_effects() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let path = fixture.workspace_root.join("stable.txt");
        fs::write(&path, "stable bytes").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before_bytes = fs::read(&path).unwrap();
        let before_metadata = fs::metadata(&path).unwrap();
        let before_names = directory_names(&fixture.workspace_root);
        fixture.read_relative("stable.txt", 64).await.unwrap();
        let after_metadata = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before_bytes);
        assert_eq!(after_metadata.len(), before_metadata.len());
        assert_eq!(after_metadata.mode(), before_metadata.mode());
        assert_eq!(after_metadata.mtime(), before_metadata.mtime());
        assert_eq!(after_metadata.mtime_nsec(), before_metadata.mtime_nsec());
        assert_eq!(directory_names(&fixture.workspace_root), before_names);
    }

    #[tokio::test]
    async fn stage13_executes_bash_with_separate_capture_and_terminal_registry_removal() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let operation_id = OperationId::generate();
        let execution_id = ExecutionId::generate();
        let requested_cwd = LogicalPathReference::workspace_relative("cwd").unwrap();
        let execute = ExecutionRequest {
            operation_id,
            execution_id,
            work_id: crate::domain::WorkId::generate(),
            workstation_id: fixture.workstation.workstation_id(),
            expected_generation: fixture.workstation.generation(),
            workspace_id: fixture.workstation.workspace_id(),
            command: "printf 'stdout'; printf 'stderr' >&2".into(),
            requested_cwd: requested_cwd.clone(),
            prepared_cwd: prepared_cwd_evidence(
                fixture.workstation.workstation_id(),
                fixture.workstation.generation(),
                fixture.workstation.workspace_id(),
                requested_cwd,
                &fixture.workspace_root.join("cwd"),
            ),
            effective_privilege: PrivilegeMode::User,
            stdin: ExecutionStdinPolicy::Closed,
            timeout: MonotonicDuration::from_millis(1_000),
            deadline: MonotonicInstant::from_elapsed(Duration::from_secs(30)),
            capture: ExecutionCapturePolicy {
                stdout_max_bytes: 1_024,
                stderr_max_bytes: 1_024,
            },
            cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
        };
        if !cfg!(target_os = "macos") {
            assert_eq!(
                fixture
                    .workstation
                    .execute(execute)
                    .await
                    .unwrap_err()
                    .kind(),
                WorkstationErrorKind::UnsupportedCapability
            );
            return;
        }
        let result = fixture.workstation.execute(execute).await.unwrap();
        assert_eq!(
            result.result_kind,
            crate::ports::workstation::ExecutionResultKind::Exited
        );
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.unwrap().projection, "stdout");
        assert_eq!(result.stderr.unwrap().projection, "stderr");
        assert!(result.cleanup.confirmed());
        let inspection = fixture
            .workstation
            .inspect_execution(ExecutionInspectionRequest {
                operation_id,
                execution_id,
                workstation_id: fixture.workstation.workstation_id(),
                expected_generation: fixture.workstation.generation(),
            })
            .await;
        match inspection {
            Ok(inspection) => assert_eq!(
                inspection.state,
                crate::ports::workstation::ExecutionInspectionState::Terminal
            ),
            Err(error) => assert_eq!(error.kind(), WorkstationErrorKind::InspectionNotFound),
        }
        let cancellation = fixture
            .workstation
            .cancel_execution(ExecutionCancellationRequest {
                operation_id,
                execution_id,
                workstation_id: fixture.workstation.workstation_id(),
                expected_generation: fixture.workstation.generation(),
            })
            .await
            .unwrap();
        assert!(matches!(
            cancellation.state,
            crate::ports::workstation::ExecutionCancellationState::AlreadyTerminal
                | crate::ports::workstation::ExecutionCancellationState::NotFound
        ));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn execution_form_quoting_pipes_redirection_profiles_and_fresh_shell_are_exact() {
        let _environment = ENVIRONMENT_LOCK.lock().await;
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let bash_env = fixture._root.0.join("hostile-bash-env");
        fs::write(&bash_env, "export PROFILE_CANARY=loaded\n").unwrap();
        // SAFETY: this test serializes its temporary process-environment mutation and restores it
        // before releasing the guard. The child launcher must clear this value.
        unsafe { std::env::set_var("BASH_ENV", &bash_env) };
        let result = fixture
            .execute(
                "value='a b;$(printf not-interpolated)'; printf '%s' \"$value\" | sed 's/not-/still-/' ; printf 'redirected' > redirected.txt; printf '|%s' \"${PROFILE_CANARY-unset}\"",
            )
            .await;
        // SAFETY: paired restoration under the serialized test guard above.
        unsafe { std::env::remove_var("BASH_ENV") };
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(
            result.stdout.unwrap().projection,
            "a b;$(printf still-interpolated)|unset"
        );
        assert_eq!(
            fs::read_to_string(fixture.workspace_root.join("cwd/redirected.txt")).unwrap(),
            "redirected"
        );

        let first = fixture
            .execute("export CRAXII_TRANSIENT=present; cd /; pwd")
            .await;
        assert_eq!(first.stdout.unwrap().projection, "/\n");
        let second = fixture
            .execute("printf '%s|' \"${CRAXII_TRANSIENT-unset}\"; pwd")
            .await;
        let canonical_cwd = fs::canonicalize(fixture.workspace_root.join("cwd")).unwrap();
        assert_eq!(
            second.stdout.unwrap().projection,
            format!("unset|{}\n", canonical_cwd.display())
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn exact_child_environment_excludes_parent_secrets_and_stdin_is_closed_without_tty() {
        use std::os::fd::AsRawFd as _;

        let _environment = ENVIRONMENT_LOCK.lock().await;
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        for (name, value) in [
            ("OPENAI_API_KEY", "openai-secret-canary"),
            ("ANTHROPIC_API_KEY", "anthropic-secret-canary"),
            ("AWS_SECRET_ACCESS_KEY", "aws-secret-canary"),
            ("CRAXII_GENERIC_SECRET_CANARY", "generic-secret-canary"),
        ] {
            // SAFETY: serialized and restored before the test releases its guard.
            unsafe { std::env::set_var(name, value) };
        }
        let request = fixture.execution_request(
            "env | LC_ALL=C sort; if read line; then printf 'STDIN=open'; else printf 'STDIN=eof'; fi; if test -t 0; then printf '|TTY=yes'; else printf '|TTY=no'; fi",
        );
        let work_id = request.work_id;
        let workspace_id = request.workspace_id;
        let result = fixture.workstation.execute(request).await.unwrap();
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "CRAXII_GENERIC_SECRET_CANARY",
        ] {
            // SAFETY: paired restoration under the serialized test guard above.
            unsafe { std::env::remove_var(name) };
        }
        let stdout = result.stdout.unwrap().projection;
        for required in [
            "HOME=/home/craxii",
            "USER=craxii",
            "LOGNAME=craxii",
            "SHELL=/bin/bash",
            "LANG=C.UTF-8",
            concat!(
                "PATH=",
                "/home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
            ),
            &format!("CRAXII_WORK_ID={work_id}"),
            &format!("CRAXII_WORKSPACE_ID={workspace_id}"),
            "STDIN=eof|TTY=no",
        ] {
            assert!(stdout.contains(required), "missing {required}");
        }
        for forbidden in [
            "openai-secret-canary",
            "anthropic-secret-canary",
            "aws-secret-canary",
            "generic-secret-canary",
            "SSH_AUTH_SOCK=",
            "HTTP_PROXY=",
            "HTTPS_PROXY=",
            "RUST_LOG=",
        ] {
            assert!(!stdout.contains(forbidden), "inherited {forbidden}");
        }

        let descriptor_file = fs::File::open(fixture.workspace_root.join("cwd")).unwrap();
        let descriptor = descriptor_file.as_raw_fd();
        // SAFETY: fcntl operates on the live descriptor owned by `descriptor_file`.
        let flags = unsafe { nix::libc::fcntl(descriptor, nix::libc::F_GETFD) };
        assert_ne!(flags, -1);
        // SAFETY: the descriptor remains live and only its close-on-exec flag is changed.
        assert_ne!(
            unsafe {
                nix::libc::fcntl(
                    descriptor,
                    nix::libc::F_SETFD,
                    flags & !nix::libc::FD_CLOEXEC,
                )
            },
            -1
        );
        let descriptor_result = fixture
            .execute(format!(
                "if test -e /dev/fd/{descriptor}; then printf inherited; else printf closed; fi"
            ))
            .await;
        assert_eq!(descriptor_result.stdout.unwrap().projection, "closed");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn cwd_relative_absolute_outside_symlink_missing_file_and_open_handle_race_are_honest() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let outside = fixture._root.0.join("outside-cwd");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, fixture.workspace_root.join("cwd-link")).unwrap();

        for requested in [
            LogicalPathReference::absolute(outside.to_str().unwrap()).unwrap(),
            LogicalPathReference::workspace_relative("cwd-link").unwrap(),
        ] {
            let request = fixture.prepared_execution_request(requested, "pwd").await;
            let result = fixture.workstation.execute(request).await.unwrap();
            assert_eq!(
                result.stdout.unwrap().projection,
                format!("{}\n", fs::canonicalize(&outside).unwrap().display())
            );
            assert_eq!(
                result.resolved_cwd.resolved_absolute_path(),
                fs::canonicalize(&outside).unwrap().to_str().unwrap()
            );
        }

        let mut missing = fixture.execution_request("true");
        missing.requested_cwd = LogicalPathReference::workspace_relative("missing-cwd").unwrap();
        missing.prepared_cwd = PreparedCwdEvidence::new(
            ResolvedPathEvidence::try_new(
                fixture.workstation.workstation_id(),
                fixture.workstation.generation(),
                fixture.workstation.workspace_id(),
                missing.requested_cwd.clone(),
                fixture.workspace_root.join("missing-cwd").to_str().unwrap(),
            )
            .unwrap(),
            PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory).unwrap(),
        );
        assert_eq!(
            fixture
                .workstation
                .execute(missing)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::NotFound
        );
        fs::write(fixture.workspace_root.join("cwd-file"), "not a directory").unwrap();
        let mut file = fixture.execution_request("true");
        file.requested_cwd = LogicalPathReference::workspace_relative("cwd-file").unwrap();
        file.prepared_cwd = PreparedCwdEvidence::new(
            ResolvedPathEvidence::try_new(
                fixture.workstation.workstation_id(),
                fixture.workstation.generation(),
                fixture.workstation.workspace_id(),
                file.requested_cwd.clone(),
                fixture.workspace_root.join("cwd-file").to_str().unwrap(),
            )
            .unwrap(),
            PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory).unwrap(),
        );
        assert_eq!(
            fixture.workstation.execute(file).await.unwrap_err().kind(),
            WorkstationErrorKind::InvalidPath
        );

        let request = fixture.execution_request("touch opened-object-marker");
        let execution = fixture.workstation.execute(request);
        let opened = fixture.workspace_root.join("opened-cwd-object");
        fs::rename(fixture.workspace_root.join("cwd"), &opened).unwrap();
        fs::create_dir(fixture.workspace_root.join("cwd")).unwrap();
        execution.await.unwrap();
        assert!(opened.join("opened-object-marker").is_file());
        assert!(
            !fixture
                .workspace_root
                .join("cwd/opened-object-marker")
                .exists()
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn output_empty_separate_simultaneous_binary_and_newlines_are_exact() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let empty = fixture.execute("true").await;
        assert_eq!(empty.stdout.unwrap().observed_bytes, 0);
        assert_eq!(empty.stderr.unwrap().observed_bytes, 0);

        let separate = fixture
            .execute("printf 'out\r\nline\n'; printf 'err-no-newline' >&2")
            .await;
        assert_eq!(
            separate.stdout.unwrap().projection.as_bytes(),
            b"out\r\nline\n"
        );
        assert_eq!(separate.stderr.unwrap().projection, "err-no-newline");

        let binary = fixture.execute("printf '\\377\\000A'").await;
        let stream = binary.stdout.unwrap();
        assert_eq!(stream.observed_bytes, 3);
        assert!(stream.projection_had_utf8_replacement);
        let bytes = fixture
            .artifact_store
            .read_verified(stream.artifact.object_reference())
            .unwrap();
        assert_eq!(bytes, [0xff, 0x00, b'A']);
        assert_eq!(stream.artifact.sha256(), Sha256Digest::hash_bytes(&bytes));

        let simultaneous = fixture
            .execute("(head -c 524288 /dev/zero | tr '\\000' o) & (head -c 524288 /dev/zero | tr '\\000' e >&2) & wait")
            .await;
        assert_eq!(simultaneous.stdout.unwrap().observed_bytes, 524_288);
        assert_eq!(simultaneous.stderr.unwrap().observed_bytes, 524_288);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn capture_ceiling_continues_draining_and_projection_is_head_tail() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        for length in [
            HARD_EXECUTION_STREAM_CAPTURE_BYTES - 1,
            HARD_EXECUTION_STREAM_CAPTURE_BYTES,
        ] {
            let exact = fixture
                .execute(format!("head -c {length} /dev/zero | tr '\\000' x"))
                .await;
            let stream = exact.stdout.unwrap();
            assert_eq!(stream.observed_bytes, length);
            assert_eq!(stream.captured_bytes, length);
            assert_eq!(stream.omitted_bytes, 0);
            assert!(!stream.truncated);
        }
        let result = fixture
            .execute(format!(
                "printf HEAD; head -c {} /dev/zero | tr '\\000' x; printf TAIL",
                HARD_EXECUTION_STREAM_CAPTURE_BYTES
            ))
            .await;
        let stream = result.stdout.unwrap();
        assert_eq!(
            stream.observed_bytes,
            HARD_EXECUTION_STREAM_CAPTURE_BYTES + 8
        );
        assert_eq!(stream.captured_bytes, HARD_EXECUTION_STREAM_CAPTURE_BYTES);
        assert_eq!(stream.omitted_bytes, 8);
        assert!(stream.truncated);
        assert_eq!(stream.projection.len(), 32_768);
        assert!(stream.projection.starts_with("HEAD"));
        assert!(stream.projection.ends_with("TAIL"));
        assert_eq!(
            stream.projection_omitted_bytes,
            stream.observed_bytes - 32_768
        );
        let captured = fixture
            .artifact_store
            .read_verified(stream.artifact.object_reference())
            .unwrap();
        assert_eq!(captured.len(), HARD_EXECUTION_STREAM_CAPTURE_BYTES as usize);
        assert!(captured.starts_with(b"HEAD"));
        assert!(!captured.ends_with(b"TAIL"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn exit_signal_spawn_and_request_validation_results_remain_distinct() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        for (command, expected) in [
            ("true", Some(0)),
            ("exit 42", Some(42)),
            ("command-that-does-not-exist", Some(127)),
        ] {
            let result = fixture.execute(command).await;
            assert_eq!(
                result.result_kind,
                crate::ports::workstation::ExecutionResultKind::Exited
            );
            assert_eq!(result.exit_code, expected);
        }
        let signal = fixture.execute("kill -TERM $$").await;
        assert_eq!(
            signal.result_kind,
            crate::ports::workstation::ExecutionResultKind::Signaled
        );
        assert_eq!(
            signal.terminating_signal,
            Some(i64::from(nix::libc::SIGTERM))
        );

        let missing_fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let missing_request = missing_fixture.execution_request("true");
        let missing_shell = missing_fixture
            .workstation
            .with_execution_shell(missing_fixture._root.0.join("missing-bash"));
        let result = missing_shell.execute(missing_request).await.unwrap();
        assert_eq!(
            result.result_kind,
            crate::ports::workstation::ExecutionResultKind::SpawnFailed
        );
        assert!(!result.start_observed);
        assert!(result.cleanup.confirmed());

        let denied_path = fixture._root.0.join("not-executable-bash");
        fs::write(&denied_path, "#!/bin/bash\n").unwrap();
        fs::set_permissions(&denied_path, fs::Permissions::from_mode(0o600)).unwrap();
        let denied_fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let denied_request = denied_fixture.execution_request("true");
        let denied_shell = denied_fixture.workstation.with_execution_shell(denied_path);
        let denied = denied_shell.execute(denied_request).await.unwrap();
        assert_eq!(
            denied.result_kind,
            crate::ports::workstation::ExecutionResultKind::SpawnFailed
        );

        for command in ["", "\0", &"x".repeat(HARD_EXECUTION_COMMAND_MAX_BYTES + 1)] {
            let request = fixture.execution_request(command);
            assert_eq!(
                fixture
                    .workstation
                    .execute(request)
                    .await
                    .unwrap_err()
                    .kind(),
                WorkstationErrorKind::SpawnFailed
            );
        }
        let mut too_long = fixture.execution_request("true");
        too_long.timeout = MonotonicDuration::from_millis(HARD_EXECUTION_TIMEOUT_MS + 1);
        assert_eq!(
            fixture
                .workstation
                .execute(too_long)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::SpawnFailed
        );
        let mut expired = fixture.execution_request("true");
        expired.deadline = MonotonicInstant::from_elapsed(Duration::from_secs(1));
        assert_eq!(
            fixture
                .workstation
                .execute(expired)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::Timeout
        );
        let mut admin = fixture.execution_request("true");
        admin.effective_privilege = PrivilegeMode::Administrative;
        assert_eq!(
            fixture.workstation.execute(admin).await.unwrap_err().kind(),
            WorkstationErrorKind::UnsupportedCapability
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn registry_inspect_duplicate_cancel_repeat_concurrent_and_natural_race_are_coherent() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let started = fixture.workspace_root.join("started");
        let request =
            fixture.execution_request(format!("touch '{}'; exec /bin/sleep 60", started.display()));
        let execution_id = request.execution_id;
        let workstation = fixture.workstation.clone();
        let running = tokio::spawn(async move { workstation.execute(request).await.unwrap() });
        wait_for_path(&started).await;
        let inspection = fixture
            .workstation
            .inspect_execution(ExecutionInspectionRequest {
                operation_id: OperationId::generate(),
                execution_id,
                workstation_id: fixture.workstation.workstation_id(),
                expected_generation: fixture.workstation.generation(),
            })
            .await
            .unwrap();
        assert_eq!(
            inspection.state,
            crate::ports::workstation::ExecutionInspectionState::Running
        );

        let mut duplicate = fixture.execution_request("touch duplicate-must-not-run");
        duplicate.execution_id = execution_id;
        assert_eq!(
            fixture
                .workstation
                .execute(duplicate)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::SpawnFailed
        );
        assert!(
            !fixture
                .workspace_root
                .join("cwd/duplicate-must-not-run")
                .exists()
        );

        let cancel_request = ExecutionCancellationRequest {
            operation_id: OperationId::generate(),
            execution_id,
            workstation_id: fixture.workstation.workstation_id(),
            expected_generation: fixture.workstation.generation(),
        };
        let first = {
            let workstation = fixture.workstation.clone();
            tokio::spawn(async move { workstation.cancel_execution(cancel_request).await.unwrap() })
        };
        let second = {
            let workstation = fixture.workstation.clone();
            tokio::spawn(async move { workstation.cancel_execution(cancel_request).await.unwrap() })
        };
        assert_eq!(
            first.await.unwrap().state,
            crate::ports::workstation::ExecutionCancellationState::Confirmed
        );
        assert_eq!(
            second.await.unwrap().state,
            crate::ports::workstation::ExecutionCancellationState::Confirmed
        );
        let terminal = running.await.unwrap();
        assert_eq!(
            terminal.result_kind,
            crate::ports::workstation::ExecutionResultKind::Cancelled
        );
        assert!(terminal.cleanup.confirmed());
        assert_eq!(
            fixture
                .workstation
                .cancel_execution(cancel_request)
                .await
                .unwrap()
                .state,
            crate::ports::workstation::ExecutionCancellationState::AlreadyTerminal
        );
        tokio::task::yield_now().await;
        assert_eq!(
            fixture
                .workstation
                .cancel_execution(cancel_request)
                .await
                .unwrap()
                .state,
            crate::ports::workstation::ExecutionCancellationState::NotFound
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn caller_drop_keeps_execution_owned_and_shutdown_closes_admission_and_joins() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let started = fixture.workspace_root.join("caller-dropped");
        let request =
            fixture.execution_request(format!("touch '{}'; exec /bin/sleep 60", started.display()));
        let execution_id = request.execution_id;
        let workstation = fixture.workstation.clone();
        let caller = tokio::spawn(async move { workstation.execute(request).await });
        wait_for_path(&started).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            fixture
                .workstation
                .inspect_execution(ExecutionInspectionRequest {
                    operation_id: OperationId::generate(),
                    execution_id,
                    workstation_id: fixture.workstation.workstation_id(),
                    expected_generation: fixture.workstation.generation(),
                })
                .await
                .unwrap()
                .state,
            crate::ports::workstation::ExecutionInspectionState::Running
        );
        let shutdown_deadline = tokio::time::Instant::now() + Duration::from_secs(7);
        fixture
            .workstation
            .begin_execution_shutdown(shutdown_deadline);
        let blocked = fixture.execution_request("touch after-shutdown");
        assert_eq!(
            fixture
                .workstation
                .execute(blocked)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::WorkstationUnavailable
        );
        fixture
            .workstation
            .shutdown_executions_before(shutdown_deadline)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .workstation
                .cancel_execution(ExecutionCancellationRequest {
                    operation_id: OperationId::generate(),
                    execution_id,
                    workstation_id: fixture.workstation.workstation_id(),
                    expected_generation: fixture.workstation.generation(),
                })
                .await
                .unwrap()
                .state,
            crate::ports::workstation::ExecutionCancellationState::NotFound
        );
        assert!(!fixture.workspace_root.join("cwd/after-shutdown").exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_and_execution_reservation_have_only_two_atomic_outcomes() {
        let before_fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let arrived = Arc::new(tokio::sync::Barrier::new(2));
        let release = Arc::new(tokio::sync::Barrier::new(2));
        let workstation = before_fixture.workstation.clone().with_execution_gate(
            execution::ExecutionTestPoint::BeforeReservation,
            Arc::clone(&arrived),
            Arc::clone(&release),
        );
        let marker = before_fixture
            .workspace_root
            .join("cwd/shutdown-won-before-reservation");
        let request = before_fixture.execution_request(format!("touch '{}'", marker.display()));
        let execution_id = request.execution_id;
        let executing = {
            let workstation = workstation.clone();
            tokio::spawn(async move { workstation.execute(request).await })
        };
        arrived.wait().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        workstation.begin_execution_shutdown(deadline);
        assert_eq!(
            workstation
                .inspect_execution(ExecutionInspectionRequest {
                    operation_id: OperationId::generate(),
                    execution_id,
                    workstation_id: workstation.workstation_id(),
                    expected_generation: workstation.generation(),
                })
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InspectionNotFound
        );
        release.wait().await;
        assert_eq!(
            executing.await.unwrap().unwrap_err().kind(),
            WorkstationErrorKind::WorkstationUnavailable
        );
        workstation
            .shutdown_executions_before(deadline)
            .await
            .unwrap();
        assert!(!marker.exists());

        let reserved_fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let arrived = Arc::new(tokio::sync::Barrier::new(2));
        let release = Arc::new(tokio::sync::Barrier::new(2));
        let workstation = reserved_fixture.workstation.clone().with_execution_gate(
            execution::ExecutionTestPoint::AfterReservation,
            Arc::clone(&arrived),
            Arc::clone(&release),
        );
        let marker = reserved_fixture
            .workspace_root
            .join("cwd/reservation-won-before-shutdown");
        let request = reserved_fixture.execution_request(format!("touch '{}'", marker.display()));
        let execution_id = request.execution_id;
        let executing = {
            let workstation = workstation.clone();
            tokio::spawn(async move { workstation.execute(request).await })
        };
        arrived.wait().await;
        assert_eq!(
            workstation
                .inspect_execution(ExecutionInspectionRequest {
                    operation_id: OperationId::generate(),
                    execution_id,
                    workstation_id: workstation.workstation_id(),
                    expected_generation: workstation.generation(),
                })
                .await
                .unwrap()
                .state,
            crate::ports::workstation::ExecutionInspectionState::Running
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        workstation.begin_execution_shutdown(deadline);
        release.wait().await;
        let result = executing.await.unwrap().unwrap();
        assert_eq!(result.execution_id, execution_id);
        assert_eq!(result.result_kind, ExecutionResultKind::Cancelled);
        assert!(!result.start_observed);
        assert!(result.cancelled);
        assert!(!result.timed_out);
        assert_eq!(result.certainty, Certainty::Definite);
        assert!(result.error.is_none());
        assert!(result.stdout.is_none());
        assert!(result.stderr.is_none());
        assert!(result.cleanup.confirmed());
        workstation
            .shutdown_executions_before(deadline)
            .await
            .unwrap();
        assert!(!marker.exists());
        let events = workstation.execution_lifecycle_events();
        assert!(events.contains(&"owned_pre_spawn_cancelled"));
        assert!(!events.contains(&"spawn_claimed"));
        assert!(!events.contains(&"process_group_signalled"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn natural_exit_cleanup_signals_descendants_before_releasing_leader_identity() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let result = fixture.execute("/bin/sleep 60 & exit 0").await;
        assert_eq!(result.result_kind, ExecutionResultKind::Exited);
        assert!(result.cleanup.confirmed());
        assert_stable_process_group_order(&fixture.workstation.execution_lifecycle_events());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn cancellation_cleanup_signals_before_releasing_leader_identity() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let started = fixture.workspace_root.join("cancel-order-started");
        let request =
            fixture.execution_request(format!("touch '{}'; exec /bin/sleep 60", started.display()));
        let execution_id = request.execution_id;
        let workstation = fixture.workstation.clone();
        let running = tokio::spawn(async move { workstation.execute(request).await.unwrap() });
        wait_for_path(&started).await;
        let cancelled = fixture
            .workstation
            .cancel_execution(ExecutionCancellationRequest {
                operation_id: OperationId::generate(),
                execution_id,
                workstation_id: fixture.workstation.workstation_id(),
                expected_generation: fixture.workstation.generation(),
            })
            .await
            .unwrap();
        assert_eq!(cancelled.state, ExecutionCancellationState::Confirmed);
        assert_eq!(
            running.await.unwrap().result_kind,
            ExecutionResultKind::Cancelled
        );
        assert_stable_process_group_order(&fixture.workstation.execution_lifecycle_events());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn timeout_cleanup_signals_before_releasing_leader_identity() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let mut request = fixture.execution_request("exec /bin/sleep 60");
        request.timeout = MonotonicDuration::from_millis(25);
        let result = fixture.workstation.execute(request).await.unwrap();
        assert_eq!(result.result_kind, ExecutionResultKind::TimedOut);
        assert!(result.cleanup.confirmed());
        assert_stable_process_group_order(&fixture.workstation.execution_lifecycle_events());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn shutdown_cleanup_uses_original_deadline_and_releases_identity_after_signals() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let started = fixture.workspace_root.join("shutdown-order-started");
        let request =
            fixture.execution_request(format!("touch '{}'; exec /bin/sleep 60", started.display()));
        let workstation = fixture.workstation.clone();
        let running = tokio::spawn(async move { workstation.execute(request).await.unwrap() });
        wait_for_path(&started).await;
        let stage10_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        fixture
            .workstation
            .begin_execution_shutdown(stage10_deadline);
        fixture
            .workstation
            .shutdown_executions_before(stage10_deadline)
            .await
            .unwrap();
        assert_eq!(
            running.await.unwrap().result_kind,
            ExecutionResultKind::Cancelled
        );
        assert_stable_process_group_order(&fixture.workstation.execution_lifecycle_events());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn stage10_expired_deadline_forces_kill_reports_uncertain_and_joins_before_return() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let started = fixture.workspace_root.join("short-shutdown-started");
        let pid_file = fixture.workspace_root.join("short-shutdown-pid");
        let mut request = fixture.execution_request(format!(
            "echo $$ > '{}'; touch '{}'; trap '' TERM; while :; do :; done",
            pid_file.display(),
            started.display()
        ));
        request.timeout = MonotonicDuration::from_millis(60_000);
        let workstation = fixture.workstation.clone();
        let running = tokio::spawn(async move { workstation.execute(request).await.unwrap() });
        wait_for_path(&started).await;
        let stage10_deadline = tokio::time::Instant::now();
        let began = Instant::now();
        fixture
            .workstation
            .begin_execution_shutdown(stage10_deadline);
        let error = fixture
            .workstation
            .shutdown_executions_before(stage10_deadline)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), WorkstationErrorKind::CleanupFailed);
        assert_eq!(error.certainty(), Certainty::OutcomeUnknown);
        assert!(began.elapsed() < Duration::from_millis(500));
        let result = running.await.unwrap();
        assert_eq!(result.result_kind, ExecutionResultKind::CleanupFailed);
        assert_eq!(result.certainty, Certainty::OutcomeUnknown);
        assert!(!result.cleanup.confirmed());
        let pid = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while kill(Pid::from_raw(pid), None) != Err(nix::errno::Errno::ESRCH) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn repeated_waitid_eintr_yields_to_shutdown_cancellation_and_stage10_deadline() {
        let mut fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let observer = Arc::new(InterruptedLeaderObserver::new());
        fixture.workstation.set_leader_observer(observer.clone());
        let started = fixture.workspace_root.join("eintr-started");
        let pid_file = fixture.workspace_root.join("eintr-pid");
        let request = fixture.execution_request(format!(
            "echo $$ > '{}'; touch '{}'; trap '' TERM; while :; do :; done",
            pid_file.display(),
            started.display()
        ));
        let workstation = fixture.workstation.clone();
        let running = tokio::spawn(async move { workstation.execute(request).await.unwrap() });
        wait_for_path(&started).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while observer.calls.load(Ordering::Relaxed) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let stage10_deadline = tokio::time::Instant::now() + Duration::from_millis(75);
        let began = Instant::now();
        fixture
            .workstation
            .begin_execution_shutdown(stage10_deadline);
        let error = fixture
            .workstation
            .shutdown_executions_before(stage10_deadline)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), WorkstationErrorKind::CleanupFailed);
        assert_eq!(error.certainty(), Certainty::OutcomeUnknown);
        assert!(began.elapsed() < Duration::from_secs(1));

        let result = running.await.unwrap();
        assert!(result.start_observed);
        assert_eq!(result.result_kind, ExecutionResultKind::CleanupFailed);
        assert_eq!(result.certainty, Certainty::OutcomeUnknown);
        assert!(result.cancelled);
        assert!(!result.cleanup.confirmed());
        let calls = observer.calls.load(Ordering::Relaxed);
        assert!(calls >= 3);
        assert!(calls < 100, "cooperative polling made {calls} observations");
        assert!(
            !fixture
                .workstation
                .execution_lifecycle_events()
                .contains(&"leader_terminal_observed")
        );

        let pid = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while kill(Pid::from_raw(pid), None) != Err(nix::errno::Errno::ESRCH) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn timeout_and_completion_remove_background_children_with_term_kill_escalation() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let pid_file = fixture.workspace_root.join("background-pid");
        let completed = fixture
            .execute(format!(
                "/bin/sleep 60 & echo $! > '{}'; exit 0",
                pid_file.display()
            ))
            .await;
        assert_eq!(
            completed.result_kind,
            crate::ports::workstation::ExecutionResultKind::Exited
        );
        assert!(completed.cleanup.confirmed());
        let pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            nix::sys::signal::kill(Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );

        let mut timeout = fixture.execution_request("trap '' TERM; while :; do :; done");
        timeout.timeout = MonotonicDuration::from_millis(50);
        timeout.deadline = MonotonicInstant::from_elapsed(Duration::from_secs(12));
        let timed_out = fixture.workstation.execute(timeout).await.unwrap();
        assert_eq!(
            timed_out.result_kind,
            crate::ports::workstation::ExecutionResultKind::TimedOut
        );
        assert!(timed_out.timed_out);
        assert!(timed_out.cleanup.confirmed());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn execution_debug_and_errors_redact_command_cwd_environment_and_output_canaries() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let command_canary = "command-secret-canary";
        let cwd_canary = fixture.workspace_root.join("cwd").display().to_string();
        let request =
            fixture.execution_request(format!("printf 'output-secret-canary'; # {command_canary}"));
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains(command_canary));
        assert!(!request_debug.contains(&cwd_canary));
        let result = fixture.workstation.execute(request).await.unwrap();
        let result_debug = format!("{result:?}");
        assert!(!result_debug.contains(command_canary));
        assert!(!result_debug.contains("output-secret-canary"));
        assert!(!result_debug.contains(&cwd_canary));
        assert!(result_debug.contains("[REDACTED]"));
        let error = WorkstationError::uncertain(WorkstationErrorKind::CleanupFailed);
        assert_eq!(error.to_string(), "cleanup_failed");
        assert!(!format!("{error:?}").contains(command_canary));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_target_smoke_asserts_platform_nonroot_pseudo_filesystems_and_srv_semantics() {
        assert_eq!(std::env::consts::OS, "linux");
        // SAFETY: `geteuid` has no pointer or lifetime preconditions.
        assert_ne!(unsafe { nix::libc::geteuid() }, 0, "V0 runs non-root");
        let fixture =
            Fixture::with_logical_root(HARD_FILE_READ_MAX_BYTES, "/srv/craxii/workspaces/primary");
        assert_eq!(
            fixture.workstation.logical_workspace_root().canonical(),
            "/srv/craxii/workspaces/primary"
        );
        assert_eq!(
            fixture
                .workstation
                .read_file(fixture.request(
                    LogicalPathReference::absolute("/proc/self/status").unwrap(),
                    DEFAULT_FILE_READ_MAX_BYTES,
                ))
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::InvalidPath
        );

        if std::env::var_os("CRAXII_REQUIRE_UBUNTU_24_04").is_some() {
            let release = fs::read_to_string("/etc/os-release").unwrap();
            assert!(release.lines().any(|line| line == "ID=ubuntu"));
            assert!(release.lines().any(|line| line == "VERSION_ID=\"24.04\""));
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires the deferred Ubuntu 24.04 x86-64 systemd target"]
    async fn linux_target_ubuntu_nonroot_systemd_cgroup_git_and_service_contract() {
        assert_eq!(std::env::consts::ARCH, "x86_64");
        // SAFETY: geteuid has no pointer or lifetime preconditions.
        assert_ne!(unsafe { nix::libc::geteuid() }, 0);
        let release = fs::read_to_string("/etc/os-release").unwrap();
        assert!(release.lines().any(|line| line == "ID=ubuntu"));
        assert!(release.lines().any(|line| line == "VERSION_ID=\"24.04\""));
        assert!(Path::new("/bin/bash").is_file());
        assert!(
            std::process::Command::new("/usr/bin/git")
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        );
        let unit = std::env::var("CRAXII_STAGE13_SYSTEMD_UNIT").unwrap();
        let properties = std::process::Command::new("/usr/bin/systemctl")
            .args([
                "show",
                &unit,
                "--property=User",
                "--property=Delegate",
                "--property=KillMode",
            ])
            .output()
            .unwrap();
        assert!(properties.status.success());
        let properties = String::from_utf8(properties.stdout).unwrap();
        assert!(properties.lines().any(|line| line == "User=craxii"));
        assert!(properties.lines().any(|line| line == "Delegate=yes"));
        assert!(
            properties
                .lines()
                .any(|line| line == "KillMode=control-group")
        );
        let root = PathBuf::from(std::env::var("CRAXII_STAGE13_CGROUP_ROOT").unwrap());
        assert!(observe_execution_support(Path::new("/bin/bash"), false, Some(&root)).cgroup);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires delegated cgroup v2 and reviewed sudo -n policy"]
    async fn linux_target_user_admin_identity_clean_environment_and_cgroup_cleanup() {
        let root = PathBuf::from(std::env::var("CRAXII_STAGE13_CGROUP_ROOT").unwrap());
        let fixture = Fixture::with_execution_target(
            HARD_FILE_READ_MAX_BYTES,
            "/srv/craxii/workspaces/primary",
            true,
            Some(root),
        );
        let flags = fixture.workstation.capabilities_snapshot().flags();
        assert!(flags.foreground_execute());
        assert!(flags.process_group_cleanup());
        assert!(flags.cgroup_cleanup());
        assert!(flags.privilege_administrative());

        let user = fixture.execute("id -u; env | LC_ALL=C sort").await;
        assert_ne!(
            user.stdout.as_ref().unwrap().projection.lines().next(),
            Some("0")
        );
        assert!(user.cleanup.confirmed());
        let mut admin = fixture.execution_request("id -u; env | LC_ALL=C sort");
        admin.effective_privilege = PrivilegeMode::Administrative;
        let admin = fixture.workstation.execute(admin).await.unwrap();
        let output = &admin.stdout.as_ref().unwrap().projection;
        assert_eq!(output.lines().next(), Some("0"));
        for required in ["HOME=/root", "USER=root", "LOGNAME=root", "SHELL=/bin/bash"] {
            assert!(output.lines().any(|line| line == required));
        }
        for forbidden in [
            "OPENAI_API_KEY=",
            "AWS_SECRET_ACCESS_KEY=",
            "SSH_AUTH_SOCK=",
        ] {
            assert!(!output.contains(forbidden));
        }
        assert!(admin.cleanup.confirmed());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires delegated cgroup v2 target for session-escape and stress proof"]
    async fn linux_target_cgroup_kills_session_escape_and_repeated_process_trees() {
        let root = PathBuf::from(std::env::var("CRAXII_STAGE13_CGROUP_ROOT").unwrap());
        let fixture = Fixture::with_execution_target(
            HARD_FILE_READ_MAX_BYTES,
            "/srv/craxii/workspaces/primary",
            false,
            Some(root.clone()),
        );
        for ordinal in 0..25 {
            let pid_file = fixture
                .workspace_root
                .join(format!("escaped-{ordinal}.pid"));
            let result = fixture
                .execute(format!(
                    "/usr/bin/setsid /bin/sleep 60 & echo $! > '{}'; (/bin/sleep 60 &) ; exit 0",
                    pid_file.display()
                ))
                .await;
            assert!(result.cleanup.confirmed(), "iteration {ordinal}");
            let pid: i32 = fs::read_to_string(pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(
                kill(Pid::from_raw(pid), None),
                Err(nix::errno::Errno::ESRCH)
            );
        }
        let residual: Vec<_> = fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("01"))
            .collect();
        assert!(residual.is_empty());
    }

    #[cfg(all(target_os = "linux", feature = "test-failpoints"))]
    #[tokio::test]
    #[ignore = "subprocess-only crash marker probe for the deferred systemd harness"]
    async fn linux_target_crash_marker_probe_execution() {
        let root = PathBuf::from(std::env::var("CRAXII_STAGE13_CGROUP_ROOT").unwrap());
        let fixture = Fixture::with_execution_target(
            HARD_FILE_READ_MAX_BYTES,
            "/srv/craxii/workspaces/primary",
            false,
            Some(root),
        );
        let mut request = fixture
            .execution_request("trap '' TERM; exec -a craxii-stage13-crash-child /bin/sleep 60");
        request.timeout = MonotonicDuration::from_millis(50);
        request.deadline = MonotonicInstant::from_elapsed(Duration::from_secs(12));
        let _ = fixture.workstation.execute(request).await;
        panic!("selected Stage 13 crash marker did not abort the subprocess");
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires disposable Docker/systemd service and crash/restart target harness"]
    fn linux_target_docker_disposable_service_crash_restart_and_reboot_leak_harness() {
        let harness = std::env::var("CRAXII_STAGE13_CRASH_HARNESS").unwrap();
        assert!(Path::new(&harness).is_absolute());
        let status = std::process::Command::new(harness)
            .arg("--verify-target")
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn directory_names(root: &Path) -> BTreeSet<String> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect()
    }
}
