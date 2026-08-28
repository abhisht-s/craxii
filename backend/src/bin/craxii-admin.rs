use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use craxii_server::adapters::sqlite::{SqliteRuntimeGuard, SqliteStateStore};
use craxii_server::adapters::system_clock::SystemClock;
use craxii_server::application::device_provisioning::DeviceProvisioningService;
use craxii_server::bootstrap::config;
use craxii_server::domain::{DeviceDisplayName, DeviceId, UtcTimestamp};
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
    let guard = SqliteRuntimeGuard::start(
        config.paths().state_root(),
        config.sqlite().pool_connections(),
    )
    .await
    .map_err(|_| AdminError::Database)?;
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_bootstrap_snapshot()
        .await
        .map_err(|_| AdminError::DatabaseIntegrity)?;
    let clock = SystemClock::new();
    let observed_at =
        UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| AdminError::Clock)?)
            .map_err(|_| AdminError::Clock)?;
    let service = DeviceProvisioningService::new(&store);
    match cli.action {
        Action::Provision(display_name) => {
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
    }
    guard.shutdown().await;
    Ok(())
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
}

impl Cli {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, AdminError> {
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        if arguments.next().as_deref() != Some(OsStr::new("--config")) {
            return Err(AdminError::Cli);
        }
        let config_path = PathBuf::from(arguments.next().ok_or(AdminError::Cli)?);
        if config_path.as_os_str().is_empty()
            || arguments.next().as_deref() != Some(OsStr::new("device"))
        {
            return Err(AdminError::Cli);
        }
        let command = arguments.next().ok_or(AdminError::Cli)?;
        let action = if command == OsStr::new("provision") {
            let display_name = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            Action::Provision(
                DeviceDisplayName::try_new(display_name).map_err(|_| AdminError::Cli)?,
            )
        } else if command == OsStr::new("list") {
            Action::List
        } else if command == OsStr::new("revoke") {
            let device_id = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            Action::Revoke(DeviceId::parse_canonical(&device_id).map_err(|_| AdminError::Cli)?)
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

#[derive(Clone, Copy)]
enum AdminError {
    Cli,
    Configuration,
    Clock,
    Database,
    DatabaseIntegrity,
    DeviceAdministration,
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
    fn cli_accepts_only_the_three_offline_device_operations() {
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
        assert!(Cli::parse(["admin".into(), "device".into(), "list".into()]).is_err());
    }
}
