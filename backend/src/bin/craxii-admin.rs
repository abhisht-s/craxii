use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use craxii_server::adapters::artifacts::LocalArtifactStore;
use craxii_server::adapters::sqlite::{
    SqliteChannelIdentityStore, SqliteEvidenceQueryStore, SqliteRuntimeGuard, SqliteStateStore,
};
use craxii_server::adapters::system_clock::SystemClock;
use craxii_server::application::device_provisioning::DeviceProvisioningService;
use craxii_server::application::evidence_inspection::{
    EvidenceInspectionService, EvidenceOutputFormat,
};
use craxii_server::bootstrap::config;
use craxii_server::domain::{
    ChannelAccountId, DeliverySource, DeviceDisplayName, DeviceId, OutboundDeliveryId,
    OutboundDeliveryState, RuntimeInstanceId, UserId, UtcTimestamp, WorkId,
};
use craxii_server::ports::channel_identity::{ChannelIdentityStore, DisableChannelAccountOutcome};
use craxii_server::ports::clock::Clock;
use craxii_server::ports::delivery_store::{
    DeliveryStore, DeliverySummary, ListDeliverySummariesRequest,
};
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
    match &cli.action {
        Action::ValidateConfig => {
            writeln!(stdout, "configuration_valid").map_err(|_| AdminError::Output)?;
            stdout.flush().map_err(|_| AdminError::Output)?;
            return Ok(());
        }
        Action::GenerateChannelAccountId => {
            writeln!(stdout, "{}", ChannelAccountId::generate()).map_err(|_| AdminError::Output)?;
            stdout.flush().map_err(|_| AdminError::Output)?;
            return Ok(());
        }
        _ => {}
    }
    let guard = if cli.action.is_read_only() {
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
            let user_id = load_owner_user_id(&store).await?;
            let observed_at = observed_at()?;
            let service = DeviceProvisioningService::new(&store);
            let provisioned = service
                .provision(user_id, display_name, observed_at)
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
            load_owner_user_id(&store).await?;
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
            load_owner_user_id(&store).await?;
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
        Action::DisableChannelAccount(channel_account_id) => {
            let observed_at = observed_at()?;
            let store = SqliteChannelIdentityStore::new(guard.runtime().clone());
            let outcome = store
                .disable_channel_account(channel_account_id, observed_at)
                .await
                .map_err(|_| AdminError::ChannelAdministration)?;
            match outcome {
                DisableChannelAccountOutcome::Disabled(account) => writeln!(
                    stdout,
                    "disabled\t{}\t{}",
                    account.channel_account_id,
                    account.disabled_at.ok_or(AdminError::DatabaseIntegrity)?
                ),
                DisableChannelAccountOutcome::AlreadyDisabled(account) => writeln!(
                    stdout,
                    "already_disabled\t{}\t{}",
                    account.channel_account_id,
                    account.disabled_at.ok_or(AdminError::DatabaseIntegrity)?
                ),
                DisableChannelAccountOutcome::NotFound => {
                    writeln!(stdout, "not_found\t{channel_account_id}")
                }
            }
            .map_err(|_| AdminError::Output)?;
        }
        Action::InspectDeliveries(request) => {
            let summaries = store
                .list_delivery_summaries(request)
                .await
                .map_err(|_| AdminError::DeliveryInspection)?;
            write_delivery_summaries(stdout, &summaries)?;
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
        Action::ValidateConfig | Action::GenerateChannelAccountId => unreachable!(),
    }
    stdout.flush().map_err(|_| AdminError::Output)?;
    guard.shutdown().await;
    if verification_failed {
        Err(AdminError::VerificationFailed)
    } else {
        Ok(())
    }
}

async fn load_owner_user_id(store: &SqliteStateStore) -> Result<UserId, AdminError> {
    store
        .load_bootstrap_snapshot()
        .await
        .map(|snapshot| snapshot.identity.user_id)
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

fn write_delivery_summaries(
    output: &mut impl Write,
    summaries: &[DeliverySummary],
) -> Result<(), AdminError> {
    if summaries.len() > 100 {
        return Err(AdminError::DatabaseIntegrity);
    }
    writeln!(
        output,
        "delivery_id\tchannel_account_id\twork_id\tstate\tpart\tparts\tattempts\tnext_attempt_at\tdeadline_at\tfailure_class\tcreated_at\tupdated_at\tterminal_at"
    )
    .map_err(|_| AdminError::Output)?;
    for summary in summaries {
        let work_id = match summary.source {
            DeliverySource::AssistantMessage { work_id, .. } => work_id.to_string(),
            DeliverySource::ControlAcknowledgement { .. } => "-".to_owned(),
        };
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            summary.outbound_delivery_id,
            summary.channel_account_id,
            work_id,
            summary.state.as_str(),
            summary.part_ordinal,
            summary.part_count,
            summary.attempt_count,
            optional_timestamp(summary.next_attempt_at),
            summary.delivery_deadline_at,
            summary.failure_class.map_or("-", |value| value.as_str()),
            summary.created_at,
            summary.updated_at,
            optional_timestamp(summary.terminal_at),
        )
        .map_err(|_| AdminError::Output)?;
    }
    Ok(())
}

