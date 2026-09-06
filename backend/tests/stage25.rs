//! Stage 25 real-provider headless acceptance.
//!
//! The spend-bearing test is ignored and can only be entered through the explicit Stage 25
//! wrapper. Normal `cargo test` runs only the non-live configuration, redaction, and failure-view
//! checks in this file.

#![cfg(unix)]

#[path = "support/headless_client.rs"]
mod headless_client;
#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use craxii_server::bootstrap::config;
use craxii_server::bootstrap::credential::load_credentials;
use craxii_server::domain::UtcTimestamp;
use craxii_server::ports::model_provider::ProviderErrorKind;
use serde_json::{Value, json};
use sqlx::{Connection as _, Row as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use headless_client::{
    HeadlessClient, assert_durable_cursor_contract, next_json_with_timeout,
    signal_owned_process_group, through_sync,
};
use stage18_harness::MachineFacts;

const CANONICAL_PROMPT: &str = "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.";
const FOLLOW_UP: &str = "What Git version did you find?";
const PROVIDER: &str = "openai";
const MODEL_TARGET: &str = "stage25_openai";
const MODEL: &str = "gpt-4.1-2025-04-14";
const ENDPOINT: &str = "https://api.openai.com/v1";
const CONTEXT_WINDOW: u64 = 1_047_576;
const MAX_OUTPUT: u64 = 32_768;
const REQUESTED_OUTPUT: u64 = 1_024;
const CREDENTIAL_ID: &str = "openai_stage25";
const CREDENTIAL_DIRECTORY: &str = "/Users/abhisht/.config/craxii/credentials";
const CREDENTIAL_PATH: &str = "/Users/abhisht/.config/craxii/credentials/openai_stage25";
const TEMPLATE: &str = include_str!("fixtures/config/stage25-openai-headless.toml.template");
const LIVE_WAIT: Duration = Duration::from_secs(6 * 60);
const STARTUP_WAIT: Duration = Duration::from_secs(30);
const EXIT_WAIT: Duration = Duration::from_secs(20);
const RAW_KEY_ENVIRONMENTS: [&str; 4] = [
    "OPENAI_API_KEY",
    "OPENAI_KEY",
    "CRAXII_OPENAI_API_KEY",
    "CRAXII_STAGE25_OPENAI_API_KEY",
];
const FORBIDDEN_EVIDENCE_MARKERS: [&[u8]; 5] = [
    b"Authorization: Bearer ",
    b"OPENAI_API_KEY=",
    b"OPENAI_KEY=",
    b"CRAXII_OPENAI_API_KEY=",
    b"CRAXII_STAGE25_OPENAI_API_KEY=",
];

#[test]
fn stage25_configuration_freezes_the_audited_nonreasoning_target() {
    let rendered = render_config(
        "127.0.0.1:38025",
        Path::new("/tmp/craxii-stage25-config-test/state"),
        Path::new("/tmp/craxii-stage25-config-test/artifacts"),
        Path::new("/tmp/craxii-stage25-config-test/workspace"),
    );
    let parsed = config::parse(&rendered).expect("Stage 25 template must validate");
    assert_eq!(parsed.configuration_version(), 1);
    assert_eq!(parsed.models().default_target(), MODEL_TARGET);
    assert_eq!(parsed.models().targets().len(), 1);
    let target = &parsed.models().targets()[0];
    assert!(target.enabled());
    assert_eq!(target.id(), MODEL_TARGET);
    assert_eq!(target.provider_model_id(), MODEL);
    assert_eq!(target.endpoint().as_str(), ENDPOINT);
    assert_eq!(target.credential().as_str(), CREDENTIAL_ID);
    assert_eq!(target.context_window_tokens(), CONTEXT_WINDOW);
    assert_eq!(target.max_output_tokens(), MAX_OUTPUT);
    assert_eq!(target.requested_output_tokens(), REQUESTED_OUTPUT);
    assert!(!target.reasoning_continuation_required());
    assert!(!target.capabilities().reasoning_continuation());
    assert_eq!(
        parsed.credentials().source().local_directory(),
        Some(Path::new(CREDENTIAL_DIRECTORY))
    );
}

#[test]
fn stage25_wrapper_is_opt_in_and_has_no_raw_key_interface() {
    let source = include_str!("../../scripts/verify-stage25-openai-headless");
    for required in [
        "CRAXII_STAGE25_LIVE",
        CREDENTIAL_DIRECTORY,
        CREDENTIAL_PATH,
        "--ignored",
        "--exact",
        "live_openai_headless_canonical_restart_follow_up",
    ] {
        assert!(source.contains(required), "wrapper omitted {required}");
    }
    assert!(!source.contains("printf '%s\\n' \"${OPENAI_API_KEY}"));
    assert!(!source.contains("--api-key"));

    let output = Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/verify-stage25-openai-headless"
    ))
    .env_remove("CRAXII_STAGE25_LIVE")
    .output()
    .expect("execute Stage 25 wrapper without opt-in");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("LIVE_OPT_IN_REQUIRED"));
}

#[test]
fn credential_rules_and_secret_scan_fail_closed_without_disclosing_a_canary() {
    let root = TemporaryRoot::new("craxii-stage25-nonlive-");
    let credentials = root.path().join("credentials");
    fs::create_dir(&credentials).unwrap();
    fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700)).unwrap();
    let credential = credentials.join(CREDENTIAL_ID);
    let canary = b"sk-stage25-synthetic-secret-canary";
    write_new(&credential, canary, 0o600);

    let rendered = render_config(
        "127.0.0.1:38025",
        &root.path().join("state"),
        &root.path().join("artifacts"),
        &root.path().join("workspace"),
    )
    .replace(CREDENTIAL_DIRECTORY, credentials.to_str().unwrap());
    let parsed = config::parse(&rendered).unwrap();
    let loaded = load_credentials(
        parsed.credentials().source(),
        parsed
            .models()
            .targets()
            .iter()
            .filter(|target| target.enabled())
            .map(|target| target.credential()),
    )
    .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        format!("{loaded:?}"),
        format!("{{\"{CREDENTIAL_ID}\": [REDACTED]}}")
    );

    let evidence = root.path().join("evidence");
    fs::create_dir(&evidence).unwrap();
    write_new(
        &evidence.join("safe.json"),
        br#"{"secret":"[REDACTED]"}"#,
        0o600,
    );
    let patterns = vec![canary.to_vec()];
    assert_eq!(
        scan_paths(std::slice::from_ref(&evidence), &patterns),
        Ok(1)
    );
    write_new(&evidence.join("unsafe.bin"), canary, 0o600);
    let error = scan_paths(&[evidence], &patterns).unwrap_err();
    let rendered_error = format!("{error:?}");
    assert_eq!(rendered_error, "SecretDetected");
    assert!(!rendered_error.contains(std::str::from_utf8(canary).unwrap()));

    fs::set_permissions(&credential, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        load_credentials(
            parsed.credentials().source(),
            parsed
                .models()
                .targets()
                .iter()
                .filter(|target| target.enabled())
                .map(|target| target.credential()),
        )
        .is_err()
    );
}

