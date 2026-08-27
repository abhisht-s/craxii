use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Display, Formatter};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::adapters::sqlite::{SqliteFailureKind, SqliteRuntimeGuard};
use crate::adapters::system_clock::SystemClock;
use crate::adapters::telemetry::{Telemetry, TelemetryError};
use crate::application::ApplicationShell;
use crate::bootstrap::config;
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{BuildMetadata, ProcessMetadata};

pub async fn run_from_env() -> Result<RunningBootstrap, StartupError> {
    let arguments: Vec<_> = std::env::args_os().collect();

    #[cfg(all(feature = "test-failpoints", unix))]
    if arguments
        .get(1)
        .is_some_and(|argument| argument == OsStr::new(crate::test_failpoints::CONTROL_ARGUMENT))
    {
        if arguments.len() != 2 {
            return Err(StartupError::TestControl);
        }
        crate::test_failpoints::run_controlled_startup().map_err(|_| StartupError::TestControl)?;
        return Err(StartupError::TestControl);
    }

    run(arguments).await
}

pub async fn run(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<RunningBootstrap, StartupError> {
    let cli = Cli::parse(arguments)?;
    let config = config::load(&cli.config_path).map_err(|_| StartupError::Configuration)?;
    let clock = SystemClock::new();
    let build = BuildMetadata::embedded().map_err(|_| StartupError::BuildMetadata)?;
    let process = ProcessMetadata::capture(build, config.fingerprint(), &clock)
        .map_err(|_| StartupError::Clock)?;
    let health = Health::new();
    let telemetry =
        Telemetry::initialize_global(config.tracing()).map_err(StartupError::Telemetry)?;
    let sqlite_runtime = SqliteRuntimeGuard::start(
        config.paths().state_root(),
        config.sqlite().pool_connections(),
    )
    .await
    .map_err(StartupError::from_sqlite)?;
    telemetry
        .emit_startup_evidence(&process, &health)
        .map_err(StartupError::Telemetry)?;

    Ok(RunningBootstrap {
        application: ApplicationShell::new(process, health),
        sqlite_runtime,
    })
}

/// Successful Stage 5 bootstrap ownership.
///
/// This guard keeps the database pool and process lock alive without making the application layer
/// depend on an outward adapter. Its application remains live but deliberately unready.
#[derive(Debug)]
pub struct RunningBootstrap {
    application: ApplicationShell,
    sqlite_runtime: SqliteRuntimeGuard,
}

impl RunningBootstrap {
    #[must_use]
    pub const fn application(&self) -> &ApplicationShell {
        &self.application
    }

    #[must_use]
    pub const fn sqlite_runtime(&self) -> &SqliteRuntimeGuard {
        &self.sqlite_runtime
    }

    pub async fn shutdown(self) {
        self.sqlite_runtime.shutdown().await;
    }
}

pub fn write_fatal_diagnostic(writer: &mut impl Write, error: &StartupError) -> io::Result<()> {
    writeln!(writer, "craxii fatal: {}", error.code())
}

struct Cli {
    config_path: PathBuf,
}

impl Cli {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, StartupError> {
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        let option = arguments.next().ok_or(StartupError::Cli)?;
        if option != OsStr::new("--config") {
            return Err(StartupError::Cli);
        }
        let value = arguments.next().ok_or(StartupError::Cli)?;
        if value.is_empty() || arguments.next().is_some() {
            return Err(StartupError::Cli);
        }
        Ok(Self {
            config_path: PathBuf::from(value),
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum StartupError {
    Cli,
    Configuration,
    BuildMetadata,
    Clock,
    DatabaseLifecycle,
    IncompatibleSchema,
    StateRootAlreadyOwned,
    DatabaseIntegrity,
    Telemetry(TelemetryError),
    #[cfg(all(feature = "test-failpoints", unix))]
    TestControl,
}

impl StartupError {
    const fn from_sqlite(error: crate::adapters::sqlite::SqliteAdapterError) -> Self {
        match error.kind() {
            SqliteFailureKind::AlreadyOwned => Self::StateRootAlreadyOwned,
            SqliteFailureKind::NewerSchema => Self::IncompatibleSchema,
            SqliteFailureKind::Corrupt | SqliteFailureKind::InconsistentSchema => {
                Self::DatabaseIntegrity
            }
            _ => Self::DatabaseLifecycle,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Cli => "invalid_cli",
            Self::Configuration => "invalid_configuration",
            Self::BuildMetadata => "invalid_build_metadata",
            Self::Clock => "clock_failure",
            Self::DatabaseLifecycle => "database_lifecycle_failure",
            Self::IncompatibleSchema => "incompatible_database_schema",
            Self::StateRootAlreadyOwned => "state_root_already_owned",
            Self::DatabaseIntegrity => "database_integrity_failure",
            Self::Telemetry(TelemetryError::GlobalSubscriberConflict) => {
                "telemetry_subscriber_conflict"
            }
            Self::Telemetry(TelemetryError::SinkFailure) => "telemetry_sink_failure",
            #[cfg(all(feature = "test-failpoints", unix))]
            Self::TestControl => "invalid_test_control",
        }
    }
}

impl Display for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Debug for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for StartupError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_requires_exact_config_pair() {
        assert!(Cli::parse(["server".into(), "--config".into(), "config.toml".into()]).is_ok());
        for arguments in [
            vec!["server".into()],
            vec!["server".into(), "config.toml".into()],
            vec!["server".into(), "--config".into()],
            vec!["server".into(), "--config".into(), "".into()],
            vec![
                "server".into(),
                "--config".into(),
                "config.toml".into(),
                "extra".into(),
            ],
        ] {
            assert!(matches!(Cli::parse(arguments), Err(StartupError::Cli)));
        }
    }

    #[test]
    fn fatal_diagnostics_are_fixed_safe_codes() {
        let mut output = Vec::new();
        write_fatal_diagnostic(&mut output, &StartupError::Configuration).unwrap();
        assert_eq!(output, b"craxii fatal: invalid_configuration\n");
    }

    #[test]
    fn sqlite_failures_map_to_fixed_startup_categories() {
        for (kind, expected) in [
            (
                SqliteFailureKind::AlreadyOwned,
                StartupError::StateRootAlreadyOwned,
            ),
            (
                SqliteFailureKind::NewerSchema,
                StartupError::IncompatibleSchema,
            ),
            (SqliteFailureKind::Corrupt, StartupError::DatabaseIntegrity),
            (
                SqliteFailureKind::InconsistentSchema,
                StartupError::DatabaseIntegrity,
            ),
            (
                SqliteFailureKind::UnsafeStatePath,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::UnsupportedFilesystem,
                StartupError::DatabaseLifecycle,
            ),
            (SqliteFailureKind::Storage, StartupError::DatabaseLifecycle),
            (
                SqliteFailureKind::BusyOrLocked,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::StateConflict,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::InternalInvariant,
                StartupError::DatabaseLifecycle,
            ),
        ] {
            let error =
                StartupError::from_sqlite(crate::adapters::sqlite::SqliteAdapterError::new(kind));
            assert_eq!(error, expected);
            assert!(!error.code().contains('/'));
            assert!(std::error::Error::source(&error).is_none());
        }
    }
}