struct Cli {
    config_path: PathBuf,
    action: Action,
}

enum Action {
    ValidateConfig,
    GenerateChannelAccountId,
    Provision(DeviceDisplayName),
    List,
    Revoke(DeviceId),
    DisableChannelAccount(ChannelAccountId),
    InspectDeliveries(ListDeliverySummariesRequest),
    Preflight(EvidenceOutputFormat),
    VerifyState(EvidenceOutputFormat),
    InspectWork(WorkId, EvidenceOutputFormat),
    InspectRuntime(RuntimeInstanceId, EvidenceOutputFormat),
    EvidenceExport(EvidenceOutputFormat),
}

impl Action {
    const fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::InspectDeliveries(_)
                | Self::Preflight(_)
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
        let action = if group == OsStr::new("config") {
            if arguments.next().as_deref() != Some(OsStr::new("validate")) {
                return Err(AdminError::Cli);
            }
            Action::ValidateConfig
        } else if group == OsStr::new("channel-account-id") {
            if arguments.next().as_deref() != Some(OsStr::new("generate")) {
                return Err(AdminError::Cli);
            }
            Action::GenerateChannelAccountId
        } else if group == OsStr::new("device") {
            let command = arguments.next().ok_or(AdminError::Cli)?;
            parse_device_action(command, &mut arguments)?
        } else if group == OsStr::new("channel-account") {
            if arguments.next().as_deref() != Some(OsStr::new("disable")) {
                return Err(AdminError::Cli);
            }
            let id = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            Action::DisableChannelAccount(
                ChannelAccountId::parse_canonical(&id).map_err(|_| AdminError::Cli)?,
            )
        } else if group == OsStr::new("delivery") {
            if arguments.next().as_deref() != Some(OsStr::new("inspect")) {
                return Err(AdminError::Cli);
            }
            Action::InspectDeliveries(parse_delivery_inspection(&mut arguments)?)
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

fn parse_delivery_inspection(
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<ListDeliverySummariesRequest, AdminError> {
    let mut states = Vec::new();
    let mut after = None;
    let mut limit = 100_u16;
    while let Some(flag) = arguments.next() {
        if flag == OsStr::new("--state") {
            let value = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            let state = match value.as_str() {
                "queued" => OutboundDeliveryState::Queued,
                "retry_wait" => OutboundDeliveryState::RetryWait,
                "permanent_failure" => OutboundDeliveryState::PermanentFailure,
                "outcome_unknown" => OutboundDeliveryState::OutcomeUnknown,
                _ => return Err(AdminError::Cli),
            };
            if !states.contains(&state) {
                states.push(state);
            }
        } else if flag == OsStr::new("--after") && after.is_none() {
            let value = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            after = Some(OutboundDeliveryId::parse_canonical(&value).map_err(|_| AdminError::Cli)?);
        } else if flag == OsStr::new("--limit") {
            let value = arguments
                .next()
                .ok_or(AdminError::Cli)?
                .into_string()
                .map_err(|_| AdminError::Cli)?;
            limit = value.parse().map_err(|_| AdminError::Cli)?;
            if !(1..=100).contains(&limit) {
                return Err(AdminError::Cli);
            }
        } else {
            return Err(AdminError::Cli);
        }
    }
    if states.is_empty() {
        states.extend([
            OutboundDeliveryState::Queued,
            OutboundDeliveryState::RetryWait,
            OutboundDeliveryState::PermanentFailure,
            OutboundDeliveryState::OutcomeUnknown,
        ]);
    }
    Ok(ListDeliverySummariesRequest {
        states,
        after,
        limit,
    })
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
    ChannelAdministration,
    DeliveryInspection,
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
            Self::ChannelAdministration => "channel_administration_failure",
            Self::DeliveryInspection => "delivery_inspection_failure",
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use craxii_server::adapters::sqlite::SqliteRuntimeGuard;
    use craxii_server::domain::{
        ChannelProviderId, DeliveryFailureClass, InboundDeliveryId, MessageId, Sha256Digest,
    };

    #[test]
    fn cli_accepts_narrow_offline_operations_only() {
        let config = OsString::from("/tmp/config.toml");
        let validate = Cli::parse([
            "admin".into(),
            "--config".into(),
            config.clone(),
            "config".into(),
            "validate".into(),
        ])
        .unwrap();
        assert!(matches!(validate.action, Action::ValidateConfig));

        let generate = Cli::parse([
            "admin".into(),
            "--config".into(),
            config.clone(),
            "channel-account-id".into(),
            "generate".into(),
        ])
        .unwrap();
        assert!(matches!(generate.action, Action::GenerateChannelAccountId));

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

        let channel_account_id = ChannelAccountId::generate();
        let disable = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "channel-account".into(),
            "disable".into(),
            channel_account_id.to_string().into(),
        ])
        .unwrap();
        assert!(matches!(
            disable.action,
            Action::DisableChannelAccount(value) if value == channel_account_id
        ));

        let after = OutboundDeliveryId::generate();
        let deliveries = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "delivery".into(),
            "inspect".into(),
            "--state".into(),
            "retry_wait".into(),
            "--state".into(),
            "outcome_unknown".into(),
            "--after".into(),
            after.to_string().into(),
            "--limit".into(),
            "7".into(),
        ])
        .unwrap();
        assert!(matches!(
            deliveries.action,
            Action::InspectDeliveries(ListDeliverySummariesRequest {
                states,
                after: Some(value),
                limit: 7,
            }) if states == [OutboundDeliveryState::RetryWait, OutboundDeliveryState::OutcomeUnknown]
                && value == after
        ));

