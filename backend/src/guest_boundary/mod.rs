//! Local guest protection evidence, independent of the frozen worker protocol.
//!
//! Valid configuration describes required facts; it grants no provider or execution authority.
pub mod config;
pub mod storage;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundaryError {
    GuestConfigInvalid,
    DiskIdentityMismatch,
    LayoutUnavailable,
    IsolationUnavailable,
    ResourcePressure,
    CleanupUnknown,
}
pub type BoundaryResult<T> = Result<T, BoundaryError>;
impl BoundaryError {
    pub fn code(self) -> &'static str {
        match self {
            Self::GuestConfigInvalid => "guest_config_invalid",
            Self::DiskIdentityMismatch => "disk_identity_mismatch",
            Self::LayoutUnavailable => "layout_unavailable",
            Self::IsolationUnavailable => "isolation_unavailable",
            Self::ResourcePressure => "resource_pressure",
            Self::CleanupUnknown => "cleanup_unknown",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuiescenceState {
    LocallyQuiescent,
    Busy,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct QuiescenceObservation {
    pub active_tasks: u64,
    pub descendants: Option<u64>,
    pub pending_cleanup: u64,
    pub pending_receipts: u64,
    pub pending_exports: u64,
    pub uncertain_exports: u64,
    pub journal_flushed: bool,
    pub storage_healthy: bool,
    pub visibility_complete: bool,
}
impl QuiescenceObservation {
    /// An observation, never a race-closing sleep authorization.
    pub fn state(self) -> QuiescenceState {
        if self.active_tasks != 0
            || self.pending_cleanup != 0
            || self.pending_receipts != 0
            || self.pending_exports != 0
            || self.uncertain_exports != 0
            || self.descendants.is_some_and(|n| n != 0)
        {
            QuiescenceState::Busy
        } else if !self.journal_flushed
            || !self.storage_healthy
            || !self.visibility_complete
            || self.descendants.is_none()
        {
            QuiescenceState::Unknown
        } else {
            QuiescenceState::LocallyQuiescent
        }
    }
}
/// Combine with journal/export/storage observations; this alone never authorizes idle.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionBoundaryObservation {
    pub active_executions: u64,
    pub descendants: Option<u64>,
    pub pending_cleanup: u64,
    pub visibility_complete: bool,
}
