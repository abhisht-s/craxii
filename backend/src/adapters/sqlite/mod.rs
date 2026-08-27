//! File-backed SQLite lifecycle adapter.

mod error;
mod runtime;
#[allow(dead_code)] // Stage 5 establishes the private primitive; Stage 6 named writes consume it.
mod transaction;

pub use error::{SqliteAdapterError, SqliteFailureKind};
pub use runtime::{
    CheckpointReport, DatabaseDisposition, MAX_SUPPORTED_SCHEMA_VERSION, SqliteRuntime,
    SqliteRuntimeGuard,
};
