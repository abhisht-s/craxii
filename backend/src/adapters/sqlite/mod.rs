//! File-backed SQLite lifecycle adapter.

#[allow(dead_code)] // Stage 6 freezes codecs before Stage 7 composes repository reads/writes.
mod codec;
mod error;
#[allow(dead_code)] // Stage 7 owns the primitive; later stages compose additional emitters.
mod journal;
#[allow(dead_code)] // Stage 6 freezes guarded primitives before Stage 7 journal composition.
mod projection;
mod runtime;
mod schema;
mod state_store;
#[allow(dead_code)] // Stage 5 establishes the private primitive; Stage 6 named writes consume it.
mod transaction;

#[cfg(test)]
mod stage6_tests;
#[cfg(test)]
mod stage7_tests;

pub use error::{SqliteAdapterError, SqliteFailureKind};
pub use runtime::{CheckpointReport, SqliteRuntime, SqliteRuntimeGuard};
pub use schema::{DatabaseDisposition, MAX_SUPPORTED_SCHEMA_VERSION};
pub use state_store::SqliteStateStore;