#[test]
fn provider_failures_have_redacted_nonlive_acceptance_views() {
    let cases = [
        (ProviderErrorKind::Authentication, false),
        (ProviderErrorKind::RateLimited, true),
        (ProviderErrorKind::TemporarilyUnavailable, true),
        (ProviderErrorKind::TransportBeforeResponse, true),
        (ProviderErrorKind::TimeoutBeforeOutput, true),
        (ProviderErrorKind::MalformedResponse, false),
        (ProviderErrorKind::Cancelled, false),
    ];
    for (kind, retryable) in cases {
        let view = provider_failure_view(kind);
        assert_eq!(view["code"], kind.code());
        assert_eq!(view["retryable_before_output"], retryable);
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(!encoded.contains("Authorization"));
        assert!(!encoded.contains("Bearer"));
        assert!(!encoded.contains("sk-"));
        let report = redacted_failure_report("non_live", "work.failed", Some(kind.code()), true);
        assert_eq!(report["failure"]["code"], kind.code());
        assert_eq!(report["status"], "failed");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit spend-bearing Stage 25 acceptance; requires the Stage 25 wrapper"]
async fn live_openai_headless_canonical_restart_follow_up() {
    assert_eq!(std::env::var("CRAXII_STAGE25_LIVE").as_deref(), Ok("1"));
    for name in RAW_KEY_ENVIRONMENTS {
        assert!(
            std::env::var_os(name).is_none(),
            "raw provider-key environment variables are forbidden"
        );
    }

    let report_path = required_report_path();
    let credential_patterns = live_credential_preflight();
    let mut scenario = LiveScenario::new();
    scenario.prepare();
    let authority = available_authority();
    let config_path = scenario.root.join("stage25.toml");
    let rendered = render_config(
        &authority,
        &scenario.root.join("state"),
        &scenario.root.join("artifacts"),
        &scenario.root.join("workspace"),
    );
    write_new(&config_path, rendered.as_bytes(), 0o600);
    let validated = config::load(&config_path).expect("load rendered Stage 25 configuration");
    assert_stage25_runtime_config(&validated);
    let configuration_fingerprint = validated.fingerprint().as_str().to_owned();
    let facts = MachineFacts::capture(&scenario.root.join("workspace"));
    assert_eq!(
        PathBuf::from(&facts.cwd),
        scenario.root.join("workspace").canonicalize().unwrap()
    );

    // Initialize the clean durable store with the production binary. This startup performs no
    // provider invocation and is stopped before offline device provisioning.
    scenario.spawn(&config_path, "initialize");
    wait_ready(&authority, scenario.child_mut()).await;
    let initializer = scenario.take_child();
    let initialized = stop_and_reap(initializer, 15).await;
    assert!(initialized.success(), "initializer did not stop cleanly");

    let bearer = provision_device(&config_path);
    let database = scenario.root.join("state/db/craxii.sqlite3");
    let durable_identity = load_identity(&database).await;
    let client = HeadlessClient::from_parts(
        authority.clone(),
        bearer,
        durable_identity.conversation_id.clone(),
    );

    scenario.spawn(&config_path, "first");
    wait_ready(&authority, scenario.child_mut()).await;
    let first_pid = scenario.child_mut().id();
    let first_runtime = runtime_for_pid(&database, first_pid).await;
    assert_recovery_preceded_readiness(&database, &first_runtime).await;

    let mut first_socket = client.websocket(0).await;
    let initial_frames = through_sync(&mut first_socket).await;
    assert_durable_cursor_contract(&initial_frames);
    let first_command_id = fresh_client_id();
    let first_acceptance = client.submit(CANONICAL_PROMPT, &first_command_id).await;
    assert_eq!(first_acceptance.status, 202);
    let first_work_id = required_string(&first_acceptance.body, "work_id");
    let first_message_id = required_string(&first_acceptance.body, "message_id");
    let (first_frames, first_terminal) =
        through_terminal_work(&mut first_socket, &first_work_id).await;
    if first_terminal != "work.completed" {
        write_live_failure_report(
            &report_path,
            &scenario.root,
            &database,
            "first_turn",
            &first_terminal,
            &first_work_id,
            &credential_patterns,
        )
        .await;
    }
    assert_durable_cursor_contract(&first_frames);
    let first_answer = committed_answer(&first_frames, &first_work_id);
    assert_machine_answer(&first_answer, &facts);
    let first_assistant_id = committed_message_id(&first_frames, &first_work_id);

    let first_bootstrap = client.bootstrap().await;
    assert_eq!(first_bootstrap.status, 200);
    let saved_cursor = first_bootstrap.body["snapshot_cursor"]
        .as_u64()
        .expect("snapshot cursor");
    assert_bootstrap_identity(&first_bootstrap.body, &durable_identity);
    first_socket.close(None).await.unwrap();

    let first_evidence = inspect_first_turn(
        &database,
        &first_work_id,
        &first_message_id,
        &first_assistant_id,
        &facts,
    )
    .await;
    scan_paths(std::slice::from_ref(&scenario.root), &credential_patterns)
        .expect("credential absent from live first-process evidence");

    let first_child = scenario.take_child();
    let killed = stop_and_reap(first_child, 9).await;
    assert_eq!(killed.signal(), Some(9));

    scenario.spawn(&config_path, "second");
    wait_ready(&authority, scenario.child_mut()).await;
    let second_pid = scenario.child_mut().id();
    assert_ne!(first_pid, second_pid);
    let second_runtime = runtime_for_pid(&database, second_pid).await;
    assert_ne!(first_runtime, second_runtime);
    assert_recovery_preceded_readiness(&database, &second_runtime).await;
    assert_eq!(load_identity(&database).await, durable_identity);

    let mut second_socket = client.websocket(saved_cursor).await;
    let reconnect_frames = through_sync(&mut second_socket).await;
    assert_durable_cursor_contract(&reconnect_frames);
    assert!(
        reconnect_frames
            .iter()
            .filter_map(durable_cursor)
            .all(|cursor| cursor > saved_cursor)
    );
    let recovered_bootstrap = client.bootstrap().await;
    assert_eq!(recovered_bootstrap.status, 200);
    assert_bootstrap_identity(&recovered_bootstrap.body, &durable_identity);

    let follow_command_id = fresh_client_id();
    let follow_acceptance = client.submit(FOLLOW_UP, &follow_command_id).await;
    assert_eq!(follow_acceptance.status, 202);
    let follow_work_id = required_string(&follow_acceptance.body, "work_id");
    let follow_message_id = required_string(&follow_acceptance.body, "message_id");
    assert_ne!(first_work_id, follow_work_id);
    assert_ne!(first_message_id, follow_message_id);
    let (follow_frames, follow_terminal) =
        through_terminal_work(&mut second_socket, &follow_work_id).await;
    if follow_terminal != "work.completed" {
        write_live_failure_report(
            &report_path,
            &scenario.root,
            &database,
            "follow_up",
            &follow_terminal,
            &follow_work_id,
            &credential_patterns,
        )
        .await;
    }
    assert_durable_cursor_contract(&follow_frames);
    let follow_answer = committed_answer(&follow_frames, &follow_work_id);
    assert_git_answer(&follow_answer, &facts.git_version);
    assert!(!follow_frames.iter().any(|frame| {
        matches!(
            frame["event_type"].as_str(),
            Some("tool.execution_started" | "tool.execution_finished")
        ) && frame["work_id"] == follow_work_id
    }));

    let follow_evidence = inspect_follow_up(
        &database,
        &follow_work_id,
        &follow_message_id,
        &first_evidence,
    )
    .await;
    let final_bootstrap = client.bootstrap().await;
    assert_eq!(final_bootstrap.status, 200);
    assert_bootstrap_identity(&final_bootstrap.body, &durable_identity);
    let final_cursor = final_bootstrap.body["snapshot_cursor"]
        .as_u64()
        .expect("final cursor");
    assert!(final_cursor > saved_cursor);
    second_socket.close(None).await.unwrap();

    let mut replay_socket = client.websocket(0).await;
    let replay_frames = through_sync(&mut replay_socket).await;
    assert_durable_cursor_contract(&replay_frames);
    assert!(replay_frames.iter().any(|frame| {
        frame["event_type"] == "work.completed" && frame["work_id"] == first_work_id
    }));
    assert!(replay_frames.iter().any(|frame| {
        frame["event_type"] == "work.completed" && frame["work_id"] == follow_work_id
    }));
    replay_socket.close(None).await.unwrap();

    scan_paths(std::slice::from_ref(&scenario.root), &credential_patterns)
        .expect("credential absent while SQLite WAL and telemetry are live");
    let second_child = scenario.take_child();
    let stopped = stop_and_reap(second_child, 15).await;
    assert!(stopped.success(), "second backend did not stop cleanly");
    let scanned_files = scan_paths(std::slice::from_ref(&scenario.root), &credential_patterns)
        .expect("credential absent from final Stage 25 evidence");

    let report = json!({
        "contract": "craxii.stage25.openai-headless.v1",
        "status": "passed",
        "configuration": {
            "configuration_version": 1,
            "fingerprint": configuration_fingerprint,
            "model_target": MODEL_TARGET,
            "provider": PROVIDER,
            "provider_model_id": MODEL,
            "endpoint": ENDPOINT,
            "context_window_tokens": CONTEXT_WINDOW,
            "max_output_tokens": MAX_OUTPUT,
            "requested_output_tokens": REQUESTED_OUTPUT,
            "reasoning_continuation": false
        },
        "credential": {
            "source": "local_directory",
            "identifier": CREDENTIAL_ID,
            "path": CREDENTIAL_PATH,
            "directory_mode": "0700",
            "file_mode": "0600",
            "directory_not_symlink": true,
            "file_not_symlink": true,
            "single_hard_link": true,
            "owner_matches": true,
            "existing_loader_accepted": true,
            "contents_included": false
        },
        "machine_facts": facts,
        "first_turn": first_evidence,
        "restart": {
            "first_runtime_id": first_runtime,
            "first_pid": first_pid,
            "termination": "SIGKILL_process_group",
            "second_runtime_id": second_runtime,
            "second_pid": second_pid,
            "runtime_changed": true,
            "pid_changed": true,
            "durable_identity_preserved": true,
            "startup_recovery_before_readiness": true
        },
        "follow_up": follow_evidence,
        "events": {
            "initial_sync_frames": initial_frames.len(),
            "first_turn_frames": first_frames.len(),
            "reconnect_frames": reconnect_frames.len(),
            "follow_up_frames": follow_frames.len(),
            "full_replay_frames": replay_frames.len(),
            "saved_cursor": saved_cursor,
            "final_cursor": final_cursor
        },
        "request_privacy": {
            "store": false,
            "conversation": null,
            "previous_response_id": null,
            "provider_owned_conversation_state": false,
            "truncation": "disabled"
        },
        "production_path": {
            "server_binary": "craxii-server",
            "http_submission": true,
            "durable_event_replay": true,
            "tool_execution_service": true,
            "tool_registry": true,
            "local_workstation": true,
            "scripted_provider_rows": 0,
            "deterministic_fixture_answer": false,
            "workstation_child_environment_policy": "env_clear_with_fixed_allowlist"
        },
        "secret_scan": {
            "passed": true,
            "files_scanned": scanned_files,
            "exact_credential_bytes_absent": true,
            "authorization_header_absent": true
        }
    });
    write_redacted_report(&report_path, &report, &credential_patterns);
    scenario.finish();
}

fn render_config(authority: &str, state: &Path, artifacts: &Path, workspace: &Path) -> String {
    TEMPLATE
        .replace("{{AUTHORITY}}", authority)
        .replace("{{STATE_ROOT}}", state.to_str().expect("UTF-8 state root"))
        .replace(
            "{{ARTIFACT_ROOT}}",
            artifacts.to_str().expect("UTF-8 artifact root"),
        )
        .replace(
            "{{WORKSPACE_ROOT}}",
            workspace.to_str().expect("UTF-8 workspace root"),
        )
}

fn assert_stage25_runtime_config(config: &config::ValidatedConfig) {
    assert_eq!(config.models().default_target(), MODEL_TARGET);
    let target = config
        .models()
        .targets()
        .iter()
        .find(|target| target.enabled())
        .expect("enabled Stage 25 target");
    assert_eq!(target.id(), MODEL_TARGET);
    assert_eq!(target.provider_model_id(), MODEL);
    assert_eq!(target.endpoint().as_str(), ENDPOINT);
    assert_eq!(target.context_window_tokens(), CONTEXT_WINDOW);
    assert_eq!(target.max_output_tokens(), MAX_OUTPUT);
    assert_eq!(target.requested_output_tokens(), REQUESTED_OUTPUT);
    assert!(!target.reasoning_continuation_required());
    assert_eq!(target.credential().as_str(), CREDENTIAL_ID);
    assert_eq!(
        config.credentials().source().local_directory(),
        Some(Path::new(CREDENTIAL_DIRECTORY))
    );
}

fn provider_failure_view(kind: ProviderErrorKind) -> Value {
    json!({
        "code": kind.code(),
        "retryable_before_output": matches!(
            kind,
            ProviderErrorKind::RateLimited
                | ProviderErrorKind::TemporarilyUnavailable
                | ProviderErrorKind::TransportBeforeResponse
                | ProviderErrorKind::TimeoutBeforeOutput
        ),
        "provider_message": null,
        "authorization": "[REDACTED]"
    })
}

fn redacted_failure_report(
    phase: &str,
    terminal_event: &str,
    code: Option<&str>,
    secret_scan_passed: bool,
) -> Value {
    let retryable = code.is_some_and(|code| {
        matches!(
            code,
            "rate_limited"
                | "temporarily_unavailable"
                | "transport_before_response"
                | "timeout_before_output"
        )
    });
    json!({
        "contract": "craxii.stage25.openai-headless.v1",
        "status": "failed",
        "phase": phase,
        "provider": PROVIDER,
        "provider_model_id": MODEL,
        "terminal_event": terminal_event,
        "failure": {
            "code": code,
            "retryable_before_output": retryable,
            "provider_message": null,
            "authorization": "[REDACTED]"
        },
        "secret_scan": {
            "passed": secret_scan_passed,
            "exact_credential_bytes_absent": secret_scan_passed
        }
    })
}

async fn write_live_failure_report(
    report_path: &Path,
    root: &Path,
    database: &Path,
    phase: &str,
    terminal_event: &str,
    work_id: &str,
    credential_patterns: &[Vec<u8>],
) -> ! {
    let mut connection = connect_read_only(database).await;
    let code: Option<String> = sqlx::query_scalar(
        "SELECT provider_error_kind FROM model_invocations \
         WHERE work_id = ? AND provider_error_kind IS NOT NULL \
         ORDER BY agent_step_no DESC, attempt_no DESC LIMIT 1",
    )
    .bind(work_id)
    .fetch_optional(&mut connection)
    .await
    .ok()
    .flatten()
    .flatten();
    let _ = connection.close().await;
    let secret_scan_passed = scan_paths(&[root.to_path_buf()], credential_patterns).is_ok();
    let report =
        redacted_failure_report(phase, terminal_event, code.as_deref(), secret_scan_passed);
    write_redacted_report(report_path, &report, credential_patterns);
    panic!("Stage 25 live work failed; see the redacted acceptance report")
}

#[derive(Debug, Eq, PartialEq)]
enum SecretScanError {
    SecretDetected,
    Storage,
    UnsafePath,
}

fn scan_paths(paths: &[PathBuf], patterns: &[Vec<u8>]) -> Result<usize, SecretScanError> {
    assert!(patterns.iter().all(|pattern| !pattern.is_empty()));
    let mut pending = paths.to_vec();
    let mut scanned = 0_usize;
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|_| SecretScanError::Storage)?;
        if metadata.file_type().is_symlink() {
            return Err(SecretScanError::UnsafePath);
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|_| SecretScanError::Storage)? {
                pending.push(entry.map_err(|_| SecretScanError::Storage)?.path());
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(SecretScanError::UnsafePath);
        }
        let bytes = fs::read(&path).map_err(|_| SecretScanError::Storage)?;
        scanned = scanned.saturating_add(1);
        if patterns
            .iter()
            .map(Vec::as_slice)
            .chain(FORBIDDEN_EVIDENCE_MARKERS)
            .any(|pattern| bytes.windows(pattern.len()).any(|window| window == pattern))
        {
            return Err(SecretScanError::SecretDetected);
        }
    }
    Ok(scanned)
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

struct TemporaryRoot {
    path: PathBuf,
}

impl TemporaryRoot {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{prefix}{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7().hyphenated()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        if self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("craxii-stage25-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct LiveScenario {
    root: PathBuf,
    child: Option<Child>,
    cleanup: bool,
}

impl LiveScenario {
    fn new() -> Self {
        Self {
            root: std::env::temp_dir().join(format!(
                "craxii-stage25-live-{}-{}",
                std::process::id(),
                uuid::Uuid::now_v7().hyphenated()
            )),
            child: None,
            cleanup: true,
        }
    }

    fn prepare(&self) {
        fs::create_dir(&self.root).unwrap();
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700)).unwrap();
        for child in ["state", "artifacts", "workspace"] {
            let path = self.root.join(child);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn spawn(&mut self, config: &Path, phase: &str) {
        assert!(self.child.is_none());
        let stdout = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(self.root.join(format!("telemetry-{phase}.stdout")))
            .unwrap();
        let stderr = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(self.root.join(format!("telemetry-{phase}.stderr")))
            .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_craxii-server"));
        command
            .args(["--config", config.to_str().unwrap()])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        self.child = Some(command.spawn().expect("spawn production Craxii backend"));
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("active Stage 25 backend")
    }

    fn take_child(&mut self) -> Child {
        self.child.take().expect("active Stage 25 backend")
    }

    fn finish(mut self) {
        assert!(self.child.is_none());
        fs::remove_dir_all(&self.root).unwrap();
        self.cleanup = false;
    }
}

impl Drop for LiveScenario {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = signal_owned_process_group(&child, 9);
            let _ = child.wait();
        }
        if self.cleanup
            && self
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("craxii-stage25-live-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn required_report_path() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("CRAXII_STAGE25_REPORT_PATH")
            .expect("CRAXII_STAGE25_REPORT_PATH is required"),
    );
    assert!(path.is_absolute() && !path.exists());
    let parent = path.parent().expect("report parent");
    let metadata = fs::symlink_metadata(parent).expect("report parent metadata");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.mode() & 0o777, 0o700);
    path
}

fn live_credential_preflight() -> Vec<Vec<u8>> {
    let directory = Path::new(CREDENTIAL_DIRECTORY);
    let path = Path::new(CREDENTIAL_PATH);
    let directory_metadata = fs::symlink_metadata(directory).expect("credential directory missing");
    assert!(directory_metadata.is_dir() && !directory_metadata.file_type().is_symlink());
    assert_eq!(directory_metadata.mode() & 0o777, 0o700);
    let metadata = fs::symlink_metadata(path).expect("credential file missing");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(metadata.uid(), directory_metadata.uid());

    let parsed = config::parse(&render_config(
        "127.0.0.1:38025",
        Path::new("/tmp/craxii-stage25-preflight/state"),
        Path::new("/tmp/craxii-stage25-preflight/artifacts"),
        Path::new("/tmp/craxii-stage25-preflight/workspace"),
    ))
    .unwrap();
    let loaded = load_credentials(
        parsed.credentials().source(),
        parsed
            .models()
            .targets()
            .iter()
            .filter(|target| target.enabled())
            .map(|target| target.credential()),
    )
    .expect("existing credential loader rejected Stage 25 credential metadata/value");
    assert_eq!(loaded.len(), 1);
    drop(loaded);

    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .expect("open credential for leak canary scan");
    let opened = file.metadata().unwrap();
    assert_eq!(
        (opened.dev(), opened.ino(), opened.len()),
        (metadata.dev(), metadata.ino(), metadata.len())
    );
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).unwrap();
    assert!(!raw.is_empty() && raw.len() <= 16 * 1024);
    while matches!(raw.last(), Some(b'\n' | b'\r')) {
        raw.pop();
    }
    assert!(!raw.is_empty());

    // This allocation is never formatted or persisted and is dropped with the live test.
    vec![raw]
}

