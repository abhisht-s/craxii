//! Explicit spend-bearing Stage 19 smoke. It is ignored and excluded from normal verification.

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use craxii_server::adapters::sqlite::SqliteStateStore;
use craxii_server::application::device_provisioning::DeviceProvisioningService;
use craxii_server::bootstrap::health::HealthState;
use craxii_server::bootstrap::startup;
use craxii_server::domain::{ClientMessageId, DeviceDisplayName, UtcTimestamp, WorkId};
use serde_json::{Value, json};
use sqlx::{Connection as _, Row as _};

#[tokio::test]
#[ignore = "explicit live OpenAI smoke; requires the Stage 19 wrapper script"]
async fn live_openai_production_path_persists_tool_and_final_completion() {
    assert_eq!(std::env::var("CRAXII_OPENAI_LIVE").as_deref(), Ok("1"));
    assert!(std::env::var_os("OPENAI_API_KEY").is_none());
    let credential_directory = required_path("CRAXII_STAGE19_LIVE_CREDENTIAL_DIR");
    let model = required_safe_model("CRAXII_OPENAI_MODEL");
    let endpoint = std::env::var("CRAXII_OPENAI_ENDPOINT")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
    let endpoint = url::Url::parse(&endpoint).expect("valid live OpenAI endpoint");
    assert_eq!(endpoint.scheme(), "https");
    assert!(endpoint.query().is_none() && endpoint.fragment().is_none());
    let context_tokens = positive_env("CRAXII_OPENAI_CONTEXT_TOKENS", 1_050_000);
    let max_output_tokens = positive_env("CRAXII_OPENAI_MAX_OUTPUT_TOKENS", 128_000);
    let requested_output_tokens = positive_env("CRAXII_OPENAI_REQUESTED_OUTPUT_TOKENS", 1_024);
    assert!(requested_output_tokens <= max_output_tokens);

    let root = live_root();
    prepare_root(&root);
    fs::write(
        root.join("workspace/stage19-live-fixture.txt"),
        "stage19-live-deterministic-marker\n",
    )
    .unwrap();
    let authority = available_authority();
    let config_path = root.join("config.toml");
    fs::write(
        &config_path,
        live_config(
            &root,
            &credential_directory,
            &authority,
            endpoint.as_str(),
            &model,
            context_tokens,
            max_output_tokens,
            requested_output_tokens,
        ),
    )
    .unwrap();

    let running = startup::run([
        "craxii-server".into(),
        "--config".into(),
        config_path.as_os_str().to_owned(),
    ])
    .await
    .expect("production startup composition");
    for _ in 0..1_000 {
        if running.application().health().snapshot().state() == HealthState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        running.application().health().snapshot().state(),
        HealthState::Ready
    );

    let store = SqliteStateStore::new(running.sqlite_runtime().runtime().clone());
    let created_at = UtcTimestamp::from_offset_datetime(time::OffsetDateTime::now_utc()).unwrap();
    let provisioned = DeviceProvisioningService::new(&store)
        .provision(
            DeviceDisplayName::try_new("Stage 19 live smoke".to_owned()).unwrap(),
            created_at,
        )
        .await
        .expect("provision live-smoke device");
    let mut token_bytes = Vec::new();
    provisioned.write_bearer_once(&mut token_bytes).unwrap();
    let bearer = String::from_utf8(token_bytes).unwrap();
    let bearer = bearer.trim_end();
    let conversation_id = running
        .application()
        .bootstrap_snapshot()
        .identity
        .conversation_id;
    let client_message_id = fresh_client_message_id();
    let body = serde_json::to_vec(&json!({
        "protocol_version": 1,
        "client_message_id": client_message_id.to_string(),
        "content": [{
            "type": "text",
            "text": "Call read_file exactly once with path stage19-live-fixture.txt. Do not call run_shell. After the tool result, finish with one concise answer."
        }]
    }))
    .unwrap();
    let response = http_post(
        &authority,
        &format!("/v1/conversations/{conversation_id}/messages"),
        bearer,
        &client_message_id.to_string(),
        &body,
    );
    assert_eq!(response.0, 200);
    let response: Value = serde_json::from_slice(&response.1).expect("message response JSON");
    let work_id: WorkId = response["work_id"]
        .as_str()
        .expect("work ID")
        .parse()
        .unwrap();

    let database = root.join("state/db/craxii.sqlite3");
    let state = wait_for_work(&database, work_id).await;
    assert_eq!(state, "completed");
    let mut connection = connect_read_only(&database).await;
    let model_attempts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM model_invocations WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let complete_provider_evidence: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_invocations WHERE work_id = ? AND state = 'completed' \
         AND provider_request_id IS NOT NULL AND provider_response_id IS NOT NULL \
         AND usage_status = 'reported'",
    )
    .bind(work_id.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let retries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_invocations WHERE work_id = ? AND attempt_no <> 1",
    )
    .bind(work_id.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let tools: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tool_executions WHERE work_id = ? AND state = 'completed'",
    )
    .bind(work_id.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let assistants: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE produced_by_work_id = ? AND role = 'assistant'",
    )
    .bind(work_id.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    assert_eq!(model_attempts, 2);
    assert_eq!(complete_provider_evidence, 2);
    assert_eq!(retries, 0);
    assert_eq!(tools, 1);
    assert_eq!(assistants, 1);

    running.shutdown().await.expect("live smoke shutdown");
    eprintln!("STAGE_19_LIVE_OPENAI_SMOKE: PASS requests=2 retries=0 tool_calls=1");
}

