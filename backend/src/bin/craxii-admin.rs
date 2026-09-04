use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use craxii_server::adapters::artifacts::LocalArtifactStore;
use craxii_server::adapters::sqlite::{
    SqliteEvidenceQueryStore, SqliteRuntimeGuard, SqliteStateStore,
};
use craxii_server::adapters::system_clock::SystemClock;
use craxii_server::application::device_provisioning::DeviceProvisioningService;
use craxii_server::application::evidence_inspection::{
    EvidenceInspectionService, EvidenceOutputFormat,
};
use craxii_server::bootstrap::config;
use craxii_server::domain::{DeviceDisplayName, DeviceId, RuntimeInstanceId, UtcTimestamp, WorkId};
use craxii_server::ports::clock::Clock;
use craxii_server::ports::device_credentials::RevokeDeviceOutcome;
use craxii_server::ports::state_store::BootstrapStateStore;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run(
        std::env::args_os(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{}", error.code());
            ExitCode::FAILURE
        }
    }
}

async fn run(
    arguments: impl IntoIterator<Item = OsString>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), AdminError> {
    let cli = Cli::parse(arguments)?;
    let config = config::load(&cli.config_path).map_err(|_| AdminError::Configuration)?;
    let guard = if cli.action.is_evidence() {
        SqliteRuntimeGuard::start_read_only(config.paths().state_root())
            .await
            .map_err(|_| AdminError::Database)?
    } else {
        SqliteRuntimeGuard::start(
            config.paths().state_root(),
            config.sqlite().pool_connections(),
        )
        .await
        .map_err(|_| AdminError::Database)?
    };
    let store = SqliteStateStore::new(guard.runtime().clone());
    let mut verification_failed = false;
    match cli.action {
        Action::Provision(display_name) => {
            load_snapshot(&store).await?;
            let observed_at = observed_at()?;
            let service = DeviceProvisioningService::new(&store);
            let provisioned = service
                .provision(display_name, observed_at)
                .await
                .map_err(|_| AdminError::DeviceAdministration)?;
            writeln!(
                stderr,
                "device_provisioned\t{}\t{}",
                provisioned.summary.device_id,
                provisioned.summary.display_name.as_str()
            )
            .map_err(|_| AdminError::Output)?;
            provisioned
                .write_bearer_once(stdout)
                .map_err(|_| AdminError::Output)?;
        }
        Action::List => {
            load_snapshot(&store).await?;
            let service = DeviceProvisioningService::new(&store);
            writeln!(
                stdout,
                "device_id\tdisplay_name\tstatus\tcreated_at\tlast_seen_at\trevoked_at"
            )
            .map_err(|_| AdminError::Output)?;
            for device in service
                .list()
                .await
                .map_err(|_| AdminError::DeviceAdministration)?
            {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    device.device_id,
                    device.display_name.as_str(),
                    if device.is_active() {
                        "active"
                    } else {
                        "revoked"
                    },
                    device.created_at,
                    optional_timestamp(device.last_seen_at),
                    optional_timestamp(device.revoked_at),
                )
                .map_err(|_| AdminError::Output)?;
            }
            stdout.flush().map_err(|_| AdminError::Output)?;
        }
        Action::Revoke(device_id) => {
            load_snapshot(&store).await?;
            let observed_at = observed_at()?;
            let service = DeviceProvisioningService::new(&store);
            let outcome = service
                .revoke(device_id, observed_at)
                .await
                .map_err(|_| AdminError::DeviceAdministration)?;
            match outcome {
                RevokeDeviceOutcome::Revoked(device) => writeln!(
                    stdout,
                    "revoked\t{}\t{}",
                    device.device_id,
                    device.revoked_at.ok_or(AdminError::DatabaseIntegrity)?
                ),
                RevokeDeviceOutcome::AlreadyRevoked(device) => writeln!(
                    stdout,
                    "already_revoked\t{}\t{}",
                    device.device_id,
                    device.revoked_at.ok_or(AdminError::DatabaseIntegrity)?
                ),
                RevokeDeviceOutcome::NotFound => writeln!(stdout, "not_found\t{device_id}"),
            }
            .map_err(|_| AdminError::Output)?;
            stdout.flush().map_err(|_| AdminError::Output)?;
        }
        Action::Preflight(format) => {
            let artifacts = LocalArtifactStore::open_read_only(config.paths().artifact_root())
                .map_err(|_| AdminError::ArtifactIntegrity)?;
            let queries = SqliteEvidenceQueryStore::new(guard.runtime().clone());
            let service = EvidenceInspectionService::new(&queries, &artifacts);
            stdout
                .write_all(
                    service
                        .preflight(format)
                        .await
                        .map_err(map_evidence_error)?
                        .as_bytes(),
                )
                .map_err(|_| AdminError::Output)?;
        }
        Action::VerifyState(format) => {
            let artifacts = LocalArtifactStore::open_read_only(config.paths().artifact_root())
                .map_err(|_| AdminError::ArtifactIntegrity)?;
            let queries = SqliteEvidenceQueryStore::new(guard.runtime().clone());
            let service = EvidenceInspectionService::new(&queries, &artifacts);
            let (report, consistent) = service
                .verify_state(format)
                .await
                .map_err(map_evidence_error)?;
            stdout
                .write_all(report.as_bytes())
                .map_err(|_| AdminError::Output)?;
            if !consistent {
                verification_failed = true;
            }
        }
        Action::InspectWork(work_id, format) => {
            let artifacts = LocalArtifactStore::open_read_only(config.paths().artifact_root())
                .map_err(|_| AdminError::ArtifactIntegrity)?;
            let queries = SqliteEvidenceQueryStore::new(guard.runtime().clone());
            let service = EvidenceInspectionService::new(&queries, &artifacts);
            stdout
                .write_all(
                    service
                        .inspect_work(work_id, format)
                        .await
                        .map_err(map_evidence_error)?
                        .as_bytes(),
                )
                .map_err(|_| AdminError::Output)?;
        }
        Action::InspectRuntime(runtime_id, format) => {
            let artifacts = LocalArtifactStore::open_read_only(config.paths().artifact_root())
                .map_err(|_| AdminError::ArtifactIntegrity)?;
            let queries = SqliteEvidenceQueryStore::new(guard.runtime().clone());
            let service = EvidenceInspectionService::new(&queries, &artifacts);
            stdout
                .write_all(
                    service
                        .inspect_runtime(runtime_id, format)
                        .await
                        .map_err(map_evidence_error)?
                        .as_bytes(),
                )
                .map_err(|_| AdminError::Output)?;
        }
        Action::EvidenceExport(format) => {
            let artifacts = LocalArtifactStore::open_read_only(config.paths().artifact_root())
                .map_err(|_| AdminError::ArtifactIntegrity)?;
            let queries = SqliteEvidenceQueryStore::new(guard.runtime().clone());
            let service = EvidenceInspectionService::new(&queries, &artifacts);
            stdout
                .write_all(
                    service
                        .export(format)
                        .await
                        .map_err(map_evidence_error)?
                        .as_bytes(),
                )
                .map_err(|_| AdminError::Output)?;
        }
    }
    stdout.flush().map_err(|_| AdminError::Output)?;
    guard.shutdown().await;
    if verification_failed {
        Err(AdminError::VerificationFailed)
    } else {
        Ok(())
    }
}

