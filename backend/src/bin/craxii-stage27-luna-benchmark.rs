use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use craxii_server::bootstrap::config::{
    self, DeviceAuthSource, ModelProvider, ShellEnvironmentPolicy,
};
use craxii_server::bootstrap::credential::CredentialSourceConfig;
use craxii_server::bootstrap::metadata::{BuildMetadata, ReleaseProvenancePolicy};
use nix::sys::termios::{LocalFlags, SetArg, Termios, tcgetattr, tcsetattr};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};
use uuid::Uuid;

const CONFIG_PATH: &str = "/etc/craxii/config.toml";
const CURRENT_RELEASE: &str = "/opt/craxii/current";
const SERVICE: &str = "craxii-server.service";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const UNAME: &str = "/usr/bin/uname";
const GIT: &str = "/usr/bin/git";
const TARGET: &str = "stage27-openai";
const PROVIDER: &str = "openai";
const MODEL: &str = "gpt-5.6-luna";
const WORKSPACE: &str = "/srv/craxii/workspaces/primary";
const STATE_ROOT: &str = "/var/lib/craxii";
const PUBLIC_URL: &str = "http://127.0.0.1:8080";
const PROMPT: &str = "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.";
const TERMINAL_STATES: &[&str] = &["completed", "failed", "cancelled", "interrupted"];

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let mut attempt = Attempt::default();
    match run(&mut attempt).await {
        Ok(report) => {
            report.print();
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("STAGE27_LUNA_CANONICAL_BENCHMARK=FAIL");
            eprintln!("STAGE27_LUNA_ERROR={}", error.code);
            if let Some(id) = attempt.client_message_id {
                eprintln!("STAGE27_CLIENT_MESSAGE_ID={id}");
            }
            if attempt.request_started {
                eprintln!("STAGE27_AUTOMATIC_RESUBMISSION=FORBIDDEN");
            }
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Attempt {
    client_message_id: Option<Uuid>,
    request_started: bool,
}

#[derive(Clone, Copy, Debug)]
struct RunnerError {
    code: &'static str,
}

impl RunnerError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

type Result<T> = std::result::Result<T, RunnerError>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServiceSnapshot {
    main_pid: u32,
    restart_count: u64,
}

struct Preflight {
    conversation_id: String,
    workspace_id: String,
    workstation_id: String,
    runtime_id: String,
    database: PathBuf,
    git_revision: String,
    service: ServiceSnapshot,
}

#[derive(Debug, Deserialize)]
struct Receipt {
    protocol_version: u64,
    message_id: String,
    work_id: String,
    work_state: String,
    conversation_work_ordinal: u64,
    committed_cursor: u64,
    duplicate: bool,
}

#[derive(Debug)]
struct HostFacts {
    os: String,
    architecture: String,
    cwd: String,
    git_version: String,
}

struct VerificationReport {
    main_pid: u32,
    git_revision: String,
    conversation_id: String,
    message_id: String,
    work_id: String,
    runtime_id: String,
    model_attempt_count: i64,
    tool_execution_count: i64,
    assistant_sha256: String,
    committed_cursor: u64,
    facts: HostFacts,
}

impl VerificationReport {
    fn print(&self) {
        println!("STAGE27_LUNA_CANONICAL_BENCHMARK=PASS");
        println!("STAGE27_MAIN_PID={}", self.main_pid);
        println!("STAGE27_GIT_REVISION={}", self.git_revision);
        println!("STAGE27_CONVERSATION_ID={}", self.conversation_id);
        println!("STAGE27_ACCEPTED_MESSAGE_ID={}", self.message_id);
        println!("STAGE27_WORK_ID={}", self.work_id);
        println!("STAGE27_RUNTIME_ID={}", self.runtime_id);
        println!("STAGE27_REAL_LUNA_PROVIDER={PROVIDER}");
        println!("STAGE27_REAL_LUNA_MODEL={MODEL}");
        println!("STAGE27_MODEL_ATTEMPT_COUNT={}", self.model_attempt_count);
        println!("STAGE27_TOOL_EXECUTION_COUNT={}", self.tool_execution_count);
        println!("STAGE27_COMMAND_COMMITTED_CURSOR={}", self.committed_cursor);
        println!("STAGE27_ASSISTANT_SHA256={}", self.assistant_sha256);
        println!("STAGE27_HOST_OS={}", self.facts.os);
        println!("STAGE27_HOST_ARCHITECTURE={}", self.facts.architecture);
        println!("STAGE27_HOST_CWD={}", self.facts.cwd);
        println!("STAGE27_HOST_GIT_VERSION={}", self.facts.git_version);
        println!("STAGE27_ASSISTANT_FACT_MATCH=YES");
        println!("STAGE27_TOOL_FACT_MATCH=YES");
        println!("STAGE27_AUTOMATIC_RESUBMISSION=NO");
    }
}

async fn run(attempt: &mut Attempt) -> Result<VerificationReport> {
    let preflight = preflight().await?;
    let facts = host_facts()?;
    health_ready().await?;
    let bearer = read_bearer()?;
    let client_message_id = Uuid::now_v7();
    attempt.client_message_id = Some(client_message_id);
    println!("STAGE27_CLIENT_MESSAGE_ID={client_message_id}");
    println!("STAGE27_SUBMISSION_ATTEMPTS=1");
    std::io::stdout()
        .flush()
        .map_err(|_| RunnerError::new("stdout_failure"))?;
    attempt.request_started = true;
    let submitted = submit(&preflight, &bearer, client_message_id).await;
    drop(bearer);
    let receipt = match submitted {
        Ok(receipt) => receipt,
        Err(error)
            if matches!(
                error.code,
                "submission_transport_ambiguous" | "submission_response_ambiguous"
            ) =>
        {
            recover_durable_receipt(&preflight.database, client_message_id)
                .await?
                .ok_or(error)?
        }
        Err(error) => return Err(error),
    };
    validate_receipt(&receipt)?;
    println!("STAGE27_SUBMISSION_ACCEPTED=YES");
    println!("STAGE27_WORK_ID={}", receipt.work_id);
    std::io::stdout()
        .flush()
        .map_err(|_| RunnerError::new("stdout_failure"))?;
    wait_for_terminal(&preflight.database, &receipt.work_id).await?;
    let service_after = service_snapshot()?;
    if service_after != preflight.service {
        return Err(RunnerError::new(
            "service_identity_changed_after_submission",
        ));
    }
    health_ready().await?;
    verify(&preflight, &receipt, client_message_id, facts).await
}

async fn preflight() -> Result<Preflight> {
    let configuration = config::load(CONFIG_PATH)
        .map_err(|_| RunnerError::new("production_configuration_invalid"))?;
    validate_configuration(&configuration)?;
    let build =
        BuildMetadata::embedded().map_err(|_| RunnerError::new("runner_build_metadata_invalid"))?;
    build
        .validate_release_provenance(ReleaseProvenancePolicy::without_required_timestamp())
        .map_err(|_| RunnerError::new("runner_release_provenance_invalid"))?;
    let release = fs::canonicalize(CURRENT_RELEASE)
        .map_err(|_| RunnerError::new("active_release_unavailable"))?;
    let runner = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|_| RunnerError::new("runner_path_unavailable"))?;
    if runner.parent() != Some(release.as_path())
        || runner.file_name().and_then(|value| value.to_str())
            != Some("craxii-stage27-luna-benchmark")
    {
        return Err(RunnerError::new("runner_is_outside_active_release"));
    }
    let service = service_snapshot()?;
    let server = fs::canonicalize(format!("/proc/{}/exe", service.main_pid))
        .map_err(|_| RunnerError::new("service_executable_unavailable"))?;
    if server != release.join("craxii-server") {
        return Err(RunnerError::new("service_is_outside_active_release"));
    }
    let database = configuration.paths().state_root().join("db/craxii.sqlite3");
    let mut connection = connect(&database).await?;
    let principal_rows = sqlx::query(
        "SELECT primary_conversation_id, default_workspace_id FROM craxii_principals \
         WHERE lifecycle_state = 'active'",
    )
    .fetch_all(&mut connection)
    .await
    .map_err(|_| RunnerError::new("durable_preflight_query_failed"))?;
    if principal_rows.len() != 1 {
        return Err(RunnerError::new("active_principal_cardinality_invalid"));
    }
    let conversation_id: String = principal_rows[0]
        .try_get("primary_conversation_id")
        .map_err(|_| RunnerError::new("principal_identity_invalid"))?;
    let workspace_id: String = principal_rows[0]
        .try_get("default_workspace_id")
        .map_err(|_| RunnerError::new("principal_identity_invalid"))?;
    let conversation = sqlx::query(
        "SELECT next_work_ordinal FROM conversations WHERE conversation_id = ? \
         AND kind = 'primary' AND lifecycle_state = 'active'",
    )
    .bind(&conversation_id)
    .fetch_optional(&mut connection)
    .await
    .map_err(|_| RunnerError::new("durable_preflight_query_failed"))?
    .ok_or_else(|| RunnerError::new("primary_conversation_invalid"))?;
    if conversation
        .try_get::<i64, _>("next_work_ordinal")
        .map_err(|_| RunnerError::new("primary_conversation_invalid"))?
        != 1
    {
        return Err(RunnerError::new("benchmark_conversation_not_pristine"));
    }
    let workspace = sqlx::query(
        "SELECT workstation_id, logical_root, local_resolved_root FROM workspaces \
         WHERE workspace_id = ? AND logical_name = 'primary' AND lifecycle_state = 'active'",
    )
    .bind(&workspace_id)
    .fetch_optional(&mut connection)
    .await
    .map_err(|_| RunnerError::new("durable_preflight_query_failed"))?
    .ok_or_else(|| RunnerError::new("primary_workspace_invalid"))?;
    let workstation_id: String = workspace
        .try_get("workstation_id")
        .map_err(|_| RunnerError::new("primary_workspace_invalid"))?;
    if workspace
        .try_get::<String, _>("logical_root")
        .map_err(|_| RunnerError::new("primary_workspace_invalid"))?
        != WORKSPACE
        || workspace
            .try_get::<String, _>("local_resolved_root")
            .map_err(|_| RunnerError::new("primary_workspace_invalid"))?
            != WORKSPACE
    {
        return Err(RunnerError::new("primary_workspace_path_invalid"));
    }
    let runtimes = sqlx::query(
        "SELECT runtime_instance_id, workstation_id, process_id, git_revision \
         FROM runtime_instances WHERE state = 'running'",
    )
    .fetch_all(&mut connection)
    .await
    .map_err(|_| RunnerError::new("durable_preflight_query_failed"))?;
    if runtimes.len() != 1 {
        return Err(RunnerError::new("running_runtime_cardinality_invalid"));
    }
    let runtime_id: String = runtimes[0]
        .try_get("runtime_instance_id")
        .map_err(|_| RunnerError::new("running_runtime_invalid"))?;
    if runtimes[0]
        .try_get::<String, _>("workstation_id")
        .map_err(|_| RunnerError::new("running_runtime_invalid"))?
        != workstation_id
        || runtimes[0]
            .try_get::<i64, _>("process_id")
            .map_err(|_| RunnerError::new("running_runtime_invalid"))?
            != i64::from(service.main_pid)
        || runtimes[0]
            .try_get::<String, _>("git_revision")
            .map_err(|_| RunnerError::new("running_runtime_invalid"))?
            != build.git_revision()
    {
        return Err(RunnerError::new("running_runtime_identity_mismatch"));
    }
    if scalar(
        &mut connection,
        "SELECT COUNT(*) FROM client_devices WHERE revoked_at IS NULL",
    )
    .await?
        != 1
    {
        return Err(RunnerError::new("active_device_cardinality_invalid"));
    }
    for query in [
        "SELECT COUNT(*) FROM messages",
        "SELECT COUNT(*) FROM work_items",
        "SELECT COUNT(*) FROM work_item_inputs",
        "SELECT COUNT(*) FROM client_commands",
        "SELECT COUNT(*) FROM model_invocations",
        "SELECT COUNT(*) FROM tool_executions",
    ] {
        if scalar(&mut connection, query).await? != 0 {
            return Err(RunnerError::new("benchmark_durable_state_not_pristine"));
        }
    }
    connection
        .close()
        .await
        .map_err(|_| RunnerError::new("database_close_failed"))?;
    Ok(Preflight {
        conversation_id,
        workspace_id,
        workstation_id,
        runtime_id,
        database,
        git_revision: build.git_revision().to_owned(),
        service,
    })
}

fn validate_configuration(configuration: &config::ValidatedConfig) -> Result<()> {
    if configuration.server().public_base_url().as_str() != PUBLIC_URL
        || configuration.server().bind_address().to_string() != "127.0.0.1:8080"
        || configuration.paths().state_root() != Path::new(STATE_ROOT)
        || configuration.paths().primary_workspace_root() != Path::new(WORKSPACE)
        || configuration.models().default_target() != TARGET
        || configuration.models().targets().len() != 1
        || !matches!(
            configuration.credentials().source(),
            CredentialSourceConfig::Systemd
        )
        || configuration.credentials().declared().len() != 1
        || configuration.credentials().declared()[0].as_str() != "openai_provider"
        || !matches!(
            configuration.device_auth().source(),
            DeviceAuthSource::ProvisionedSqlite
        )
        || configuration.shell().executable() != Path::new("/bin/bash")
        || !matches!(
            configuration.shell().environment_policy(),
            ShellEnvironmentPolicy::Clean
        )
        || !configuration.shell().inherited_variables().is_empty()
        || configuration.shell().administrative_enabled()
        || configuration.shell().user_switch_launcher()
            != Some(Path::new("/opt/craxii/current/craxii-workstation-launcher"))
    {
        return Err(RunnerError::new(
            "production_configuration_contract_mismatch",
        ));
    }
    let target = &configuration.models().targets()[0];
    if target.id() != TARGET
        || !target.enabled()
        || !matches!(target.provider(), ModelProvider::OpenAi)
        || target.provider_model_id() != MODEL
        || target.credential().as_str() != "openai_provider"
        || target.reasoning_continuation_required()
    {
        return Err(RunnerError::new("luna_target_contract_mismatch"));
    }
    Ok(())
}

fn service_snapshot() -> Result<ServiceSnapshot> {
    let active = systemctl(&["is-active", "--quiet", SERVICE])?;
    if !active.status.success() {
        return Err(RunnerError::new("service_not_active"));
    }
    let main_pid = systemctl_value("MainPID")?
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| RunnerError::new("service_main_pid_invalid"))?;
    let restart_count = systemctl_value("NRestarts")?
        .parse::<u64>()
        .map_err(|_| RunnerError::new("service_restart_count_invalid"))?;
    Ok(ServiceSnapshot {
        main_pid,
        restart_count,
    })
}

