use std::fmt::{Debug, Display, Formatter};

use crate::domain::NormalizedError;

/// Safe adapter-level SQLite failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteFailureKind {
    UnsafeStatePath,
    UnsupportedFilesystem,
    AlreadyOwned,
    Storage,
    BusyOrLocked,
    Corrupt,
    NewerSchema,
    InconsistentSchema,
    StateConflict,
    IdempotencyConflict,
    TargetNotFound,
    InternalInvariant,
}

/// Redacted SQLite adapter failure. Raw source failures are deliberately discarded.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SqliteAdapterError {
    kind: SqliteFailureKind,
    sqlite_code: Option<i32>,
}

impl SqliteAdapterError {
    #[must_use]
    pub const fn new(kind: SqliteFailureKind) -> Self {
        Self {
            kind,
            sqlite_code: None,
        }
    }

    #[must_use]
    pub const fn kind(self) -> SqliteFailureKind {
        self.kind
    }

    pub(super) const fn sqlite_code(self) -> Option<i32> {
        self.sqlite_code
    }

    #[must_use]
    pub fn normalized(self) -> NormalizedError {
        match self.kind {
            SqliteFailureKind::StateConflict => NormalizedError::state_conflict(),
            SqliteFailureKind::IdempotencyConflict => NormalizedError::idempotency(),
            SqliteFailureKind::TargetNotFound => NormalizedError::client_protocol(),
            SqliteFailureKind::InternalInvariant => NormalizedError::internal_invariant(),
            _ => NormalizedError::storage(None),
        }
    }

    pub(super) fn from_sqlx(error: sqlx::Error) -> Self {
        let sqlite_code = match &error {
            sqlx::Error::Database(database) => {
                database.code().and_then(|code| code.parse::<i32>().ok())
            }
            _ => None,
        };
        let primary = sqlite_code.map(|code| code & 0xff);
        let kind = match primary {
            Some(5 | 6) => SqliteFailureKind::BusyOrLocked,
            Some(11 | 26) => SqliteFailureKind::Corrupt,
            Some(19) => SqliteFailureKind::InternalInvariant,
            _ => SqliteFailureKind::Storage,
        };
        Self { kind, sqlite_code }
    }

    pub(super) fn schema_query(error: sqlx::Error) -> Self {
        let classified = Self::from_sqlx(error);
        if classified.kind == SqliteFailureKind::Corrupt {
            classified
        } else {
            Self {
                kind: SqliteFailureKind::InconsistentSchema,
                sqlite_code: classified.sqlite_code,
            }
        }
    }
}

impl Display for SqliteAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self.kind {
            SqliteFailureKind::UnsafeStatePath => "unsafe SQLite state path",
            SqliteFailureKind::UnsupportedFilesystem => "unsupported SQLite filesystem",
            SqliteFailureKind::AlreadyOwned => "SQLite state is already owned",
            SqliteFailureKind::Storage => "SQLite storage failure",
            SqliteFailureKind::BusyOrLocked => "SQLite storage contention",
            SqliteFailureKind::Corrupt => "SQLite database corruption",
            SqliteFailureKind::NewerSchema => "SQLite schema is newer than this binary",
            SqliteFailureKind::InconsistentSchema => "SQLite schema is inconsistent",
            SqliteFailureKind::StateConflict => "SQLite state conflict",
            SqliteFailureKind::IdempotencyConflict => "SQLite idempotency conflict",
            SqliteFailureKind::TargetNotFound => "SQLite target not found",
            SqliteFailureKind::InternalInvariant => "SQLite internal invariant failure",
        })
    }
}

impl Debug for SqliteAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl std::error::Error for SqliteAdapterError {}

impl From<sqlx::Error> for SqliteAdapterError {
    fn from(error: sqlx::Error) -> Self {
        Self::schema_query(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_error_surfaces_never_include_raw_sentinels_or_numeric_code() {
        let error = SqliteAdapterError {
            kind: SqliteFailureKind::Storage,
            sqlite_code: Some(14),
        };
        let sentinel = "/secret/db.sqlite SELECT * user-content-sentinel";
        for surface in [error.to_string(), format!("{error:?}")] {
            assert!(!surface.contains(sentinel));
            assert!(!surface.contains("db.sqlite"));
            assert!(!surface.contains("SELECT"));
            assert!(!surface.contains("14"));
        }
        assert!(!format!("{}", error.normalized()).contains("14"));
        assert!(std::error::Error::source(&error).is_none());
    }
}