        let delivery_defaults = Cli::parse([
            "admin".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "delivery".into(),
            "inspect".into(),
        ])
        .unwrap();
        assert!(matches!(
            delivery_defaults.action,
            Action::InspectDeliveries(ListDeliverySummariesRequest { states, after: None, limit: 100 })
                if states == [
                    OutboundDeliveryState::Queued,
                    OutboundDeliveryState::RetryWait,
                    OutboundDeliveryState::PermanentFailure,
                    OutboundDeliveryState::OutcomeUnknown,
                ]
        ));

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
        for rejected in ["accepted", "dispatching", "provider_secret"] {
            assert!(
                Cli::parse([
                    "admin".into(),
                    "--config".into(),
                    "/tmp/config.toml".into(),
                    "delivery".into(),
                    "inspect".into(),
                    "--state".into(),
                    rejected.into(),
                ])
                .is_err()
            );
        }
        assert!(
            Cli::parse([
                "admin".into(),
                "--config".into(),
                "/tmp/config.toml".into(),
                "delivery".into(),
                "inspect".into(),
                "--limit".into(),
                "101".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn delivery_inspection_output_is_bounded_and_redacted() {
        let payload_canary = "SYNTHETIC_MESSAGE_PAYLOAD_CANARY";
        let external_id_canary = "SYNTHETIC_TELEGRAM_EXTERNAL_ID_CANARY";
        let provider_description_canary = "SYNTHETIC_RAW_PROVIDER_DESCRIPTION_CANARY";
        let model_context_canary = "SYNTHETIC_MODEL_CONTEXT_CANARY";
        let token_canary = "SYNTHETIC_BOT_TOKEN_CANARY";
        let message_id = MessageId::generate();
        let inbound_delivery_id = InboundDeliveryId::generate();
        let now: UtcTimestamp = "2026-09-18T01:02:03.000000Z".parse().unwrap();
        let later: UtcTimestamp = "2026-09-18T01:03:03.000000Z".parse().unwrap();
        let work_id = WorkId::generate();
        let mut summaries = Vec::new();
        for (index, state) in [
            OutboundDeliveryState::Queued,
            OutboundDeliveryState::RetryWait,
            OutboundDeliveryState::PermanentFailure,
            OutboundDeliveryState::OutcomeUnknown,
        ]
        .into_iter()
        .enumerate()
        {
            summaries.push(DeliverySummary {
                outbound_delivery_id: OutboundDeliveryId::generate(),
                source: if index == 0 {
                    DeliverySource::AssistantMessage {
                        message_id,
                        work_id,
                    }
                } else {
                    DeliverySource::ControlAcknowledgement {
                        inbound_delivery_id,
                        outcome: craxii_server::domain::ControlAcknowledgementOutcome::NoOp,
                    }
                },
                channel_account_id: ChannelAccountId::generate(),
                provider_id: ChannelProviderId::try_new("synthetic.provider").unwrap(),
                part_ordinal: 1,
                part_count: 1,
                state,
                attempt_count: u16::try_from(index).unwrap(),
                created_at: now,
                updated_at: now,
                next_attempt_at: matches!(
                    state,
                    OutboundDeliveryState::Queued | OutboundDeliveryState::RetryWait
                )
                .then_some(now),
                delivery_deadline_at: later,
                terminal_at: state.is_terminal().then_some(now),
                payload_sha256: Sha256Digest::hash_bytes(payload_canary.as_bytes()),
                failure_class: state.is_terminal().then_some(if index == 2 {
                    DeliveryFailureClass::ProviderPermanent
                } else {
                    DeliveryFailureClass::ProviderOutcomeUnknown
                }),
                failure_code: state
                    .is_terminal()
                    .then(|| provider_description_canary.to_owned()),
            });
        }

        let mut output = Vec::new();
        write_delivery_summaries(&mut output, &summaries).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 5);
        for state in [
            "queued",
            "retry_wait",
            "permanent_failure",
            "outcome_unknown",
        ] {
            assert!(output.contains(state));
        }
        assert!(output.contains(&work_id.to_string()));
        for forbidden in [
            payload_canary,
            external_id_canary,
            provider_description_canary,
            model_context_canary,
            token_canary,
            &message_id.to_string(),
            &inbound_delivery_id.to_string(),
        ] {
            assert!(!output.contains(forbidden));
        }
        assert!(!output.contains(&summaries[0].payload_sha256.to_string()));
        assert!(!output.contains(summaries[0].provider_id.as_str()));

        let mut over_limit = vec![summaries[0].clone(); 101];
        over_limit[100].outbound_delivery_id = OutboundDeliveryId::generate();
        let mut rejected_output = Vec::new();
        assert!(matches!(
            write_delivery_summaries(&mut rejected_output, &over_limit),
            Err(AdminError::DatabaseIntegrity)
        ));
        assert!(rejected_output.is_empty());
    }

    #[tokio::test]
    async fn offline_admin_fails_closed_while_runtime_lock_is_owned() {
        let root = std::env::temp_dir().join(format!(
            "craxii-admin-offline-lock-{}",
            uuid::Uuid::now_v7()
        ));
        let state_root = root.join("state");
        fs::create_dir_all(&state_root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700)).unwrap();
        let config_path = root.join("config.toml");
        let config = include_str!("../../../ops/stage27/config.toml.template")
            .replace(
                "state_root = \"/var/lib/craxii\"",
                &format!("state_root = {:?}", state_root),
            )
            .replace(
                "artifact_root = \"/var/lib/craxii/artifacts\"",
                &format!("artifact_root = {:?}", state_root.join("artifacts")),
            )
            .replace(
                "primary_workspace_root = \"/srv/craxii/workspaces/primary\"",
                &format!("primary_workspace_root = {:?}", root.join("workspace")),
            );
        fs::write(&config_path, config).unwrap();
        let guard = SqliteRuntimeGuard::start(&state_root, 1).await.unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(
            [
                OsString::from("admin"),
                OsString::from("--config"),
                config_path.into_os_string(),
                OsString::from("delivery"),
                OsString::from("inspect"),
            ],
            &mut stdout,
            &mut stderr,
        )
        .await;
        assert!(matches!(result, Err(AdminError::Database)));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(
            [
                OsString::from("admin"),
                OsString::from("--config"),
                root.join("config.toml").into_os_string(),
                OsString::from("channel-account"),
                OsString::from("disable"),
                ChannelAccountId::generate().to_string().into(),
            ],
            &mut stdout,
            &mut stderr,
        )
        .await;
        assert!(matches!(result, Err(AdminError::Database)));
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
        guard.shutdown().await;
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn config_validation_and_channel_account_generation_need_no_database_or_credential() {
        let root =
            std::env::temp_dir().join(format!("craxii-admin-config-only-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap();
        let config_path = root.join("config.toml");
        std::fs::write(
            &config_path,
            include_str!("../../../ops/stage27/config.toml.template"),
        )
        .unwrap();

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run(
            [
                OsString::from("admin"),
                OsString::from("--config"),
                config_path.clone().into_os_string(),
                OsString::from("config"),
                OsString::from("validate"),
            ],
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(stdout, b"configuration_valid\n");
        assert!(stderr.is_empty());

        stdout.clear();
        run(
            [
                OsString::from("admin"),
                OsString::from("--config"),
                config_path.clone().into_os_string(),
                OsString::from("channel-account-id"),
                OsString::from("generate"),
            ],
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        let generated = std::str::from_utf8(&stdout).unwrap().trim();
        assert_eq!(
            ChannelAccountId::parse_canonical(generated)
                .unwrap()
                .to_string(),
            generated
        );
        assert!(stderr.is_empty());
        std::fs::remove_file(config_path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