fn systemctl(arguments: &[&str]) -> Result<std::process::Output> {
    Command::new(SYSTEMCTL)
        .env_clear()
        .args(arguments)
        .output()
        .map_err(|_| RunnerError::new("systemctl_execution_failed"))
}

fn systemctl_value(property: &str) -> Result<String> {
    let output = systemctl(&["show", SERVICE, "--property", property, "--value"])?;
    if !output.status.success() {
        return Err(RunnerError::new("systemctl_query_failed"));
    }
    one_line(output.stdout, "systemctl_value_invalid")
}

async fn health_ready() -> Result<()> {
    let response = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| RunnerError::new("health_client_failed"))?
        .get(format!("{PUBLIC_URL}/health/ready"))
        .send()
        .await
        .map_err(|_| RunnerError::new("health_request_failed"))?;
    if response.status() != StatusCode::OK {
        return Err(RunnerError::new("service_not_ready"));
    }
    Ok(())
}

struct SecretBearer(Vec<u8>);

impl SecretBearer {
    fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated bearer is ASCII")
    }
}

impl Drop for SecretBearer {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

struct EchoGuard<'a> {
    terminal: &'a File,
    original: Termios,
}

impl Drop for EchoGuard<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.terminal, SetArg::TCSANOW, &self.original);
    }
}