fn available_authority() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap().to_string();
    drop(listener);
    authority
}

async fn wait_ready(authority: &str, child: &mut Child) {
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        if let Some(status) = child.try_wait().expect("poll backend") {
            panic!("backend exited before readiness with {status}");
        }
        if health_status(authority).await == Some(200) {
            return;
        }
        assert!(Instant::now() < deadline, "backend readiness timeout");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn health_status(authority: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(authority).await.ok()?;
    stream
        .write_all(
            format!("GET /health/ready HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.ok()?;
    let line = std::str::from_utf8(&response).ok()?.lines().next()?;
    line.split_whitespace().nth(1)?.parse().ok()
}

async fn stop_and_reap(mut child: Child, signal: i32) -> std::process::ExitStatus {
    signal_owned_process_group(&child, signal).expect("signal owned backend process group");
    let deadline = Instant::now() + EXIT_WAIT;
    loop {
        if let Some(status) = child.try_wait().expect("poll signalled backend") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "backend did not exit after signal"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn provision_device(config_path: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_craxii-admin"))
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "device",
            "provision",
            "Stage 25 headless",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("run production device provisioner");
    assert!(output.status.success(), "device provisioning failed");
    let bearer = String::from_utf8(output.stdout).expect("device bearer UTF-8");
    let bearer = bearer.trim_end_matches(['\r', '\n']).to_owned();
    assert!(!bearer.is_empty());
    bearer
}

fn fresh_client_id() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}

async fn through_terminal_work(
    socket: &mut headless_client::Socket,
    work_id: &str,
) -> (Vec<Value>, String) {
    let mut frames = Vec::new();
    loop {
        let frame = next_json_with_timeout(socket, LIVE_WAIT).await;
        let terminal = matches!(
            frame["event_type"].as_str(),
            Some("work.completed" | "work.failed" | "work.cancelled" | "work.interrupted")
        ) && frame["work_id"] == work_id;
        let event_type = frame["event_type"].as_str().unwrap_or_default().to_owned();
        frames.push(frame);
        if terminal {
            return (frames, event_type);
        }
    }
}

fn required_string(body: &Value, key: &str) -> String {
    body[key]
        .as_str()
        .unwrap_or_else(|| panic!("response omitted {key}"))
        .to_owned()
}

fn committed_answer(frames: &[Value], work_id: &str) -> String {
    frames
        .iter()
        .find(|frame| {
            frame["event_type"] == "assistant.message_committed" && frame["work_id"] == work_id
        })
        .and_then(|frame| frame["payload"]["content"].as_array())
        .expect("durable assistant content")
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn committed_message_id(frames: &[Value], work_id: &str) -> String {
    frames
        .iter()
        .find(|frame| {
            frame["event_type"] == "assistant.message_committed" && frame["work_id"] == work_id
        })
        .and_then(|frame| frame["payload"]["message_id"].as_str())
        .expect("durable assistant message ID")
        .to_owned()
}

fn assert_machine_answer(answer: &str, facts: &MachineFacts) {
    let lower = answer.to_ascii_lowercase();
    let os_present = lower.contains(&facts.os.to_ascii_lowercase())
        || (facts.os == "Darwin" && (lower.contains("macos") || lower.contains("mac os")));
    assert!(
        os_present,
        "assistant answer omitted the independently measured OS"
    );
    assert!(
        lower.contains(&facts.architecture.to_ascii_lowercase()),
        "assistant answer omitted the independently measured architecture"
    );
    assert!(
        answer.contains(&facts.cwd),
        "assistant answer omitted the measured cwd"
    );
    assert_git_answer(answer, &facts.git_version);
}

fn assert_git_answer(answer: &str, git_version: &str) {
    let expected = git_version
        .strip_prefix("git version ")
        .unwrap_or(git_version);
    assert!(
        answer
            .to_ascii_lowercase()
            .contains(&expected.to_ascii_lowercase()),
        "assistant answer omitted the independently measured Git version"
    );
}

fn durable_cursor(frame: &Value) -> Option<u64> {
    (frame["delivery_kind"] == "durable")
        .then(|| frame["cursor"].as_u64())
        .flatten()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DurableIdentity {
    craxii_id: String,
    conversation_id: String,
    workstation_id: String,
    workspace_id: String,
}

async fn connect_read_only(database: &Path) -> sqlx::SqliteConnection {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

async fn load_identity(database: &Path) -> DurableIdentity {
    let mut connection = connect_read_only(database).await;
    let row = sqlx::query(
        "SELECT p.craxii_id, p.primary_conversation_id, w.workstation_id, p.default_workspace_id \
         FROM craxii_principals p JOIN workspaces w ON w.workspace_id = p.default_workspace_id",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    DurableIdentity {
        craxii_id: row.get(0),
        conversation_id: row.get(1),
        workstation_id: row.get(2),
        workspace_id: row.get(3),
    }
}

fn assert_bootstrap_identity(body: &Value, expected: &DurableIdentity) {
    assert_eq!(body["craxii"]["craxii_id"], expected.craxii_id);
    assert_eq!(
        body["primary_conversation"]["conversation_id"],
        expected.conversation_id
    );
}

async fn runtime_for_pid(database: &Path, pid: u32) -> String {
    let mut connection = connect_read_only(database).await;
    let runtime: String = sqlx::query_scalar(
        "SELECT runtime_instance_id FROM runtime_instances WHERE process_id = ? ORDER BY started_at DESC LIMIT 1",
    )
    .bind(i64::from(pid))
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    runtime
}

async fn assert_recovery_preceded_readiness(database: &Path, runtime: &str) {
    let mut connection = connect_read_only(database).await;
    let recovery: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE runtime_instance_id = ? AND event_type = 'runtime.recovery_performed'",
    )
    .bind(runtime)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    assert_eq!(
        recovery, 1,
        "readiness observed without durable recovery evidence"
    );
}

async fn inspect_first_turn(
    database: &Path,
    work_id: &str,
    first_message_id: &str,
    assistant_message_id: &str,
    facts: &MachineFacts,
) -> Value {
    let mut connection = connect_read_only(database).await;
    let invocations = model_evidence(&mut connection, work_id).await;
    assert!(
        invocations.len() >= 2,
        "first turn did not continue after tools"
    );
    assert_provider_identity(&invocations);
    assert!(invocations.iter().any(|value| {
        value["tool_call_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    }));

    let tool_rows = sqlx::query(
        "SELECT tool_execution_id, source_model_invocation_id, agent_step_no, tool_name, state, \
                provider_tool_call_id, requested_at, dispatch_intent_at, started_at, completed_at, \
                cleanup_confirmed, result_json, runtime_instance_id, workstation_id, workspace_id \
         FROM tool_executions WHERE work_id = ? ORDER BY agent_step_no, tool_ordinal",
    )
    .bind(work_id)
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert!(
        !tool_rows.is_empty(),
        "first turn contains no real tool execution"
    );
    let invocation_ids: BTreeSet<String> = invocations
        .iter()
        .map(|value| value["model_invocation_id"].as_str().unwrap().to_owned())
        .collect();
    let mut tool_ids = Vec::new();
    let mut observed_output = String::new();
    let mut shell_execution = false;
    let mut source_model_ids = BTreeSet::new();
    for row in &tool_rows {
        let tool_id: String = row.get("tool_execution_id");
        let source: String = row.get("source_model_invocation_id");
        let name: String = row.get("tool_name");
        let state: String = row.get("state");
        let provider_call: Option<String> = row.get("provider_tool_call_id");
        assert!(invocation_ids.contains(&source));
        assert_eq!(state, "completed");
        assert!(provider_call.is_some());
        assert_tool_timing(row);
        let result: String = row.get("result_json");
        let result: Value = serde_json::from_str(&result).unwrap();
        let successful = result["result_kind"] == "success";
        observed_output.push_str(&tool_field_values(&result));
        shell_execution |= name == "run_shell"
            && successful
            && row.get::<Option<String>, _>("started_at").is_some();
        source_model_ids.insert(source);
        tool_ids.push(tool_id);
    }
    assert!(
        shell_execution,
        "canonical inspection never reached a real shell child"
    );
    assert_machine_evidence(&observed_output, facts);

    let final_invocation = invocations
        .iter()
        .filter(|value| value["state"] == "completed" && value["tool_call_count"] == 0)
        .max_by_key(|value| value["agent_step_no"].as_u64().unwrap())
        .expect("final answering model invocation");
    let final_invocation_id = final_invocation["model_invocation_id"].as_str().unwrap();
    let final_step = final_invocation["agent_step_no"].as_u64().unwrap();
    assert!(
        tool_rows
            .iter()
            .all(|row| (row.get::<i64, _>("agent_step_no") as u64) < final_step)
    );
    for tool_id in &tool_ids {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_manifest_sources s \
             JOIN model_invocations m ON m.context_manifest_id = s.context_manifest_id \
             WHERE m.model_invocation_id = ? AND s.source_record_kind = 'tool_execution' \
               AND s.source_record_id = ? AND s.source_kind = 'observed_tool_result'",
        )
        .bind(final_invocation_id)
        .bind(tool_id)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            count, 1,
            "final model context omitted a persisted tool result"
        );
    }
    let cause: String = sqlx::query_scalar(
        "SELECT cause.event_type FROM journal_events assistant \
         JOIN journal_events cause ON cause.event_id = assistant.causation_event_id \
         WHERE assistant.event_type = 'assistant.message_committed' AND assistant.work_id = ?",
    )
    .bind(work_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(cause, "model.invocation_completed");
    let cause_actor: String = sqlx::query_scalar(
        "SELECT cause.actor_id FROM journal_events assistant \
         JOIN journal_events cause ON cause.event_id = assistant.causation_event_id \
         WHERE assistant.event_type = 'assistant.message_committed' AND assistant.work_id = ?",
    )
    .bind(work_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(cause_actor, final_invocation_id);

    let assistant_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE message_id = ? AND produced_by_work_id = ? AND role = 'assistant'",
    )
    .bind(assistant_message_id)
    .bind(work_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(assistant_count, 1);
    let forbidden: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_invocations WHERE provider_id IN \
         ('stage18-scripted', 'scripted', 'context-answering', 'deterministic-fixture')",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(forbidden, 0);
    connection.close().await.unwrap();

    json!({
        "work_id": work_id,
        "accepted_http_status": 202,
        "completed": true,
        "canonical_prompt": CANONICAL_PROMPT,
        "user_message_id": first_message_id,
        "assistant_message_id": assistant_message_id,
        "provider": PROVIDER,
        "model": MODEL,
        "model_invocations": invocations,
        "model_originated_tool_calls": true,
        "tool_execution_count": tool_ids.len(),
        "tool_execution_ids": tool_ids,
        "source_model_invocation_ids": source_model_ids,
        "workstation_output_matches_independent_facts": true,
        "final_answer_downstream_of_tool_results": true,
        "final_model_invocation_id": final_invocation_id
    })
}

async fn inspect_follow_up(
    database: &Path,
    work_id: &str,
    follow_message_id: &str,
    first: &Value,
) -> Value {
    let mut connection = connect_read_only(database).await;
    let invocations = model_evidence(&mut connection, work_id).await;
    assert!(!invocations.is_empty());
    assert_provider_identity(&invocations);
    let tools: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_executions WHERE work_id = ?")
        .bind(work_id)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(tools, 0);

    let prior_user = first["user_message_id"].as_str().unwrap().to_owned();
    let prior_assistant = first["assistant_message_id"].as_str().unwrap().to_owned();
    assert!(manifest_source_count(&mut connection, work_id, "message", &prior_user).await > 0);
    assert!(manifest_source_count(&mut connection, work_id, "message", &prior_assistant).await > 0);
    for id in first["source_model_invocation_ids"].as_array().unwrap() {
        assert!(
            manifest_source_count(
                &mut connection,
                work_id,
                "model_invocation",
                id.as_str().unwrap(),
            )
            .await
                > 0
        );
    }
    for id in first["tool_execution_ids"].as_array().unwrap() {
        assert!(
            manifest_source_count(
                &mut connection,
                work_id,
                "tool_execution",
                id.as_str().unwrap(),
            )
            .await
                > 0
        );
    }
    let future_in_prior: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM context_manifest_sources s \
         JOIN context_manifests c ON c.context_manifest_id = s.context_manifest_id \
         WHERE c.work_id = ? AND s.source_record_kind = 'message' AND s.source_record_id = ?",
    )
    .bind(first["work_id"].as_str().unwrap())
    .bind(follow_message_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(future_in_prior, 0);
    connection.close().await.unwrap();
    json!({
        "work_id": work_id,
        "user_message_id": follow_message_id,
        "accepted_http_status": 202,
        "same_conversation": true,
        "completed": true,
        "provider": PROVIDER,
        "model": MODEL,
        "model_invocations": invocations,
        "prior_durable_turn_in_context": true,
        "prior_model_tool_chain_in_context": true,
        "future_message_excluded_from_prior_context": true,
        "git_version_matches_independent_measurement": true,
        "tool_execution_count": 0
    })
}

async fn manifest_source_count(
    connection: &mut sqlx::SqliteConnection,
    work_id: &str,
    kind: &str,
    id: &str,
) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM context_manifest_sources s \
         JOIN model_invocations m ON m.context_manifest_id = s.context_manifest_id \
         WHERE m.work_id = ? AND s.source_record_kind = ? AND s.source_record_id = ?",
    )
    .bind(work_id)
    .bind(kind)
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .unwrap()
}

async fn model_evidence(connection: &mut sqlx::SqliteConnection, work_id: &str) -> Vec<Value> {
    let rows = sqlx::query(
        "SELECT m.model_invocation_id, m.logical_invocation_id, m.context_manifest_id, \
                m.runtime_instance_id, m.agent_step_no, m.attempt_no, m.retry_of_invocation_id, \
                m.retry_reason, m.retry_delay_ms, m.provider_retry_after_ms, m.model_target_id, \
                m.provider_id, m.provider_model_id, m.target_configuration_version, \
                m.provider_options_json, m.state, m.provider_request_id, m.provider_response_id, \
                m.started_at, m.first_byte_at, m.first_output_at, m.completed_at, m.usage_status, \
                m.input_tokens, m.cached_input_tokens, m.output_tokens, m.reasoning_tokens, \
                m.total_tokens, m.tool_call_count, m.provider_error_kind, \
                m.provider_outcome_certainty, m.billing_ambiguity, \
                c.context_window_tokens, c.reserved_output_tokens \
         FROM model_invocations m JOIN context_manifests c \
           ON c.context_manifest_id = m.context_manifest_id \
         WHERE m.work_id = ? ORDER BY m.agent_step_no, m.attempt_no",
    )
    .bind(work_id)
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter().map(model_row_evidence).collect()
}

fn model_row_evidence(row: &sqlx::sqlite::SqliteRow) -> Value {
    let started: String = row.get("started_at");
    let first_byte: Option<String> = row.get("first_byte_at");
    let first_output: Option<String> = row.get("first_output_at");
    let completed: Option<String> = row.get("completed_at");
    let state: String = row.get("state");
    assert!(
        completed.is_some(),
        "live model attempt did not reach a terminal state"
    );
    let completed_value = completed.as_deref().unwrap();
    assert_timestamp_order(
        &started,
        first_byte.as_deref(),
        first_output.as_deref(),
        completed_value,
    );
    let usage_status: String = row.get("usage_status");
    assert!(matches!(usage_status.as_str(), "reported" | "unavailable"));
    let input: Option<i64> = row.get("input_tokens");
    let cached: Option<i64> = row.get("cached_input_tokens");
    let output: Option<i64> = row.get("output_tokens");
    let reasoning: Option<i64> = row.get("reasoning_tokens");
    let total: Option<i64> = row.get("total_tokens");
    if usage_status == "reported" {
        assert!(
            input.is_some()
                && cached.is_some()
                && output.is_some()
                && reasoning.is_some()
                && total.is_some()
        );
    } else {
        assert!(
            input.is_none()
                && cached.is_none()
                && output.is_none()
                && reasoning.is_none()
                && total.is_none()
        );
    }
    let attempt: i64 = row.get("attempt_no");
    let retry_of: Option<String> = row.get("retry_of_invocation_id");
    let retry_reason: Option<String> = row.get("retry_reason");
    let retry_delay: Option<i64> = row.get("retry_delay_ms");
    if attempt == 1 {
        assert!(retry_of.is_none() && retry_reason.is_none() && retry_delay.is_none());
    } else {
        assert!(retry_of.is_some() && retry_reason.is_some() && retry_delay.is_some());
    }
    let provider_options: String = row.get("provider_options_json");
    let provider_options: Value = serde_json::from_str(&provider_options).unwrap();
    assert_eq!(provider_options["reasoning_continuation"], false);
    assert_eq!(
        row.get::<i64, _>("context_window_tokens"),
        CONTEXT_WINDOW as i64
    );
    assert_eq!(
        row.get::<i64, _>("reserved_output_tokens"),
        REQUESTED_OUTPUT as i64
    );
    if state == "completed" {
        assert!(
            row.get::<Option<String>, _>("provider_request_id")
                .is_some()
        );
        assert!(
            row.get::<Option<String>, _>("provider_response_id")
                .is_some()
        );
    }
    json!({
        "model_invocation_id": row.get::<String, _>("model_invocation_id"),
        "logical_invocation_id": row.get::<String, _>("logical_invocation_id"),
        "context_manifest_id": row.get::<String, _>("context_manifest_id"),
        "runtime_instance_id": row.get::<String, _>("runtime_instance_id"),
        "agent_step_no": row.get::<i64, _>("agent_step_no"),
        "attempt_no": attempt,
        "retry_of_invocation_id": retry_of,
        "retry_reason": retry_reason,
        "retry_delay_ms": retry_delay,
        "provider_retry_after_ms": row.get::<Option<i64>, _>("provider_retry_after_ms"),
        "model_target_id": row.get::<String, _>("model_target_id"),
        "provider_id": row.get::<String, _>("provider_id"),
        "provider_model_id": row.get::<String, _>("provider_model_id"),
        "target_configuration_version": row.get::<i64, _>("target_configuration_version"),
        "reasoning_continuation": false,
        "context_window_tokens": row.get::<i64, _>("context_window_tokens"),
        "reserved_output_tokens": row.get::<i64, _>("reserved_output_tokens"),
        "state": state,
        "provider_request_id_available": row.get::<Option<String>, _>("provider_request_id").is_some(),
        "provider_response_id_available": row.get::<Option<String>, _>("provider_response_id").is_some(),
        "started_at": started,
        "first_byte_at": first_byte,
        "first_output_at": first_output,
        "completed_at": completed_value,
        "provider_latency_ms": elapsed_ms(&started, completed_value),
        "first_byte_latency_ms": first_byte.as_deref().map(|value| elapsed_ms(&started, value)),
        "first_output_latency_ms": first_output.as_deref().map(|value| elapsed_ms(&started, value)),
        "usage_status": usage_status,
        "usage": if input.is_some() { json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
            "reasoning_tokens": reasoning,
            "total_tokens": total
        }) } else { Value::Null },
        "tool_call_count": row.get::<Option<i64>, _>("tool_call_count"),
        "provider_error_kind": row.get::<Option<String>, _>("provider_error_kind")
        ,"provider_outcome_certainty": row.get::<Option<String>, _>("provider_outcome_certainty"),
        "billing_ambiguity": row.get::<i64, _>("billing_ambiguity") != 0
    })
}

fn assert_provider_identity(invocations: &[Value]) {
    for invocation in invocations {
        assert_eq!(invocation["provider_id"], PROVIDER);
        assert_eq!(invocation["provider_model_id"], MODEL);
        assert_eq!(invocation["model_target_id"], MODEL_TARGET);
        assert_eq!(invocation["target_configuration_version"], 1);
    }
}

fn assert_timestamp_order(
    started: &str,
    first_byte: Option<&str>,
    first_output: Option<&str>,
    completed: &str,
) {
    let started = parse_timestamp(started);
    let completed = parse_timestamp(completed);
    assert!(started <= completed);
    if let Some(value) = first_byte {
        let value = parse_timestamp(value);
        assert!(started <= value && value <= completed);
    }
    if let Some(value) = first_output {
        let value = parse_timestamp(value);
        assert!(started <= value && value <= completed);
        if let Some(first_byte) = first_byte {
            assert!(parse_timestamp(first_byte) <= value);
        }
    }
}

fn parse_timestamp(value: &str) -> time::OffsetDateTime {
    UtcTimestamp::parse_canonical(value)
        .unwrap()
        .to_offset_datetime()
}

fn elapsed_ms(start: &str, end: &str) -> i128 {
    (parse_timestamp(end) - parse_timestamp(start)).whole_milliseconds()
}

fn assert_tool_timing(row: &sqlx::sqlite::SqliteRow) {
    let requested: String = row.get("requested_at");
    let dispatch: String = row.get::<Option<String>, _>("dispatch_intent_at").unwrap();
    let started: Option<String> = row.get("started_at");
    let completed: String = row.get::<Option<String>, _>("completed_at").unwrap();
    assert!(parse_timestamp(&requested) <= parse_timestamp(&dispatch));
    if let Some(started) = started {
        assert!(parse_timestamp(&dispatch) <= parse_timestamp(&started));
        assert!(parse_timestamp(&started) <= parse_timestamp(&completed));
        if row.get::<String, _>("tool_name") == "run_shell" {
            assert_eq!(row.get::<Option<i64>, _>("cleanup_confirmed"), Some(1));
        }
    }
}

fn tool_field_values(result: &Value) -> String {
    let mut fields = BTreeMap::new();
    for pair in result["fields"].as_array().expect("tool result fields") {
        let pair = pair.as_array().expect("tool field pair");
        fields.insert(
            pair[0].as_str().expect("tool field key"),
            pair[1].as_str().expect("tool field value"),
        );
    }
    fields.values().copied().collect::<Vec<_>>().join("\n")
}

fn assert_machine_evidence(output: &str, facts: &MachineFacts) {
    let lower = output.to_ascii_lowercase();
    assert!(
        lower.contains(&facts.os.to_ascii_lowercase())
            || (facts.os == "Darwin" && (lower.contains("macos") || lower.contains("mac os")))
    );
    assert!(output.contains(&facts.architecture));
    assert!(output.contains(&facts.cwd));
    assert!(output.contains(&facts.git_version));
}

fn write_redacted_report(path: &Path, report: &Value, patterns: &[Vec<u8>]) {
    let bytes = serde_json::to_vec_pretty(report).unwrap();
    assert!(
        !bytes
            .windows(b"Authorization: Bearer".len())
            .any(|window| window == b"Authorization: Bearer")
    );
    for pattern in patterns {
        assert!(
            !bytes
                .windows(pattern.len())
                .any(|window| window == pattern.as_slice())
        );
    }
    write_new(path, &bytes, 0o600);
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
}