async fn load_snapshot(store: &SqliteStateStore) -> Result<(), AdminError> {
    store
        .load_bootstrap_snapshot()
        .await
        .map(|_| ())
        .map_err(|_| AdminError::DatabaseIntegrity)
}

fn observed_at() -> Result<UtcTimestamp, AdminError> {
    let clock = SystemClock::new();
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| AdminError::Clock)?)
        .map_err(|_| AdminError::Clock)
}

fn map_evidence_error(
    error: craxii_server::ports::evidence_query::EvidenceQueryError,
) -> AdminError {
    match error.kind() {
        craxii_server::ports::evidence_query::EvidenceQueryErrorKind::NotFound => {
            AdminError::EvidenceNotFound
        }
        craxii_server::ports::evidence_query::EvidenceQueryErrorKind::Storage => {
            AdminError::Database
        }
        craxii_server::ports::evidence_query::EvidenceQueryErrorKind::Integrity => {
            AdminError::DatabaseIntegrity
        }
    }
}

fn optional_timestamp(value: Option<UtcTimestamp>) -> String {
    value.map_or_else(|| "-".to_owned(), |timestamp| timestamp.to_string())
}

struct Cli {
    config_path: PathBuf,
    action: Action,
}

enum Action {
    Provision(DeviceDisplayName),
    List,
    Revoke(DeviceId),
    Preflight(EvidenceOutputFormat),
    VerifyState(EvidenceOutputFormat),
    InspectWork(WorkId, EvidenceOutputFormat),
    InspectRuntime(RuntimeInstanceId, EvidenceOutputFormat),
    EvidenceExport(EvidenceOutputFormat),
}