fn read_bearer() -> Result<SecretBearer> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| RunnerError::new("interactive_terminal_required"))?;
    terminal
        .write_all(b"Retained 64-character device bearer: ")
        .and_then(|()| terminal.flush())
        .map_err(|_| RunnerError::new("terminal_write_failed"))?;
    let original =
        tcgetattr(&terminal).map_err(|_| RunnerError::new("terminal_settings_unavailable"))?;
    let mut hidden = original.clone();
    hidden.local_flags.remove(LocalFlags::ECHO);
    tcsetattr(&terminal, SetArg::TCSANOW, &hidden)
        .map_err(|_| RunnerError::new("terminal_echo_disable_failed"))?;
    let guard = EchoGuard {
        terminal: &terminal,
        original,
    };
    let mut bytes = Vec::with_capacity(66);
    let read = BufReader::new(&terminal).read_until(b'\n', &mut bytes);
    drop(guard);
    terminal
        .write_all(b"\n")
        .and_then(|()| terminal.flush())
        .map_err(|_| RunnerError::new("terminal_write_failed"))?;
    read.map_err(|_| RunnerError::new("bearer_read_failed"))?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        bytes.fill(0);
        return Err(RunnerError::new("device_bearer_format_invalid"));
    }
    Ok(SecretBearer(bytes))
}

