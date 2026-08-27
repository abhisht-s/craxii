//! Lexical POSIX logical-path identity with no filesystem access.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{DomainValidationError, DomainValidationKind};

/// Maximum canonical UTF-8 bytes in a logical or resolved path value.
pub const MAX_LOGICAL_PATH_BYTES: usize = 4_096;

/// The two explicit V0 logical path-reference kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalPathKind {
    /// A path interpreted relative to an injected workspace root.
    WorkspaceRelative,
    /// An explicit POSIX absolute machine path.
    Absolute,
}

/// A lexically canonical POSIX logical path reference.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct LogicalPathReference {
    kind: LogicalPathKind,
    canonical: String,
}

impl LogicalPathReference {
    /// Lexically canonicalizes a workspace-relative path.
    pub fn workspace_relative(input: impl Into<String>) -> Result<Self, DomainValidationError> {
        Self::parse(LogicalPathKind::WorkspaceRelative, input.into())
    }

    /// Lexically canonicalizes an absolute POSIX path.
    pub fn absolute(input: impl Into<String>) -> Result<Self, DomainValidationError> {
        Self::parse(LogicalPathKind::Absolute, input.into())
    }

    fn parse(kind: LogicalPathKind, input: String) -> Result<Self, DomainValidationError> {
        if input.contains('\0') || input.contains('\\') {
            return Err(invalid_path());
        }

        let starts_absolute = input.starts_with('/');
        if matches!(kind, LogicalPathKind::Absolute) != starts_absolute {
            return Err(invalid_path());
        }

        let mut segments: Vec<&str> = Vec::new();
        for segment in input.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none()
                        && matches!(kind, LogicalPathKind::WorkspaceRelative)
                    {
                        return Err(invalid_path());
                    }
                }
                normal => segments.push(normal),
            }
        }

        let canonical = match kind {
            LogicalPathKind::WorkspaceRelative => {
                if segments.is_empty() {
                    return Err(invalid_path());
                }
                segments.join("/")
            }
            LogicalPathKind::Absolute => {
                if segments.is_empty() {
                    "/".to_owned()
                } else {
                    format!("/{}", segments.join("/"))
                }
            }
        };

        if canonical.len() > MAX_LOGICAL_PATH_BYTES {
            return Err(invalid_path());
        }

        Ok(Self { kind, canonical })
    }

    /// Returns the explicit reference kind.
    #[must_use]
    pub const fn kind(&self) -> LogicalPathKind {
        self.kind
    }

    /// Returns the exact canonical UTF-8 representation.
    #[must_use]
    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// Returns whether this reference is absolute.
    #[must_use]
    pub const fn is_absolute(&self) -> bool {
        matches!(self.kind, LogicalPathKind::Absolute)
    }
}

impl fmt::Debug for LogicalPathReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LogicalPathReference")
            .field("kind", &self.kind)
            .field("canonical", &"[REDACTED]")
            .finish()
    }
}

fn invalid_path() -> DomainValidationError {
    DomainValidationError::new(DomainValidationKind::InvalidLogicalPath)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_relative_paths_normalize_lexically() {
        let cases = [
            ("src/lib.rs", "src/lib.rs"),
            ("src//domain///mod.rs", "src/domain/mod.rs"),
            ("./src/./lib.rs", "src/lib.rs"),
            ("src/domain/../lib.rs", "src/lib.rs"),
            ("src/lib.rs/", "src/lib.rs"),
            ("資料/設計.md", "資料/設計.md"),
        ];
        for (input, expected) in cases {
            let path = LogicalPathReference::workspace_relative(input).unwrap();
            assert_eq!(path.kind(), LogicalPathKind::WorkspaceRelative);
            assert_eq!(path.canonical(), expected);
            assert!(!path.is_absolute());
        }
    }

    #[test]
    fn workspace_relative_escape_and_empty_results_are_rejected() {
        for input in ["/etc/passwd", "..", "../src", "src/../..", "", ".", "a/.."] {
            assert_eq!(
                LogicalPathReference::workspace_relative(input)
                    .unwrap_err()
                    .kind(),
                DomainValidationKind::InvalidLogicalPath,
                "unexpected result for {input:?}"
            );
        }
    }

    #[test]
    fn absolute_paths_normalize_and_clamp_at_root() {
        let cases = [
            ("/", "/"),
            ("///", "/"),
            ("/etc//hosts/", "/etc/hosts"),
            ("/a/./b/../c", "/a/c"),
            ("/../../etc/hosts", "/etc/hosts"),
            ("/資料/設計.md", "/資料/設計.md"),
        ];
        for (input, expected) in cases {
            let path = LogicalPathReference::absolute(input).unwrap();
            assert_eq!(path.kind(), LogicalPathKind::Absolute);
            assert_eq!(path.canonical(), expected);
            assert!(path.is_absolute());
        }
        assert!(LogicalPathReference::absolute("relative/path").is_err());
    }

    #[test]
    fn backslash_and_nul_are_rejected_for_both_kinds() {
        for result in [
            LogicalPathReference::workspace_relative("src\\lib.rs"),
            LogicalPathReference::workspace_relative("src/\0/lib.rs"),
            LogicalPathReference::absolute("/tmp\\file"),
            LogicalPathReference::absolute("/tmp/\0/file"),
        ] {
            assert_eq!(
                result.unwrap_err().kind(),
                DomainValidationKind::InvalidLogicalPath
            );
        }
    }

    #[test]
    fn canonical_utf8_boundary_is_exact() {
        let exact = "a".repeat(MAX_LOGICAL_PATH_BYTES);
        assert_eq!(
            LogicalPathReference::workspace_relative(&exact)
                .unwrap()
                .canonical()
                .len(),
            MAX_LOGICAL_PATH_BYTES
        );
        let over = "a".repeat(MAX_LOGICAL_PATH_BYTES + 1);
        assert!(LogicalPathReference::workspace_relative(over).is_err());

        let unicode_exact = format!("{}a", "é".repeat(2_047));
        assert_eq!(unicode_exact.len(), 4_095);
        assert!(LogicalPathReference::workspace_relative(unicode_exact).is_ok());
    }

    #[test]
    fn debug_redacts_path_text() {
        let sentinel = "secret-workspace/path";
        let path = LogicalPathReference::workspace_relative(sentinel).unwrap();
        let debug = format!("{path:?}");
        assert!(!debug.contains(sentinel));
        assert!(debug.contains("[REDACTED]"));
    }
}
