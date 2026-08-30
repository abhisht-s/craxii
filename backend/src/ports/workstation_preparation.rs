//! Provider-neutral, non-side-effecting preparation before durable tool dispatch.

use std::future::Future;
use std::pin::Pin;

use crate::domain::{
    LogicalPathReference, OperationId, PrivilegeMode, ResolvedPathEvidence, WorkspaceId,
    WorkstationGeneration, WorkstationId,
};
use crate::ports::workstation::WorkstationError;

/// Boxed future used without adding an async-trait dependency.
pub type WorkstationPreparationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WorkstationError>> + Send + 'a>>;

/// The exact machine capability whose feasibility preparation must confirm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredWorkstationCapability {
    FilesystemRead,
    ForegroundExecute,
}

/// Identity, capability, privilege, and cwd facts prepared before dispatch intent.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkstationPreparationRequest {
    pub operation_id: OperationId,
    pub workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
    pub workspace_id: WorkspaceId,
    pub requested_cwd: LogicalPathReference,
    pub required_capability: RequiredWorkstationCapability,
    pub effective_privilege: PrivilegeMode,
}

impl std::fmt::Debug for WorkstationPreparationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkstationPreparationRequest")
            .field("operation_id", &self.operation_id)
            .field("workstation_id", &self.workstation_id)
            .field("expected_generation", &self.expected_generation)
            .field("workspace_id", &self.workspace_id)
            .field("requested_cwd", &"[REDACTED]")
            .field("required_capability", &self.required_capability)
            .field("effective_privilege", &self.effective_privilege)
            .finish()
    }
}

/// Adapter-observed evidence produced without executing the requested tool action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstationPreparationResult {
    pub operation_id: OperationId,
    pub prepared_cwd: PreparedCwdEvidence,
}

/// Closed file-type proof for a prepared cwd object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedCwdObjectType {
    Directory,
}

/// Stable serializable filesystem identity observed from an opened directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedCwdObjectIdentity {
    device: u64,
    inode: u64,
    object_type: PreparedCwdObjectType,
}

impl PreparedCwdObjectIdentity {
    pub fn try_new(
        device: u64,
        inode: u64,
        object_type: PreparedCwdObjectType,
    ) -> Result<Self, WorkstationError> {
        if inode == 0 {
            return Err(WorkstationError::new(
                crate::ports::workstation::WorkstationErrorKind::InvalidPath,
            ));
        }
        Ok(Self {
            device,
            inode,
            object_type,
        })
    }

    #[must_use]
    pub const fn device(self) -> u64 {
        self.device
    }

    #[must_use]
    pub const fn inode(self) -> u64 {
        self.inode
    }

    #[must_use]
    pub const fn object_type(self) -> PreparedCwdObjectType {
        self.object_type
    }
}

/// Durable Stage 14 cwd binding: canonical path evidence plus stable object identity.
#[derive(Clone, Eq, PartialEq)]
pub struct PreparedCwdEvidence {
    resolved_cwd: ResolvedPathEvidence,
    object_identity: PreparedCwdObjectIdentity,
}

impl PreparedCwdEvidence {
    #[must_use]
    pub const fn new(
        resolved_cwd: ResolvedPathEvidence,
        object_identity: PreparedCwdObjectIdentity,
    ) -> Self {
        Self {
            resolved_cwd,
            object_identity,
        }
    }

    #[must_use]
    pub const fn resolved_cwd(&self) -> &ResolvedPathEvidence {
        &self.resolved_cwd
    }

    #[must_use]
    pub const fn object_identity(&self) -> PreparedCwdObjectIdentity {
        self.object_identity
    }
}

impl std::fmt::Debug for PreparedCwdEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedCwdEvidence")
            .field("resolved_cwd", &"[REDACTED]")
            .field("object_identity", &self.object_identity)
            .finish()
    }
}

/// Separate preparation lifecycle seam; this is deliberately not a Workstation method.
pub trait WorkstationPreparation: Send + Sync {
    fn prepare(
        &self,
        request: WorkstationPreparationRequest,
    ) -> WorkstationPreparationFuture<'_, WorkstationPreparationResult>;
}