async fn submit(
    preflight: &Preflight,
    bearer: &SecretBearer,
    client_message_id: Uuid,
) -> Result<Receipt> {
    let body = serde_json::to_vec(&json!({
        "protocol_version": 1,
        "client_message_id": client_message_id.to_string(),
        "content": [{"type": "text", "text": PROMPT}],
    }))
    .map_err(|_| RunnerError::new("request_encoding_failed"))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| RunnerError::new("submission_client_failed"))?;
    let response = client
        .post(format!(
            "{PUBLIC_URL}/v1/conversations/{}/messages",
            preflight.conversation_id
        ))
        .header("Idempotency-Key", client_message_id.to_string())
        .header("Content-Type", "application/json")
        .bearer_auth(bearer.expose())
        .body(body)
        .send()
        .await
        .map_err(|_| RunnerError::new("submission_transport_ambiguous"))?;
    if response.status() != StatusCode::ACCEPTED {
        return Err(RunnerError::new("submission_http_rejected"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| RunnerError::new("submission_response_ambiguous"))?;
    if bytes.len() > 65_536 {
        return Err(RunnerError::new("submission_response_ambiguous"));
    }
    serde_json::from_slice(&bytes).map_err(|_| RunnerError::new("submission_response_ambiguous"))
}

async fn recover_durable_receipt(
    database: &Path,
    client_message_id: Uuid,
) -> Result<Option<Receipt>> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut connection = connect(database).await?;
        let values: Vec<String> = sqlx::query_scalar(
            "SELECT response_json FROM client_commands \
             WHERE idempotency_key = ? AND command_type = 'message'",
        )
        .bind(client_message_id.to_string())
        .fetch_all(&mut connection)
        .await
        .map_err(|_| RunnerError::new("durable_receipt_query_failed"))?;
        connection
            .close()
            .await
            .map_err(|_| RunnerError::new("database_close_failed"))?;
        match values.as_slice() {
            [value] => {
                return serde_json::from_str(value)
                    .map(Some)
                    .map_err(|_| RunnerError::new("durable_receipt_invalid"));
            }
            [] if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            [] => return Ok(None),
            _ => return Err(RunnerError::new("durable_receipt_cardinality_invalid")),
        }
    }
}

fn validate_receipt(receipt: &Receipt) -> Result<()> {
    if receipt.protocol_version != 1
        || receipt.work_state != "queued"
        || receipt.conversation_work_ordinal != 1
        || receipt.committed_cursor == 0
        || receipt.duplicate
        || !is_v7(&receipt.message_id)
        || !is_v7(&receipt.work_id)
    {
        return Err(RunnerError::new("accepted_receipt_contract_mismatch"));
    }
    Ok(())
}

