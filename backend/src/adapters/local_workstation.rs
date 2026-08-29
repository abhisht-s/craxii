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

use crate::domain::{
    CanonicalByteCount, LogicalPathKind, LogicalPathReference, ResolvedPathEvidence, Sha256Digest,
    UtcTimestamp, WorkspaceCapabilityRef, WorkspaceId, WorkspaceIdentity, WorkstationCapabilities,
    WorkstationCapabilitiesInput, WorkstationCapabilityFlags, WorkstationCapabilityFlagsInput,
    WorkstationCapabilityLimits, WorkstationGeneration, WorkstationId, WorkstationIdentity,
};
use crate::ports::clock::Clock;
use crate::ports::workstation::{
    CancellationResult, CapabilitiesRequest, CapabilitiesResult, ExecutionCancellationRequest,
    ExecutionInspection, ExecutionInspectionRequest, ExecutionRequest, ExecutionResult,
    FileEncoding, FileReadRequest, FileReadResult, HARD_FILE_READ_MAX_BYTES, Workstation,
    WorkstationError, WorkstationErrorKind, WorkstationFileType, WorkstationFuture,
};

const READ_BUFFER_BYTES: usize = 16_384;

#[cfg(test)]
type ReadHook = Arc<dyn Fn(ReadHookPoint, &Path) + Send + Sync>;

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
    #[cfg(test)]
    read_hook: Option<ReadHook>,
}

impl LocalWorkstation {
    /// Binds verified durable identity to one explicit configured local workspace root.
    pub fn new(
        workstation: &WorkstationIdentity,
        workspace: &WorkspaceIdentity,
        default_shell: LogicalPathReference,
        configured_workspace_root: &Path,
        read_hard_limit: u64,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, WorkstationError> {
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
            std::fs::canonicalize(configured_workspace_root).map_err(map_constructor_error)?;
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

        let capabilities = stage12_capabilities(
            workstation.workstation_id(),
            workstation.generation(),
            workspace.workspace_id(),
            workspace.logical_root().clone(),
            default_shell,
        )?;

        Ok(Self {
            workstation_id: workstation.workstation_id(),
            generation: workstation.generation(),
            workspace_id: workspace.workspace_id(),
            logical_workspace_root: workspace.logical_root().clone(),
            resolved_workspace_root,
            read_hard_limit,
            capabilities,
            clock,
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

    #[cfg(test)]
    fn with_read_hook(
        mut self,
        hook: impl Fn(ReadHookPoint, &Path) + Send + Sync + 'static,
    ) -> Self {
        self.read_hook = Some(Arc::new(hook));
        self
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
        let result = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| self.validate_workspace(request.workspace_id))
            .and_then(|()| {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            });
        Box::pin(async move { result })
    }

    fn inspect_execution(
        &self,
        request: ExecutionInspectionRequest,
    ) -> WorkstationFuture<'_, ExecutionInspection> {
        let result = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            });
        Box::pin(async move { result })
    }

    fn cancel_execution(
        &self,
        request: ExecutionCancellationRequest,
    ) -> WorkstationFuture<'_, CancellationResult> {
        let result = self
            .validate_identity(request.workstation_id, request.expected_generation)
            .and_then(|()| {
                Err(WorkstationError::new(
                    WorkstationErrorKind::UnsupportedCapability,
                ))
            });
        Box::pin(async move { result })
    }
}

