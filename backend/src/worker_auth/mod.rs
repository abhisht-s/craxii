//! Authentication-only distribution client. No scheduler, customer SQLite, or provider credentials.
pub mod client;
pub mod key_store;
pub mod state;
use std::fmt;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthError {
    UnsafeStorage,
    KeyUnavailable,
    InvalidContext,
    Transport,
    SessionExpired,
    Revoked,
    Stale,
    Unavailable,
    ResultMissing,
    ResultConflict,
}
impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsafeStorage => "worker storage unsafe",
            Self::KeyUnavailable => "worker key recovery required",
            Self::InvalidContext => "worker context invalid",
            Self::SessionExpired => "worker session expired",
            Self::Transport => "worker transport unavailable",
            Self::Revoked => "worker authority revoked",
            Self::Stale => "worker installation stale",
            Self::Unavailable => "worker service unavailable",
            Self::ResultMissing => "worker result reconciliation required",
            Self::ResultConflict => "worker result conflict",
        })
    }
}
impl std::error::Error for AuthError {}
pub type AuthResult<T> = Result<T, AuthError>;