async fn wait_for_terminal(database: &Path, work_id: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1_800);
    loop {
        let mut connection = connect(database).await?;
        let row =
            sqlx::query("SELECT state, terminal_reason_code FROM work_items WHERE work_id = ?")
                .bind(work_id)
                .fetch_optional(&mut connection)
                .await
                .map_err(|_| RunnerError::new("work_state_query_failed"))?;
        connection
            .close()
            .await
            .map_err(|_| RunnerError::new("database_close_failed"))?;
        let row = row.ok_or_else(|| RunnerError::new("accepted_work_missing"))?;
        let state: String = row
            .try_get("state")
            .map_err(|_| RunnerError::new("work_state_invalid"))?;
        if TERMINAL_STATES.contains(&state.as_str()) {
            let reason: Option<String> = row
                .try_get("terminal_reason_code")
                .map_err(|_| RunnerError::new("work_state_invalid"))?;
            return if state == "completed" && reason.as_deref() == Some("answered") {
                Ok(())
            } else {
                Err(RunnerError::new("work_terminal_outcome_failed"))
            };
        }
        if Instant::now() >= deadline {
            return Err(RunnerError::new("work_terminal_timeout"));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn verify(
    preflight: &Preflight,
    receipt: &Receipt,
    client_message_id: Uuid,
    facts: HostFacts,
) -> Result<VerificationReport> {
    let mut connection = connect(&preflight.database).await?;
    for (query, expected) in [
        ("SELECT COUNT(*) FROM work_items", 1),
        ("SELECT COUNT(*) FROM messages", 2),
        ("SELECT COUNT(*) FROM client_commands", 1),
        ("SELECT COUNT(*) FROM work_item_inputs", 1),
    ] {
        if scalar(&mut connection, query).await? != expected {
            return Err(RunnerError::new("benchmark_durable_cardinality_invalid"));
        }
    }
    verify_work(&mut connection, preflight, receipt).await?;
    let assistant = verify_messages(&mut connection, preflight, receipt, client_message_id).await?;
    verify_command(&mut connection, receipt, client_message_id).await?;
    let (model_attempt_count, final_step, final_context, final_text) =
        verify_models(&mut connection, preflight, &receipt.work_id).await?;
    if final_text != assistant {
        return Err(RunnerError::new("assistant_output_provenance_mismatch"));
    }
    let tool_execution_count = verify_tools(
        &mut connection,
        preflight,
        &receipt.work_id,
        final_step,
        &final_context,
        &facts,
    )
    .await?;
    verify_journal(&mut connection, &receipt.work_id).await?;
    if !answer_contains_facts(&assistant, &facts) {
        return Err(RunnerError::new("assistant_fact_mismatch"));
    }
    let runtime_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_instances WHERE runtime_instance_id = ? \
         AND state = 'running' AND process_id = ? AND git_revision = ?",
    )
    .bind(&preflight.runtime_id)
    .bind(i64::from(preflight.service.main_pid))
    .bind(&preflight.git_revision)
    .fetch_one(&mut connection)
    .await
    .map_err(|_| RunnerError::new("runtime_evidence_query_failed"))?;
    if runtime_count != 1 {
        return Err(RunnerError::new("runtime_evidence_mismatch"));
    }
    connection
        .close()
        .await
        .map_err(|_| RunnerError::new("database_close_failed"))?;
    let assistant_sha256 = hex_digest(assistant.as_bytes());
    Ok(VerificationReport {
        main_pid: preflight.service.main_pid,
        git_revision: preflight.git_revision.clone(),
        conversation_id: preflight.conversation_id.clone(),
        message_id: receipt.message_id.clone(),
        work_id: receipt.work_id.clone(),
        runtime_id: preflight.runtime_id.clone(),
        model_attempt_count,
        tool_execution_count,
        assistant_sha256,
        committed_cursor: receipt.committed_cursor,
        facts,
    })
}

async fn verify_work(
    connection: &mut SqliteConnection,
    preflight: &Preflight,
    receipt: &Receipt,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT conversation_id, conversation_work_ordinal, kind, workspace_id, state, \
         runtime_instance_id, started_at, terminal_at, terminal_reason_code \
         FROM work_items WHERE work_id = ?",
    )
    .bind(&receipt.work_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("work_evidence_query_failed"))?
    .ok_or_else(|| RunnerError::new("work_evidence_missing"))?;
    if row.get::<String, _>("conversation_id") != preflight.conversation_id
        || row.get::<i64, _>("conversation_work_ordinal") != 1
        || row.get::<String, _>("kind") != "conversational"
        || row.get::<String, _>("workspace_id") != preflight.workspace_id
        || row.get::<String, _>("state") != "completed"
        || row
            .get::<Option<String>, _>("runtime_instance_id")
            .as_deref()
            != Some(preflight.runtime_id.as_str())
        || row.get::<Option<String>, _>("started_at").is_none()
        || row.get::<Option<String>, _>("terminal_at").is_none()
        || row
            .get::<Option<String>, _>("terminal_reason_code")
            .as_deref()
            != Some("answered")
    {
        return Err(RunnerError::new("work_evidence_mismatch"));
    }
    Ok(())
}

async fn verify_messages(
    connection: &mut SqliteConnection,
    preflight: &Preflight,
    receipt: &Receipt,
    client_message_id: Uuid,
) -> Result<String> {
    let user = sqlx::query(
        "SELECT message_id, conversation_id, content_json, client_message_id, client_device_id \
         FROM messages WHERE role = 'user'",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("user_message_evidence_invalid"))?;
    let user_content: String = user.get("content_json");
    if user.get::<String, _>("message_id") != receipt.message_id
        || user.get::<String, _>("conversation_id") != preflight.conversation_id
        || user.get::<String, _>("client_message_id") != client_message_id.to_string()
        || user.get::<Option<String>, _>("client_device_id").is_none()
        || content_text(&user_content).as_deref() != Some(PROMPT)
    {
        return Err(RunnerError::new("canonical_user_message_mismatch"));
    }
    let assistants = sqlx::query_scalar::<_, String>(
        "SELECT content_json FROM messages WHERE role = 'assistant' AND produced_by_work_id = ?",
    )
    .bind(&receipt.work_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("assistant_message_query_failed"))?;
    match assistants.as_slice() {
        [content] => content_text(content)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| RunnerError::new("assistant_message_invalid")),
        _ => Err(RunnerError::new("assistant_message_cardinality_invalid")),
    }
}