/// Constructs the exact truthful Stage 12 snapshot without process execution.
pub(crate) fn stage12_capabilities(
    workstation_id: WorkstationId,
    generation: WorkstationGeneration,
    workspace_id: WorkspaceId,
    logical_workspace_root: LogicalPathReference,
    default_shell: LogicalPathReference,
) -> Result<WorkstationCapabilities, WorkstationError> {
    WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
        workstation_id,
        generation,
        cpu_architecture: std::env::consts::ARCH.to_owned(),
        os_release: std::env::consts::OS.to_owned(),
        default_shell,
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
        limits: WorkstationCapabilityLimits::try_new(0, 0, 0)
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
        Some(nix::libc::ELOOP | nix::libc::ENXIO | nix::libc::ENODEV | nix::libc::EOPNOTSUPP,)
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;
    use crate::domain::{
        CraxiiId, ExecutionId, HostingProvider, MonotonicDuration, OperationId, PrivilegeMode,
        WorkspaceIdentityInput, WorkstationIdentityInput,
    };
    use crate::ports::clock::{MonotonicInstant, TestClock};
    use crate::ports::workstation::{
        DEFAULT_FILE_READ_MAX_BYTES, ExecutionCapturePolicy, ExecutionCleanupPolicy,
        ExecutionStdinPolicy,
    };

    const AT: &str = "2026-08-29T01:02:03.456789Z";

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
        clock: Arc<TestClock>,
    }

    impl Fixture {
        fn new(read_hard_limit: u64) -> Self {
            Self::with_logical_root(read_hard_limit, "/logical/primary")
        }

        fn with_logical_root(read_hard_limit: u64, logical_root: &str) -> Self {
            let root = TestRoot::new();
            let workspace_root = root.workspace();
            fs::create_dir(&workspace_root).unwrap();
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
            let workstation = LocalWorkstation::new(
                &workstation_identity,
                &workspace,
                LogicalPathReference::absolute("/bin/sh").unwrap(),
                &workspace_root,
                read_hard_limit,
                clock.clone(),
            )
            .unwrap();
            Self {
                _root: root,
                workspace_root,
                workstation,
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
    async fn capabilities_are_exact_truthful_stage12_runtime_facts() {
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
        assert!(!flags.foreground_execute());
        assert!(!flags.cancel_execution());
        assert!(!flags.inspect_execution());
        assert!(!flags.privilege_administrative());
        assert!(!flags.process_group_cleanup());
        assert!(!flags.cgroup_cleanup());
        assert_eq!(capabilities.limits().max_execution_timeout_ms(), 0);
        assert_eq!(capabilities.limits().max_stdout_bytes(), 0);
        assert_eq!(capabilities.limits().max_stderr_bytes(), 0);
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
    async fn stage13_methods_are_guarded_and_unsupported_with_zero_process_mutation() {
        let fixture = Fixture::new(HARD_FILE_READ_MAX_BYTES);
        let marker = fixture.workspace_root.join("must-not-exist");
        let operation_id = OperationId::generate();
        let execution_id = ExecutionId::generate();
        let execute = ExecutionRequest {
            operation_id,
            execution_id,
            workstation_id: fixture.workstation.workstation_id(),
            expected_generation: fixture.workstation.generation(),
            workspace_id: fixture.workstation.workspace_id(),
            command: format!("touch {}", marker.display()),
            requested_cwd: LogicalPathReference::workspace_relative("src").unwrap(),
            effective_privilege: PrivilegeMode::User,
            environment: Vec::new(),
            stdin: ExecutionStdinPolicy::Closed,
            timeout: MonotonicDuration::from_millis(1_000),
            deadline: MonotonicInstant::from_elapsed(Duration::from_secs(30)),
            capture: ExecutionCapturePolicy {
                stdout_max_bytes: 0,
                stderr_max_bytes: 0,
            },
            cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
        };
        assert_eq!(
            fixture
                .workstation
                .execute(execute)
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::UnsupportedCapability
        );
        assert!(!marker.exists());
        assert_eq!(
            fixture
                .workstation
                .inspect_execution(ExecutionInspectionRequest {
                    operation_id,
                    execution_id,
                    workstation_id: fixture.workstation.workstation_id(),
                    expected_generation: fixture.workstation.generation(),
                })
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::UnsupportedCapability
        );
        assert_eq!(
            fixture
                .workstation
                .cancel_execution(ExecutionCancellationRequest {
                    operation_id,
                    execution_id,
                    workstation_id: fixture.workstation.workstation_id(),
                    expected_generation: fixture.workstation.generation(),
                })
                .await
                .unwrap_err()
                .kind(),
            WorkstationErrorKind::UnsupportedCapability
        );
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

    fn directory_names(root: &Path) -> BTreeSet<String> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect()
    }
}