impl Action {
    const fn is_evidence(&self) -> bool {
        matches!(
            self,
            Self::Preflight(_)
                | Self::VerifyState(_)
                | Self::InspectWork(_, _)
                | Self::InspectRuntime(_, _)
                | Self::EvidenceExport(_)
        )
    }
}

impl Cli {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, AdminError> {
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        if arguments.next().as_deref() != Some(OsStr::new("--config")) {
            return Err(AdminError::Cli);
        }
        let config_path = PathBuf::from(arguments.next().ok_or(AdminError::Cli)?);
        if config_path.as_os_str().is_empty() {
            return Err(AdminError::Cli);
        }
        let group = arguments.next().ok_or(AdminError::Cli)?;
        let action = if group == OsStr::new("device") {
            let command = arguments.next().ok_or(AdminError::Cli)?;
            parse_device_action(command, &mut arguments)?
        } else if group == OsStr::new("preflight") {
            Action::Preflight(parse_format(&mut arguments)?)
        } else if group == OsStr::new("verify-state") {
            Action::VerifyState(parse_format(&mut arguments)?)
        } else if group == OsStr::new("inspect-work") {
            let id = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            Action::InspectWork(
                WorkId::parse_canonical(&id).map_err(|_| AdminError::Cli)?,
                parse_format(&mut arguments)?,
            )
        } else if group == OsStr::new("inspect-runtime") {
            let id = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            Action::InspectRuntime(
                RuntimeInstanceId::parse_canonical(&id).map_err(|_| AdminError::Cli)?,
                parse_format(&mut arguments)?,
            )
        } else if group == OsStr::new("evidence-export") {
            Action::EvidenceExport(parse_format(&mut arguments)?)
        } else {
            return Err(AdminError::Cli);
        };
        if arguments.next().is_some() {
            return Err(AdminError::Cli);
        }
        Ok(Self {
            config_path,
            action,
        })
    }
}