async fn verify_command(
    connection: &mut SqliteConnection,
    receipt: &Receipt,
    client_message_id: Uuid,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT command_type, response_http_status, response_json, committed_cursor \
         FROM client_commands WHERE idempotency_key = ?",
    )
    .bind(client_message_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("client_command_evidence_invalid"))?;
    let stored: Receipt = serde_json::from_str(&row.get::<String, _>("response_json"))
        .map_err(|_| RunnerError::new("client_command_response_invalid"))?;
    if row.get::<String, _>("command_type") != "message"
        || row.get::<i64, _>("response_http_status") != 202
        || row.get::<i64, _>("committed_cursor") != receipt.committed_cursor as i64
        || stored.message_id != receipt.message_id
        || stored.work_id != receipt.work_id
        || stored.duplicate
    {
        return Err(RunnerError::new("client_command_evidence_mismatch"));
    }
    Ok(())
}

async fn verify_models(
    connection: &mut SqliteConnection,
    preflight: &Preflight,
    work_id: &str,
) -> Result<(i64, i64, String, String)> {
    let rows = sqlx::query(
        "SELECT model_invocation_id, context_manifest_id, agent_step_no, attempt_no, \
         model_target_id, provider_id, provider_model_id, target_configuration_version, \
         selection_reason, state, normalized_output_json, provider_request_id, \
         provider_response_id, completed_at, tool_call_count, provider_outcome_certainty, \
         billing_ambiguity, runtime_instance_id FROM model_invocations WHERE work_id = ? \
         ORDER BY agent_step_no, attempt_no",
    )
    .bind(work_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("model_evidence_query_failed"))?;
    if rows.len() < 2 {
        return Err(RunnerError::new("luna_continuation_missing"));
    }
    let mut final_candidate = None;
    for row in &rows {
        let state: String = row.get("state");
        if row.get::<String, _>("model_target_id") != TARGET
            || row.get::<String, _>("provider_id") != PROVIDER
            || row.get::<String, _>("provider_model_id") != MODEL
            || row.get::<i64, _>("target_configuration_version") != 1
            || row.get::<String, _>("selection_reason") != "configured_default"
            || row.get::<String, _>("runtime_instance_id") != preflight.runtime_id
            || row.get::<Option<String>, _>("completed_at").is_none()
            || row.get::<i64, _>("billing_ambiguity") != 0
            || !matches!(state.as_str(), "completed" | "failed")
        {
            return Err(RunnerError::new("luna_attempt_evidence_unsafe"));
        }
        if state == "completed"
            && (row
                .get::<Option<String>, _>("provider_request_id")
                .is_none()
                || row
                    .get::<Option<String>, _>("provider_response_id")
                    .is_none()
                || row
                    .get::<Option<String>, _>("provider_outcome_certainty")
                    .as_deref()
                    != Some("definitely_completed"))
        {
            return Err(RunnerError::new(
                "completed_luna_attempt_evidence_incomplete",
            ));
        }
        if state == "failed"
            && !matches!(
                row.get::<Option<String>, _>("provider_outcome_certainty")
                    .as_deref(),
                Some(
                    "definitely_not_sent"
                        | "definite_provider_failure"
                        | "semantic_output_observed"
                )
            )
        {
            return Err(RunnerError::new("failed_luna_attempt_evidence_ambiguous"));
        }
        if state == "completed" && row.get::<Option<i64>, _>("tool_call_count") == Some(0) {
            final_candidate = Some(row);
        }
    }
    let final_row = final_candidate.ok_or_else(|| RunnerError::new("final_luna_answer_missing"))?;
    let normalized: String = final_row
        .get::<Option<String>, _>("normalized_output_json")
        .ok_or_else(|| RunnerError::new("final_luna_output_missing"))?;
    let final_text = normalized_output_text(&normalized)
        .ok_or_else(|| RunnerError::new("final_luna_output_invalid"))?;
    Ok((
        rows.len() as i64,
        final_row.get("agent_step_no"),
        final_row.get("context_manifest_id"),
        final_text,
    ))
}