fn required_path(name: &str) -> PathBuf {
    let value = std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required"));
    PathBuf::from(value)
}

fn required_safe_model(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"));
    assert!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    );
    value
}

fn positive_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| value.parse::<u64>().expect("positive numeric live setting"))
        .unwrap_or(default)
        .max(1)
}

struct LiveRoot(PathBuf);

impl Deref for LiveRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for LiveRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn live_root() -> LiveRoot {
    LiveRoot(std::env::temp_dir().join(format!(
        "craxii-stage19-live-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7().hyphenated()
    )))
}

fn fresh_client_message_id() -> ClientMessageId {
    ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

fn prepare_root(root: &Path) {
    fs::create_dir(root).unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    for child in ["state", "workspace"] {
        fs::create_dir(root.join(child)).unwrap();
        fs::set_permissions(root.join(child), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn available_authority() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap().to_string();
    drop(listener);
    authority
}

#[allow(clippy::too_many_arguments)]
fn live_config(
    root: &Path,
    credential_directory: &Path,
    authority: &str,
    endpoint: &str,
    model: &str,
    context_tokens: u64,
    max_output_tokens: u64,
    requested_output_tokens: u64,
) -> String {
    format!(
        r#"configuration_version = 1
failpoint_mode = "disabled"

[server]
bind_address = "{authority}"
public_base_url = "http://{authority}"

[paths]
state_root = "{state}"
artifact_root = "{artifacts}"
primary_workspace_root = "{workspace}"

[sqlite]
pool_connections = 4
busy_timeout_ms = 5000
wal_autocheckpoint_pages = 1000

[workstation]
identity_source = "state_store"
initial_generation = 1
primary_workspace_logical_name = "primary"

[credentials]
source = "local_directory"
directory = "{credentials}"
declared = ["openai_live"]

[models]
default_target = "primary"

[[models.targets]]
id = "primary"
config_version = 1
enabled = true
provider = "openai"
provider_model_id = "{model}"
endpoint = "{endpoint}"
credential = "openai_live"
token_estimator = "conservative_v1"
context_window_tokens = {context_tokens}
max_output_tokens = {max_output_tokens}
requested_output_tokens = {requested_output_tokens}
reasoning_continuation = false

[models.targets.capabilities]
text_input = true
text_output = true
custom_tool_calling = true
streaming = true
ordered_output_items = true
structured_output = false
reasoning_continuation = false

[model_gateway]
max_attempts_per_invocation = 3
invocation_timeout_ms = 300000
response_idle_timeout_ms = 60000

[limits.agent]
max_model_steps_per_work = 16
max_model_attempts_per_work = 32
max_tool_calls_per_work = 32
max_ordered_output_items_per_response = 64
max_raw_tool_argument_bytes = 65536
max_work_item_duration_ms = 1800000

[limits.tools]
read_file_default_bytes = 1048576
read_file_max_bytes = 8388608
run_shell_command_max_bytes = 65536
run_shell_default_timeout_ms = 120000
run_shell_max_timeout_ms = 900000
stdout_capture_bytes = 8388608
stderr_capture_bytes = 8388608
inline_model_result_bytes = 65536
per_stream_projection_bytes = 32768

[limits.protocol]
websocket_durable_payload_bytes = 262144
user_text_message_bytes = 65536

[shell]
executable = "/bin/bash"
environment_policy = "clean"
inherited_variables = []
administrative_enabled = false

[device_auth]
source = "provisioned_sqlite"

[tracing]
format = "pretty"
filter = "error"

[shutdown]
grace_period_ms = 10000
"#,
        state = root.join("state").display(),
        artifacts = root.join("artifacts").display(),
        workspace = root.join("workspace").display(),
        credentials = credential_directory.display(),
    )
}

fn http_post(
    authority: &str,
    path: &str,
    bearer: &str,
    idempotency_key: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(authority).expect("connect live smoke HTTP");
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {bearer}\r\nConnection: close\r\nIdempotency-Key: {idempotency_key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = std::str::from_utf8(&bytes[..split]).unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, bytes[split + 4..].to_vec())
}

async fn connect_read_only(database: &Path) -> sqlx::SqliteConnection {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

async fn wait_for_work(database: &Path, work_id: WorkId) -> String {
    for _ in 0..1_800 {
        let mut connection = connect_read_only(database).await;
        let row = sqlx::query("SELECT state FROM work_items WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_optional(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        if let Some(row) = row {
            let state: String = row.get(0);
            if matches!(
                state.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            ) {
                return state;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    "timeout".to_owned()
}
