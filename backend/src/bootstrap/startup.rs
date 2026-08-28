use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Display, Formatter};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::artifacts::LocalArtifactStore;
use crate::adapters::runtime_observation::SystemRuntimeProcessObserver;
use crate::adapters::sqlite::{SqliteFailureKind, SqliteRuntimeGuard, SqliteStateStore};
use crate::adapters::system_clock::SystemClock;
use crate::adapters::telemetry::{Telemetry, TelemetryError};
use crate::application::ApplicationShell;
use crate::application::runtime::{
    HeartbeatTask, RuntimeControlError, ShutdownController, bootstrap_runtime,
};
use crate::bootstrap::config;
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{BuildMetadata, ProcessMetadata};
use crate::domain::{
    ConversationId, CorrelationId, CraxiiId, GitRevision, JournalEventId, PackageVersion,
    RuntimeInstanceId, RuntimeStartEvidence, RuntimeStartEvidenceInput, SchemaVersion,
    UtcTimestamp, WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::artifact_store::{ArtifactOrphanReport, ArtifactStore, ArtifactStoreErrorKind};
use crate::ports::clock::Clock;
use crate::ports::runtime_observation::RuntimeProcessObserver;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, LoadOrBootstrapIdentityRequest, RuntimeStateStore,
    StateStoreErrorKind, V0IdentityReference,
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
    let clock = Arc::new(SystemClock::new());
    let build = BuildMetadata::embedded().map_err(|_| StartupError::BuildMetadata)?;
    let process = ProcessMetadata::capture(build, config.fingerprint(), clock.as_ref())
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
    let artifact_store = LocalArtifactStore::initialize(config.paths().artifact_root())
        .map_err(StartupError::from_artifact_initialization)?;
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
    let referenced_artifacts = state_store
        .load_referenced_artifacts()
        .await
        .map_err(StartupError::from_sqlite)?;
    let mut referenced_keys = BTreeSet::new();
    for artifact in &referenced_artifacts {
        artifact_store
            .verify(artifact)
            .map_err(StartupError::from_artifact_integrity)?;
        referenced_keys.insert(artifact.storage_key().clone());
    }
    let orphan_report = artifact_store
        .scan_orphans(&referenced_keys, created_at)
        .map_err(StartupError::from_artifact_integrity)?;
    let snapshot = state_store
        .load_bootstrap_snapshot()
        .await
        .map_err(StartupError::from_state_store)?;
    if snapshot.identity != bootstrap.identity {
        return Err(StartupError::DatabaseIntegrity);
    }
    let observation = SystemRuntimeProcessObserver
        .observe()
        .map_err(|_| StartupError::RuntimeLifecycle)?;
    let runtime_started_at =
        UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| StartupError::Clock)?)
            .map_err(|_| StartupError::Clock)?;
    let runtime_evidence = RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
        runtime_instance_id: RuntimeInstanceId::generate(),
        craxii_id: snapshot.identity.craxii_id,
        workstation_id: snapshot.identity.workstation_id,
        workstation_generation: snapshot.workstation.generation(),
        linux_boot_id: Some(observation.linux_boot_id),
        diagnostic_pid: Some(observation.process_id),
        package_version: PackageVersion::try_new(process.build().package_version())
            .map_err(|_| StartupError::BuildMetadata)?,
        git_revision: GitRevision::try_new(process.build().git_revision())
            .map_err(|_| StartupError::BuildMetadata)?,
        schema_version: SchemaVersion::try_new(3).map_err(|_| StartupError::BuildMetadata)?,
        started_at: runtime_started_at,
    });
    let state_store = Arc::new(state_store);
    let runtime = bootstrap_runtime(
        state_store.as_ref(),
        runtime_evidence,
        orphan_report.orphans.len() as u64,
        clock.as_ref(),
    )
    .await
    .map_err(StartupError::from_runtime)?;
    if let Err(error) = state_store.verify_application_consistency().await {
        if let Ok(wall_time) = clock.utc_now()
            && let Ok(stopped_at) = UtcTimestamp::from_offset_datetime(wall_time)
        {
            let _ = state_store
                .mark_runtime_startup_failure(crate::ports::state_store::FinishRuntimeRequest {
                    runtime_instance_id: runtime.runtime_instance_id,
                    stopped_at,
                })
                .await;
        }
        return Err(StartupError::from_state_store(error));
    }
    if let Err(error) = telemetry.emit_startup_evidence(&process, &health) {
        if let Ok(wall_time) = clock.utc_now()
            && let Ok(stopped_at) = UtcTimestamp::from_offset_datetime(wall_time)
        {
            let _ = state_store
                .mark_runtime_startup_failure(crate::ports::state_store::FinishRuntimeRequest {
                    runtime_instance_id: runtime.runtime_instance_id,
                    stopped_at,
                })
                .await;
        }
        return Err(StartupError::Telemetry(error));
    }
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let heartbeat = HeartbeatTask::start(
        Arc::clone(&state_store),
        Arc::clone(&clock),
        health.clone(),
        runtime.runtime_instance_id,
        fatal,
    );
    let shutdown = Arc::new(ShutdownController::new(
        Arc::clone(&state_store),
        Arc::clone(&clock),
        health.clone(),
        runtime.runtime_instance_id,
        runtime.correlation_id,
        config.shutdown().grace_period_ms(),
        heartbeat,
    ));
    Ok(RunningBootstrap {
        application: ApplicationShell::new(process, health, snapshot),
        sqlite_runtime,
        artifact_store,
        orphan_report,
        runtime_instance_id: runtime.runtime_instance_id,
        shutdown,
        fatal_receiver,
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
pub struct RunningBootstrap {
    application: ApplicationShell,
    sqlite_runtime: SqliteRuntimeGuard,
    artifact_store: LocalArtifactStore,
    orphan_report: ArtifactOrphanReport,
    runtime_instance_id: RuntimeInstanceId,
    shutdown: Arc<ShutdownController<SqliteStateStore, SystemClock>>,
    fatal_receiver: tokio::sync::watch::Receiver<bool>,
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

    #[must_use]
    pub const fn artifact_store(&self) -> &LocalArtifactStore {
        &self.artifact_store
    }

    #[must_use]
    pub const fn artifact_orphan_report(&self) -> &ArtifactOrphanReport {
        &self.orphan_report
    }

    #[must_use]
    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }

    pub async fn wait_for_shutdown_request(&mut self) -> Result<(), StartupError> {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|_| StartupError::RuntimeLifecycle)?;
            tokio::select! {
                result = tokio::signal::ctrl_c() => result.map_err(|_| StartupError::RuntimeLifecycle),
                _ = terminate.recv() => Ok(()),
                result = self.fatal_receiver.changed() => {
                    result.map_err(|_| StartupError::RuntimeLifecycle)
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::select! {
                result = tokio::signal::ctrl_c() => result.map_err(|_| StartupError::RuntimeLifecycle),
                result = self.fatal_receiver.changed() => {
                    result.map_err(|_| StartupError::RuntimeLifecycle)
                }
            }
        }
    }

    pub async fn shutdown(self) -> Result<(), StartupError> {
        self.shutdown
            .request()
            .await
            .map_err(StartupError::from_runtime)?;
        self.shutdown
            .finish()
            .await
            .map_err(StartupError::from_runtime)?;
        self.sqlite_runtime.shutdown().await;
        Ok(())
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
    RuntimeLifecycle,
    Telemetry(TelemetryError),
    #[cfg(all(feature = "test-failpoints", unix))]
    TestControl,
}

impl StartupError {
    const fn from_artifact_initialization(
        _error: crate::ports::artifact_store::ArtifactStoreError,
    ) -> Self {
        Self::DatabaseLifecycle
    }

    const fn from_artifact_integrity(
        error: crate::ports::artifact_store::ArtifactStoreError,
    ) -> Self {
        match error.kind() {
            ArtifactStoreErrorKind::Integrity | ArtifactStoreErrorKind::Collision => {
                Self::DatabaseIntegrity
            }
            ArtifactStoreErrorKind::InvalidRequest
            | ArtifactStoreErrorKind::UnsafeRoot
            | ArtifactStoreErrorKind::UnsupportedFilesystem
            | ArtifactStoreErrorKind::Storage => Self::DatabaseLifecycle,
        }
    }

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
            StateStoreErrorKind::InternalInvariant
            | StateStoreErrorKind::StateConflict
            | StateStoreErrorKind::IdempotencyConflict
            | StateStoreErrorKind::TargetNotFound => Self::DatabaseIntegrity,
            StateStoreErrorKind::Storage => Self::DatabaseLifecycle,
        }
    }

    const fn from_runtime(_error: RuntimeControlError) -> Self {
        Self::RuntimeLifecycle
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
            Self::RuntimeLifecycle => "runtime_lifecycle_failure",
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