async fn verify_tools(
    connection: &mut SqliteConnection,
    preflight: &Preflight,
    work_id: &str,
    final_step: i64,
    final_context: &str,
    facts: &HostFacts,
) -> Result<i64> {
    let rows = sqlx::query(
        "SELECT tool_execution_id, source_model_invocation_id, agent_step_no, \
         provider_tool_call_id, tool_name, arguments_json, runtime_instance_id, workstation_id, \
         workspace_id, resolved_cwd, requested_privilege, effective_privilege, state, \
         exit_code, timed_out, cancelled, cleanup_confirmed, result_json, stdout_artifact_id \
         FROM tool_executions WHERE work_id = ? ORDER BY agent_step_no, tool_ordinal",
    )
    .bind(work_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| RunnerError::new("tool_evidence_query_failed"))?;
    if rows.is_empty() {
        return Err(RunnerError::new("real_tool_execution_missing"));
    }
    let mut successful_shells = 0_i64;
    let mut tool_output = String::new();
    for row in &rows {
        let tool_id: String = row.get("tool_execution_id");
        let tool_name: String = row.get("tool_name");
        let result_json: String = row
            .get::<Option<String>, _>("result_json")
            .ok_or_else(|| RunnerError::new("tool_result_missing"))?;
        let result: Value = serde_json::from_str(&result_json)
            .map_err(|_| RunnerError::new("tool_result_invalid"))?;
        if row.get::<String, _>("state") != "completed"
            || row
                .get::<Option<String>, _>("provider_tool_call_id")
                .is_none()
            || row.get::<String, _>("runtime_instance_id") != preflight.runtime_id
            || row.get::<String, _>("workstation_id") != preflight.workstation_id
            || row.get::<String, _>("workspace_id") != preflight.workspace_id
            || row.get::<i64, _>("agent_step_no") >= final_step
        {
            return Err(RunnerError::new("tool_execution_evidence_unsafe"));
        }
        let source_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM model_invocations WHERE model_invocation_id = ? \
             AND work_id = ? AND state = 'completed' AND tool_call_count > 0",
        )
        .bind(row.get::<String, _>("source_model_invocation_id"))
        .bind(work_id)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| RunnerError::new("tool_source_query_failed"))?;
        if source_count != 1 {
            return Err(RunnerError::new("tool_source_model_invalid"));
        }
        let context_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_manifest_sources WHERE context_manifest_id = ? \
             AND source_kind = 'observed_tool_result' AND source_record_kind = 'tool_execution' \
             AND source_record_id = ?",
        )
        .bind(final_context)
        .bind(&tool_id)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| RunnerError::new("tool_context_query_failed"))?;
        if context_count != 1 {
            return Err(RunnerError::new("tool_result_continuation_missing"));
        }
        append_strings(&result, &mut tool_output);
        if tool_name == "run_shell" {
            let stdout_artifact: Option<String> = row.get("stdout_artifact_id");
            if row.get::<String, _>("requested_privilege") != "user"
                || row
                    .get::<Option<String>, _>("effective_privilege")
                    .as_deref()
                    != Some("user")
                || row.get::<Option<i64>, _>("timed_out") != Some(0)
                || row.get::<Option<i64>, _>("cancelled") != Some(0)
                || row.get::<Option<i64>, _>("cleanup_confirmed") != Some(1)
                || row.get::<Option<String>, _>("resolved_cwd").as_deref() != Some(WORKSPACE)
            {
                return Err(RunnerError::new("real_shell_evidence_unsafe"));
            }
            if result.get("result_kind").and_then(Value::as_str) != Some("success")
                || row.get::<Option<i64>, _>("exit_code") != Some(0)
            {
                continue;
            }
            let stdout_artifact = stdout_artifact
                .ok_or_else(|| RunnerError::new("successful_shell_artifact_missing"))?;
            let artifact_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? AND producing_work_id = ? \
                 AND producer_kind = 'tool_execution' AND producer_id = ?",
            )
            .bind(&stdout_artifact)
            .bind(work_id)
            .bind(&tool_id)
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| RunnerError::new("shell_artifact_query_failed"))?;
            if artifact_count != 1 {
                return Err(RunnerError::new("shell_artifact_provenance_invalid"));
            }
            successful_shells += 1;
        }
    }
    if successful_shells == 0 {
        return Err(RunnerError::new("successful_real_shell_missing"));
    }
    if !tool_output_contains_facts(&tool_output, facts) {
        return Err(RunnerError::new("tool_fact_mismatch"));
    }
    Ok(rows.len() as i64)
}

async fn verify_journal(connection: &mut SqliteConnection, work_id: &str) -> Result<()> {
    for event in [
        "work.queued",
        "work.started",
        "model.invocation_started",
        "model.invocation_completed",
        "tool.execution_requested",
        "tool.execution_dispatching",
        "tool.execution_completed",
        "assistant.message_committed",
        "work.completed",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type = ?",
        )
        .bind(work_id)
        .bind(event)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| RunnerError::new("journal_evidence_query_failed"))?;
        if count == 0 {
            return Err(RunnerError::new("required_journal_evidence_missing"));
        }
    }
    Ok(())
}

fn host_facts() -> Result<HostFacts> {
    let os_release = fs::read_to_string("/etc/os-release")
        .map_err(|_| RunnerError::new("os_release_unavailable"))?;
    let id = os_release_value(&os_release, "ID")
        .ok_or_else(|| RunnerError::new("os_release_invalid"))?;
    let version = os_release_value(&os_release, "VERSION_ID")
        .ok_or_else(|| RunnerError::new("os_release_invalid"))?;
    if id != "ubuntu" || version != "24.04" {
        return Err(RunnerError::new("host_os_contract_mismatch"));
    }
    let architecture = safe_command(UNAME, &["-m"], Path::new(WORKSPACE))?;
    let kernel = safe_command(UNAME, &["-s"], Path::new(WORKSPACE))?;
    if kernel != "Linux" {
        return Err(RunnerError::new("host_kernel_contract_mismatch"));
    }
    let git_version = safe_command(GIT, &["--version"], Path::new(WORKSPACE))?;
    if !git_version.starts_with("git version ") {
        return Err(RunnerError::new("host_git_version_invalid"));
    }
    let cwd = fs::canonicalize(WORKSPACE)
        .map_err(|_| RunnerError::new("host_workspace_unavailable"))?
        .to_str()
        .filter(|value| *value == WORKSPACE)
        .ok_or_else(|| RunnerError::new("host_workspace_identity_mismatch"))?
        .to_owned();
    Ok(HostFacts {
        os: format!("Ubuntu {version}"),
        architecture,
        cwd,
        git_version,
    })
}

