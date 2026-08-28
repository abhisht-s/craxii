//! Narrow process diagnostics used to construct durable runtime-start evidence.

use crate::domain::{DiagnosticPid, LinuxBootId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeProcessObservation {
    pub linux_boot_id: LinuxBootId,
    pub process_id: DiagnosticPid,
}

pub trait RuntimeProcessObserver: Send + Sync {
    fn observe(&self) -> Result<RuntimeProcessObservation, RuntimeObservationError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeObservationError;

impl std::fmt::Display for RuntimeObservationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("runtime process observation failed")
    }
}

impl std::error::Error for RuntimeObservationError {}
