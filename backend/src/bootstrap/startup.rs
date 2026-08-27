use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Display, Formatter};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::adapters::sqlite::{SqliteFailureKind, SqliteRuntimeGuard, SqliteStateStore};
use crate::adapters::system_clock::SystemClock;
use crate::adapters::telemetry::{Telemetry, TelemetryError};
use crate::application::ApplicationShell;
use crate::bootstrap::config;
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{BuildMetadata, ProcessMetadata};
use crate::domain::{
    ConversationId, CorrelationId, CraxiiId, JournalEventId, UtcTimestamp, WorkspaceId,
    WorkstationGeneration, WorkstationId,
};
use crate::ports::clock::Clock;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, LoadOrBootstrapIdentityRequest, StateStoreErrorKind,
    V0IdentityReference,
};

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
    let observation = bootstrap_observation(&config)?;
    let created_at =
        UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| StartupError::Clock)?)
            .map_err(|_| StartupError::Clock)?;
    let state_store = SqliteStateStore::new(sqlite_runtime.runtime().clone());
    let bootstrap = state_store
        .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
            proposed: V0IdentityReference {
                craxii_id: CraxiiId::generate(),
                conversation_id: ConversationId::generate(),
                workstation_id: WorkstationId::generate(),
                workspace_id: WorkspaceId::generate(),
            },
            initialized_event_id: JournalEventId::generate(),
            conversation_created_event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
            created_at,
            observation,
        })
        .await
        .map_err(StartupError::from_state_store)?;
    let _consistency = state_store
        .verify_application_consistency()
        .await
        .map_err(StartupError::from_state_store)?;
    let snapshot = state_store
        .load_bootstrap_snapshot()
        .await
        .map_err(StartupError::from_state_store)?;
    if snapshot.identity != bootstrap.identity {
        return Err(StartupError::DatabaseIntegrity);
    }
    telemetry
        .emit_startup_evidence(&process, &health)
        .map_err(StartupError::Telemetry)?;

    Ok(RunningBootstrap {
        application: ApplicationShell::new(process, health, snapshot),
        sqlite_runtime,
    })
}

fn bootstrap_observation(
    config: &config::ValidatedConfig,
) -> Result<BootstrapObservation, StartupError> {
    let workspace_root = config
        .paths()
        .primary_workspace_root()
        .to_str()
        .ok_or(StartupError::Configuration)?
        .to_owned();
    let default_shell = config
        .shell()
        .executable()
        .to_str()
        .ok_or(StartupError::Configuration)?
        .to_owned();
    let generation = i64::try_from(config.workstation().initial_generation())
        .map_err(|_| StartupError::Configuration)?;
    Ok(BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(generation)
            .map_err(|_| StartupError::Configuration)?,
        architecture: std::env::consts::ARCH.to_owned(),
        os_release: std::env::consts::OS.to_owned(),
        default_shell,
        workspace_logical_name: config
            .workstation()
            .primary_workspace_logical_name()
            .to_owned(),
        workspace_logical_root: workspace_root.clone(),
        workspace_resolved_root: workspace_root,
        max_execution_timeout_ms: config.limits().tools().run_shell_max_timeout_ms(),
        max_stdout_bytes: config.limits().tools().stdout_capture_bytes(),
        max_stderr_bytes: config.limits().tools().stderr_capture_bytes(),
        administrative_enabled: config.shell().administrative_enabled(),
    })
}

/// Successful Stage 7 bootstrap ownership.
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

    const fn from_state_store(error: crate::ports::state_store::StateStoreError) -> Self {
        match error.kind() {
            StateStoreErrorKind::InternalInvariant | StateStoreErrorKind::StateConflict => {
                Self::DatabaseIntegrity
            }
            StateStoreErrorKind::Storage => Self::DatabaseLifecycle,
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