fn safe_command(program: &str, arguments: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new(program)
        .env_clear()
        .current_dir(cwd)
        .args(arguments)
        .output()
        .map_err(|_| RunnerError::new("host_fact_command_failed"))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(RunnerError::new("host_fact_command_failed"));
    }
    one_line(output.stdout, "host_fact_command_output_invalid")
}

fn one_line(bytes: Vec<u8>, error: &'static str) -> Result<String> {
    let value = String::from_utf8(bytes).map_err(|_| RunnerError::new(error))?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n']) || value.chars().any(char::is_control) {
        return Err(RunnerError::new(error));
    }
    Ok(value.to_owned())
}

fn os_release_value(input: &str, key: &str) -> Option<String> {
    let raw = input
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))?;
    Some(raw.trim_matches('"').to_owned())
}

fn content_text(encoded: &str) -> Option<String> {
    let value: Value = serde_json::from_str(encoded).ok()?;
    let blocks = match &value {
        Value::Object(object) => object.get("blocks")?.as_array()?,
        Value::Array(blocks) => blocks,
        _ => return None,
    };
    Some(
        blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn normalized_output_text(encoded: &str) -> Option<String> {
    let value: Value = serde_json::from_str(encoded).ok()?;
    Some(
        value
            .get("items")?
            .as_array()?
            .iter()
            .filter(|item| item.get("kind").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
    )
}

fn append_strings(value: &Value, output: &mut String) {
    match value {
        Value::String(value) => {
            output.push_str(value);
            output.push('\n');
        }
        Value::Array(values) => {
            for value in values {
                append_strings(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                append_strings(value, output);
            }
        }
        _ => {}
    }
}

fn answer_contains_facts(answer: &str, facts: &HostFacts) -> bool {
    let lower = answer.to_ascii_lowercase();
    lower.contains("ubuntu")
        && lower.contains("24.04")
        && architecture_matches(&lower, &facts.architecture)
        && lower.contains(&facts.cwd.to_ascii_lowercase())
        && semantic_contains(answer, facts.git_version.trim_start_matches("git version "))
}

fn tool_output_contains_facts(output: &str, facts: &HostFacts) -> bool {
    let lower = output.to_ascii_lowercase();
    (lower.contains("linux") || (lower.contains("ubuntu") && lower.contains("24.04")))
        && architecture_matches(&lower, &facts.architecture)
        && lower.contains(&facts.cwd.to_ascii_lowercase())
        && semantic_contains(output, facts.git_version.trim_start_matches("git version "))
}

fn architecture_matches(lower: &str, architecture: &str) -> bool {
    let normalized = lower.replace('-', "_");
    normalized.contains(&architecture.to_ascii_lowercase())
        || (architecture == "x86_64" && lower.contains("amd64"))
}

fn semantic_contains(haystack: &str, needle: &str) -> bool {
    let haystack = tokens(haystack);
    let needle = tokens(needle);
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle.as_slice())
}

fn tokens(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_v7(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.get_version_num() == 7)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn connect(database: &Path) -> Result<SqliteConnection> {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(database)
            .read_only(true)
            .busy_timeout(Duration::from_secs(5)),
    )
    .await
    .map_err(|_| RunnerError::new("database_read_only_connection_failed"))
}

async fn scalar(connection: &mut SqliteConnection, query: &'static str) -> Result<i64> {
    sqlx::query_scalar::<_, i64>(query)
        .fetch_one(connection)
        .await
        .map_err(|_| RunnerError::new("durable_preflight_query_failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> HostFacts {
        HostFacts {
            os: "Ubuntu 24.04".to_owned(),
            architecture: "x86_64".to_owned(),
            cwd: WORKSPACE.to_owned(),
            git_version: "git version 2.43.0".to_owned(),
        }
    }

    #[test]
    fn exact_canonical_prompt_is_stable() {
        assert_eq!(
            PROMPT,
            "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have."
        );
    }

    #[test]
    fn fact_matching_requires_every_independently_observed_value() {
        let answer = "Ubuntu 24.04; AMD64; /srv/craxii/workspaces/primary; Git version 2.43.0.";
        assert!(answer_contains_facts(answer, &facts()));
        assert!(tool_output_contains_facts(
            "Linux\nx86_64\n/srv/craxii/workspaces/primary\ngit version 2.43.0\n",
            &facts(),
        ));
        assert!(!answer_contains_facts(
            "Ubuntu 24.04; x86_64; /srv/craxii/workspaces/primary",
            &facts(),
        ));
    }

    #[test]
    fn durable_content_and_final_output_decode_without_accepting_other_shapes() {
        assert_eq!(
            content_text(r#"{"version":1,"blocks":[{"type":"text","text":"safe"}]}"#).as_deref(),
            Some("safe"),
        );
        assert_eq!(
            normalized_output_text(
                r#"{"items":[{"kind":"text","text":"one"},{"kind":"text","text":" two"}]}"#
            )
            .as_deref(),
            Some("one two"),
        );
        assert!(content_text("null").is_none());
    }

    #[test]
    fn os_release_parser_requires_exact_keys() {
        let input = "ID=ubuntu\nVERSION_ID=\"24.04\"\n";
        assert_eq!(os_release_value(input, "ID").as_deref(), Some("ubuntu"));
        assert_eq!(
            os_release_value(input, "VERSION_ID").as_deref(),
            Some("24.04")
        );
        assert!(os_release_value(input, "VERSION").is_none());
    }
}
