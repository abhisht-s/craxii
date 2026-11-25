//! Central signed running authority. No provider client or local funding authority.
pub mod authorization;
pub mod watchdog;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeError(pub &'static str);
pub type ComputeResult<T> = Result<T, ComputeError>;
pub(crate) fn denied() -> ComputeError {
    ComputeError("compute_authorization_denied")
}

#[cfg(target_os = "linux")]
pub mod worker;