fn parse_device_action(
    command: OsString,
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<Action, AdminError> {
    if command == OsStr::new("provision") {
        let display_name = arguments
            .next()
            .ok_or(AdminError::Cli)?
            .into_string()
            .map_err(|_| AdminError::Cli)?;
        Ok(Action::Provision(
            DeviceDisplayName::try_new(display_name).map_err(|_| AdminError::Cli)?,
        ))
    } else if command == OsStr::new("list") {
        Ok(Action::List)
    } else if command == OsStr::new("revoke") {
        let device_id = arguments
            .next()
            .ok_or(AdminError::Cli)?
            .into_string()
            .map_err(|_| AdminError::Cli)?;
        Ok(Action::Revoke(
            DeviceId::parse_canonical(&device_id).map_err(|_| AdminError::Cli)?,
        ))
    } else {
        Err(AdminError::Cli)
    }
}

fn parse_format(
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<EvidenceOutputFormat, AdminError> {
    let Some(flag) = arguments.next() else {
        return Ok(EvidenceOutputFormat::Json);
    };
    if flag != OsStr::new("--format") {
        return Err(AdminError::Cli);
    }
    let value = arguments
        .next()
        .ok_or(AdminError::Cli)?
        .into_string()
        .map_err(|_| AdminError::Cli)?;
    EvidenceOutputFormat::parse(&value).ok_or(AdminError::Cli)
}

#[derive(Clone, Copy)]
enum AdminError {
    Cli,
    Configuration,
    Clock,
    Database,
    DatabaseIntegrity,
    DeviceAdministration,
    ArtifactIntegrity,
    EvidenceNotFound,
    VerificationFailed,
    Output,
}

impl AdminError {
    const fn code(self) -> &'static str {
        match self {
            Self::Cli => "invalid_cli",
            Self::Configuration => "invalid_configuration",
            Self::Clock => "clock_failure",
            Self::Database => "database_lifecycle_failure",
            Self::DatabaseIntegrity => "database_integrity_failure",
            Self::DeviceAdministration => "device_administration_failure",
            Self::ArtifactIntegrity => "artifact_integrity_failure",
            Self::EvidenceNotFound => "evidence_not_found",
            Self::VerificationFailed => "verification_failed",
            Self::Output => "output_failure",
        }
    }
}

impl fmt::Debug for AdminError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_device_and_stage23_offline_operations_only() {
        let config = OsString::from("/tmp/config.toml");
        let list = Cli::parse([
            "admin".into(),
            "--config".into(),
            config.clone(),
            "device".into(),
            "list".into(),
        ]);
        assert!(matches!(list.unwrap().action, Action::List));

        let device_id = DeviceId::generate();
        let revoke = Cli::parse([
            "admin".into(),
            "--config".into(),
            config.clone(),
            "device".into(),
            "revoke".into(),
            device_id.to_string().into(),
        ])
        .unwrap();
        assert!(matches!(revoke.action, Action::Revoke(value) if value == device_id));

        let provision = Cli::parse([
            "admin".into(),
            "--config".into(),
            config,
            "device".into(),
            "provision".into(),
            "Office Mac".into(),
        ])
        .unwrap();
        assert!(matches!(provision.action, Action::Provision(_)));

        let preflight = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "preflight".into(),
        ])
        .unwrap();
        assert!(matches!(
            preflight.action,
            Action::Preflight(EvidenceOutputFormat::Json)
        ));

        let verify = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "verify-state".into(),
            "--format".into(),
            "markdown".into(),
        ])
        .unwrap();
        assert!(matches!(
            verify.action,
            Action::VerifyState(EvidenceOutputFormat::Markdown)
        ));

        let work_id = WorkId::generate();
        let inspect_work = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "inspect-work".into(),
            work_id.to_string().into(),
        ])
        .unwrap();
        assert!(matches!(
            inspect_work.action,
            Action::InspectWork(value, EvidenceOutputFormat::Json) if value == work_id
        ));

        let runtime_id = RuntimeInstanceId::generate();
        let inspect_runtime = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "inspect-runtime".into(),
            runtime_id.to_string().into(),
            "--format".into(),
            "markdown".into(),
        ])
        .unwrap();
        assert!(matches!(
            inspect_runtime.action,
            Action::InspectRuntime(value, EvidenceOutputFormat::Markdown) if value == runtime_id
        ));

        let export = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "evidence-export".into(),
        ])
        .unwrap();
        assert!(matches!(
            export.action,
            Action::EvidenceExport(EvidenceOutputFormat::Json)
        ));

        assert!(
            Cli::parse([
                "admin".into(),
                "--config".into(),
                "/tmp/config.toml".into(),
                "evidence-export".into(),
                "--format".into(),
                "unsafe".into(),
            ])
            .is_err()
        );
        assert!(
            Cli::parse([
                "admin".into(),
                "--config".into(),
                "/tmp/config.toml".into(),
                "inspect-work".into(),
                "not-a-work-id".into(),
            ])
            .is_err()
        );
        assert!(Cli::parse(["admin".into(), "device".into(), "list".into()]).is_err());
    }
}
