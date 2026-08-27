use std::fmt::{Display, Formatter};
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    TomlSyntaxOrShape,
    UnsupportedConfigurationVersion {
        found: i64,
        supported: u64,
    },
    UnsafeBind {
        reason: &'static str,
    },
    InvalidPublicUrl {
        reason: &'static str,
    },
    InvalidProviderUrl {
        target: String,
        reason: &'static str,
    },
    InvalidPath {
        field: &'static str,
        reason: &'static str,
    },
    InvalidSqliteTuning {
        field: &'static str,
        reason: &'static str,
    },
    InvalidWorkstationGeneration,
    InvalidLogicalName {
        field: &'static str,
    },
    InvalidCredentialSource {
        reason: &'static str,
    },
    DuplicateCredentialDeclaration {
        credential: String,
    },
    InvalidCredentialRef {
        field: &'static str,
    },
    UndeclaredCredentialRef {
        credential: String,
        target: String,
    },
    DuplicateModelTarget {
        target: String,
    },
    MissingDefaultTarget {
        target: String,
    },
    DisabledDefaultTarget {
        target: String,
    },
    InvalidModelTarget {
        target: String,
        field: &'static str,
        reason: &'static str,
    },
    InvalidModelCapabilityRelationship {
        target: String,
    },
    OutOfBounds {
        field: &'static str,
        minimum: u64,
        maximum: u64,
    },
    CrossFieldLimitInversion {
        lower: &'static str,
        upper: &'static str,
    },
    InvalidShell {
        reason: &'static str,
    },
    InvalidDeviceAuthSource,
    InvalidTracingValue {
        field: &'static str,
    },
    InvalidShutdownDuration,
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "could not read configuration {}: {source}",
                    path.display()
                )
            }
            Self::TomlSyntaxOrShape => {
                formatter.write_str("configuration TOML has invalid syntax or shape")
            }
            Self::UnsupportedConfigurationVersion { found, supported } => write!(
                formatter,
                "unsupported configuration version {found}; supported version is {supported}"
            ),
            Self::UnsafeBind { reason } => write!(formatter, "unsafe server bind: {reason}"),
            Self::InvalidPublicUrl { reason } => {
                write!(formatter, "invalid public base URL: {reason}")
            }
            Self::InvalidProviderUrl { target, reason } => {
                write!(
                    formatter,
                    "invalid provider URL for target {target}: {reason}"
                )
            }
            Self::InvalidPath { field, reason } => {
                write!(formatter, "invalid path for {field}: {reason}")
            }
            Self::InvalidSqliteTuning { field, reason } => {
                write!(formatter, "invalid SQLite setting {field}: {reason}")
            }
            Self::InvalidWorkstationGeneration => {
                formatter.write_str("workstation initial_generation must be in 1..=i64::MAX")
            }
            Self::InvalidLogicalName { field } => {
                write!(formatter, "invalid logical name for {field}")
            }
            Self::InvalidCredentialSource { reason } => {
                write!(
                    formatter,
                    "invalid credential source configuration: {reason}"
                )
            }
            Self::DuplicateCredentialDeclaration { credential } => {
                write!(formatter, "duplicate credential declaration {credential}")
            }
            Self::InvalidCredentialRef { field } => {
                write!(formatter, "invalid logical credential reference in {field}")
            }
            Self::UndeclaredCredentialRef { credential, target } => write!(
                formatter,
                "model target {target} references undeclared credential {credential}"
            ),
            Self::DuplicateModelTarget { target } => {
                write!(formatter, "duplicate model target {target}")
            }
            Self::MissingDefaultTarget { target } => {
                write!(formatter, "default model target {target} is not declared")
            }
            Self::DisabledDefaultTarget { target } => {
                write!(formatter, "default model target {target} is disabled")
            }
            Self::InvalidModelTarget {
                target,
                field,
                reason,
            } => write!(
                formatter,
                "invalid model target {target} field {field}: {reason}"
            ),
            Self::InvalidModelCapabilityRelationship { target } => write!(
                formatter,
                "model target {target} requires reasoning continuation without that capability"
            ),
            Self::OutOfBounds {
                field,
                minimum,
                maximum,
            } => write!(
                formatter,
                "configuration value {field} must be in {minimum}..={maximum}"
            ),
            Self::CrossFieldLimitInversion { lower, upper } => write!(
                formatter,
                "configuration limit {lower} must not exceed {upper}"
            ),
            Self::InvalidShell { reason } => {
                write!(formatter, "invalid shell configuration: {reason}")
            }
            Self::InvalidDeviceAuthSource => {
                formatter.write_str("invalid device authentication source")
            }
            Self::InvalidTracingValue { field } => {
                write!(formatter, "invalid tracing {field}")
            }
            Self::InvalidShutdownDuration => {
                formatter.write_str("shutdown grace_period_ms must be positive")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}
