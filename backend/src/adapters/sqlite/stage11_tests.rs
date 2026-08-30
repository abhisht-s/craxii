use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message as WebSocketMessage};

use crate::adapters::http::{
    ConnectionRegistry, HttpState, ServerErrorKind, ServerHandle, TestPostCommitGate,
    TestUpgradeGate,
};
use crate::adapters::system_clock::SystemClock;
use crate::application::command_service::{AcceptMessageCommand, CommandService};
use crate::application::device_provisioning::DeviceProvisioningService;
use crate::application::publication::PublicStateService;
use crate::application::runtime::{
    ControlledShutdown, ControlledShutdownFuture, HeartbeatTask, ShutdownController,
    ShutdownReceipt, bootstrap_runtime,
};
use crate::application::transport::{CursorBroadcaster, MutationAdmission};
use crate::bootstrap::health::Health;
use crate::domain::*;
use crate::ports::clock::TestClock;
use crate::ports::state_store::{
    BeginRuntimeStoppingRequest, BootstrapObservation, BootstrapStateStore,
    ListPublicJournalRequest, LoadOrBootstrapIdentityRequest, RuntimeStateStore,
    V0IdentityReference,
};
use crate::protocol::{ProtocolVersion, ReplayCursor};

use super::stage11::{Stage11SnapshotBarrier, Stage11SnapshotPoint, Stage11SnapshotTestHook};
use super::{SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2020-01-01T03:00:00.000000Z";
const TOKEN: &str = "efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef";
const MESSAGE_BODY_CANARY: &str = "message-body-secret-canary";
const PROVIDER_CANARY: &str = "provider-secret-canary";
const MODEL_CANARY: &str = "model-evidence-secret-canary";
const TOOL_ARGUMENTS_CANARY: &str = "tool-arguments-secret-canary";
const TOOL_RESULT_CANARY: &str = "tool-result-output-secret-canary";
const ARTIFACT_METADATA_CANARY: &str = "artifact-metadata-secret-canary";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage11-test-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: Arc<SqliteStateStore>,
    identity: V0IdentityReference,
    device_id: DeviceId,
    token: String,
    health: Health,
    admission: MutationAdmission,
    cursors: CursorBroadcaster,
    connections: ConnectionRegistry,
    fatal_receiver: tokio::sync::watch::Receiver<bool>,
    shutdown_authority: Arc<dyn ControlledShutdown>,
    shutdown_controller: Option<Arc<ShutdownController<SqliteStateStore, TestClock>>>,
    server: Option<ServerHandle>,
    authority: String,
}

#[derive(Default)]
struct TestShutdownLatch {
    requested: AtomicBool,
}

impl ControlledShutdown for TestShutdownLatch {
    fn request_controlled_shutdown(&self) -> ControlledShutdownFuture<'_> {
        self.requested.store(true, Ordering::Release);
        Box::pin(async {
            Ok(ShutdownReceipt {
                shutdown_requested_at: at(T0),
                grace_deadline: at("2020-01-01T03:00:05.000000Z"),
                began: true,
            })
        })
    }

    fn shutdown_is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

impl Harness {
    async fn new() -> Self {
        Self::new_with_options(None, None, None, false).await
    }

    async fn new_with_stall(ws_send_stall: Option<Arc<AtomicBool>>) -> Self {
        Self::new_with_options(ws_send_stall, None, None, false).await
    }

    async fn new_with_upgrade_gate(upgrade_gate: Arc<TestUpgradeGate>) -> Self {
        Self::new_with_options(None, Some(upgrade_gate), None, false).await
    }

    async fn new_with_post_commit_gate(gate: Arc<TestPostCommitGate>) -> Self {
        Self::new_with_options(None, None, Some(gate), false).await
    }

    async fn new_with_controlled_shutdown() -> Self {
        Self::new_with_options(None, None, None, true).await
    }

    async fn new_with_options(
        ws_send_stall: Option<Arc<AtomicBool>>,
        upgrade_gate: Option<Arc<TestUpgradeGate>>,
        post_commit_gate: Option<Arc<TestPostCommitGate>>,
        controlled: bool,
    ) -> Self {
        let root = TestRoot::new();
        let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
        let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
        let identity = store
            .load_or_bootstrap_v0_identity(bootstrap_request())
            .await
            .unwrap()
            .identity;
        let provisioned = DeviceProvisioningService::new(store.as_ref())
            .provision_fixture_token(
                DeviceDisplayName::try_new("Stage 11 device".into()).unwrap(),
                at(T0),
                BearerToken::parse(TOKEN.to_owned()).unwrap(),
            )
            .await
            .unwrap();
        let device_id = provisioned.summary.device_id;
        let mut token = Vec::new();
        provisioned.write_bearer_once(&mut token).unwrap();
        let token = String::from_utf8(token).unwrap().trim().to_owned();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let health = Health::new();
        let admission = MutationAdmission::new();
        let cursors = CursorBroadcaster::new();
        let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
        let (ws_shutdown, _) = tokio::sync::watch::channel(false);
        let connections = ConnectionRegistry::default();
        let shutdown_controller = if controlled {
            let runtime_instance_id = RuntimeInstanceId::generate();
            let shutdown_clock =
                Arc::new(TestClock::new(at(T0).to_offset_datetime(), Duration::ZERO));
            let runtime = bootstrap_runtime(
                store.as_ref(),
                runtime_evidence(identity, runtime_instance_id),
                0,
                shutdown_clock.as_ref(),
            )
            .await
            .unwrap();
            let heartbeat = HeartbeatTask::start(
                Arc::clone(&store),
                Arc::clone(&shutdown_clock),
                health.clone(),
                runtime_instance_id,
                fatal.clone(),
            );
            Some(Arc::new(ShutdownController::new(
                Arc::clone(&store),
                shutdown_clock,
                health.clone(),
                runtime_instance_id,
                runtime.correlation_id,
                5_000,
                heartbeat,
            )))
        } else {
            None
        };
        let shutdown_authority: Arc<dyn ControlledShutdown> =
            shutdown_controller.as_ref().map_or_else(
                || Arc::new(TestShutdownLatch::default()) as Arc<dyn ControlledShutdown>,
                |shutdown| shutdown.clone(),
            );
        let mut state = HttpState::new(
            Arc::clone(&store),
            Arc::new(SystemClock::new()),
            health.clone(),
            admission.clone(),
            cursors.clone(),
            fatal,
            ws_shutdown,
            connections.clone(),
            vec![authority.clone()],
            Some(Arc::clone(&shutdown_authority)),
        );
        if let Some(stall) = ws_send_stall {
            state = state.with_test_ws_send_stall(stall);
        }
        if let Some(gate) = upgrade_gate {
            state = state.with_test_upgrade_gate(gate);
        }
        if let Some(gate) = post_commit_gate {
            state = state.with_test_post_commit_gate(gate);
        }
        let server = ServerHandle::start(listener, state);
        Self {
            _root: root,
            guard,
            store,
            identity,
            device_id,
            token,
            health,
            admission,
            cursors,
            connections,
            fatal_receiver,
            shutdown_authority,
            shutdown_controller,
            server: Some(server),
            authority,
        }
    }

    async fn stop_server(&mut self) {
        if let Some(server) = self.server.take() {
            self.shutdown_authority
                .request_controlled_shutdown()
                .await
                .unwrap();
            server.stop_accepting();
            server.close_websockets();
            server.join().await.unwrap();
        }
    }

    async fn close(mut self) {
        self.stop_server().await;
        self.guard.runtime().close().await;
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> HttpResponse {
        let mut stream = tokio::net::TcpStream::connect(&self.authority)
            .await
            .unwrap();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.authority,
            body.len()
        );
        for (name, value) in headers {
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        HttpResponse::parse(&bytes)
    }

    async fn authenticated_json(
        &self,
        method: &str,
        path: &str,
        idempotency_key: Option<&str>,
        body: &str,
    ) -> HttpResponse {
        let mut headers = vec![
            ("Authorization", format!("Bearer {}", self.token)),
            ("Content-Type", "application/json".to_owned()),
        ];
        if let Some(key) = idempotency_key {
            headers.push(("Idempotency-Key", key.to_owned()));
        }
        self.request(method, path, &headers, body).await
    }

    fn ws_request(&self, after: ReplayCursor) -> tokio_tungstenite::tungstenite::http::Request<()> {
        let mut request = format!("ws://{}/v1/events?after={after}", self.authority)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        request
    }
}

struct HttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn parse(bytes: &[u8]) -> Self {
        let split = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        Self {
            status,
            headers,
            body: bytes[split + 4..].to_vec(),
        }
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn at(value: &str) -> UtcTimestamp {
    value.parse().unwrap()
}

fn bootstrap_request() -> LoadOrBootstrapIdentityRequest {
    LoadOrBootstrapIdentityRequest {
        proposed: V0IdentityReference {
            craxii_id: CraxiiId::generate(),
            conversation_id: ConversationId::generate(),
            workstation_id: WorkstationId::generate(),
            workspace_id: WorkspaceId::generate(),
        },
        initialized_event_id: JournalEventId::generate(),
        conversation_created_event_id: JournalEventId::generate(),
        correlation_id: CorrelationId::generate(),
        created_at: at(T0),
        observation: BootstrapObservation {
            initial_generation: WorkstationGeneration::try_new(1).unwrap(),
            architecture: "stage11-secret-architecture".into(),
            os_release: "stage11-secret-os".into(),
            default_shell: "/bin/secret-shell".into(),
            workspace_logical_name: "primary".into(),
            workspace_logical_root: "/secret/workspace".into(),
            workspace_resolved_root: "/secret/resolved/workspace".into(),
            execution_capabilities:
                crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
        },
    }
}

fn runtime_evidence(
    identity: V0IdentityReference,
    runtime_instance_id: RuntimeInstanceId,
) -> RuntimeStartEvidence {
    RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
        runtime_instance_id,
        craxii_id: identity.craxii_id,
        workstation_id: identity.workstation_id,
        workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
        linux_boot_id: Some(LinuxBootId::try_new("stage11-test-boot").unwrap()),
        diagnostic_pid: Some(DiagnosticPid::try_new(111).unwrap()),
        package_version: PackageVersion::try_new("0.0.1").unwrap(),
        git_revision: GitRevision::try_new("stage11-test").unwrap(),
        schema_version: SchemaVersion::try_new(3).unwrap(),
        started_at: at(T0),
    })
}

fn client_id() -> ClientMessageId {
    ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

fn command_id() -> ClientCommandId {
    ClientCommandId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

fn message_body(id: ClientMessageId, text: &str) -> String {
    json!({
        "protocol_version": 1,
        "client_message_id": id,
        "content": [{"type": "text", "text": text}],
    })
    .to_string()
}

async fn journal_count(store: &SqliteStateStore) -> i64 {
    let mut connection = store.runtime.acquire().await.unwrap();
    sqlx::query_scalar("SELECT count(*) FROM journal_events")
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

async fn accept_without_hint(harness: &Harness, id: ClientMessageId, text: &str) -> WorkId {
    CommandService::new(harness.store.as_ref())
        .accept_message(
            AuthenticatedDevice::new(harness.device_id),
            AcceptMessageCommand {
                idempotency_key: IdempotencyKey::for_message(id),
                client_message_id: id,
                conversation_id: harness.identity.conversation_id,
                content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
                accepted_at: at("2020-01-01T03:00:01.000000Z"),
            },
        )
        .await
        .unwrap()
        .into_receipt()
        .work_id
}

async fn insert_filtered_initialization_clones(store: &SqliteStateStore, count: usize) {
    let mut connection = store.runtime.acquire().await.unwrap();
    for _ in 0..count {
        sqlx::query(
            "INSERT INTO journal_events \
             (event_id, craxii_id, stream_id, stream_seq, event_type, event_version, \
              conversation_id, work_id, causation_event_id, correlation_id, actor_kind, \
              actor_id, runtime_instance_id, payload_json, payload_sha256, recorded_at, occurred_at) \
             SELECT ?, craxii_id, stream_id, \
                    (SELECT max(existing.stream_seq) + 1 FROM journal_events existing \
                     WHERE existing.stream_id = journal_events.stream_id), \
                    event_type, event_version, conversation_id, work_id, NULL, ?, actor_kind, \
                    actor_id, runtime_instance_id, payload_json, payload_sha256, recorded_at, occurred_at \
             FROM journal_events WHERE event_type = 'craxii.initialized' LIMIT 1",
        )
        .bind(JournalEventId::generate().to_string())
        .bind(CorrelationId::generate().to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    }
}

async fn insert_dynamic_redaction_canaries(
    store: &SqliteStateStore,
    identity: V0IdentityReference,
    work_id: WorkId,
) -> Vec<String> {
    let artifact_id = ArtifactId::generate();
    let artifact_sha = "a".repeat(64);
    let artifact_storage_key = format!("sha256/aa/{artifact_sha}");
    let mut connection = store.runtime.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO artifacts (
             artifact_id, craxii_id, producing_work_id, producer_kind, producer_id, backend,
             storage_key, sha256, captured_byte_count, observed_byte_count, mime_type, encoding,
             logical_name, retention_class, truncated, compression, created_at
         ) VALUES (?, ?, NULL, NULL, NULL, 'local', ?, ?, 0, NULL,
                   'application/octet-stream', NULL, ?, 'diagnostic', 0, NULL, ?)",
    )
    .bind(artifact_id.to_string())
    .bind(identity.craxii_id.to_string())
    .bind(&artifact_storage_key)
    .bind(&artifact_sha)
    .bind(ARTIFACT_METADATA_CANARY)
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();

    let manifest_id = ContextManifestId::generate();
    sqlx::query(
        "INSERT INTO context_manifests (
             context_manifest_id, work_id, logical_invocation_id, model_target_id, provider_id,
             provider_model_id, target_configuration_version, model_capabilities_json,
             assembler_version, context_policy_version, system_prompt_fingerprint,
             toolset_fingerprint, eligibility_cutoff_json, source_count, canonical_byte_count,
             rendered_request_byte_count, estimated_input_tokens, token_estimator_id,
             context_window_tokens, reserved_output_tokens, utilization_basis_points,
             manifest_sha256, rendered_request_sha256, rendered_request_artifact_id,
             omissions_json, created_at
         ) VALUES (?, ?, ?, 'redaction-target', ?, ?, 1, '{}', 'redaction-v1', 'redaction-v1',
                   ?, ?, '{}', 1, 0, 0, 1, 'redaction-estimator', 100, 1, 200,
                   ?, ?, NULL, '{}', ?)",
    )
    .bind(manifest_id.to_string())
    .bind(work_id.to_string())
    .bind(LogicalInvocationId::generate().to_string())
    .bind(PROVIDER_CANARY)
    .bind(MODEL_CANARY)
    .bind("b".repeat(64))
    .bind("c".repeat(64))
    .bind("d".repeat(64))
    .bind("e".repeat(64))
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO context_manifest_sources (
             context_manifest_id, position, source_kind, event_id, artifact_id,
             source_record_kind, source_record_id, model_role, item_class,
             source_content_sha256, rendered_byte_contribution, transform_json
         ) VALUES (?, 1, 'observed_tool_result', NULL, ?, NULL, NULL, 'tool',
                   'tool_result', ?, 0, ?)",
    )
    .bind(manifest_id.to_string())
    .bind(artifact_id.to_string())
    .bind("f".repeat(64))
    .bind(
        json!({
            "arguments": TOOL_ARGUMENTS_CANARY,
            "result": TOOL_RESULT_CANARY,
        })
        .to_string(),
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    vec![
        PROVIDER_CANARY.to_owned(),
        MODEL_CANARY.to_owned(),
        TOOL_ARGUMENTS_CANARY.to_owned(),
        TOOL_RESULT_CANARY.to_owned(),
        ARTIFACT_METADATA_CANARY.to_owned(),
        artifact_storage_key,
    ]
}

async fn receive_json<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<WebSocketMessage, WebSocketError>> + Unpin,
{
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let WebSocketMessage::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

#[tokio::test]
async fn real_http_health_auth_message_replay_conflict_limits_and_redaction() {
    let harness = Harness::new().await;
    let live = harness
        .request(
            "GET",
            "/health/live",
            &[("X-Request-Id", "untrusted-request-id".to_owned())],
            "",
        )
        .await;
    assert_eq!(live.status, 200);
    assert_eq!(
        live.json(),
        json!({"protocol_version": 1, "status": "live"})
    );
    assert_eq!(live.headers["cache-control"], "no-store");
    assert_eq!(live.headers["x-content-type-options"], "nosniff");
    let request_id = &live.headers["x-request-id"];
    assert_ne!(request_id, "untrusted-request-id");
    assert_eq!(
        uuid::Uuid::parse_str(request_id).unwrap().get_version_num(),
        7
    );

    let ready = harness.request("GET", "/health/ready", &[], "").await;
    assert_eq!(ready.status, 503);
    assert_eq!(ready.json()["status"], "live_unready");

    let client_message_id = client_id();
    let body = message_body(client_message_id, MESSAGE_BODY_CANARY);
    let unavailable = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_message_id.to_string()),
            &body,
        )
        .await;
    assert_eq!(unavailable.status, 503);
    assert_eq!(unavailable.json()["error"]["retryable"], true);
    assert_eq!(journal_count(&harness.store).await, 2);

    for authorization in [None, Some("Bearer bad"), Some("Basic efef")] {
        let headers = authorization
            .map(|value| vec![("Authorization", value.to_owned())])
            .unwrap_or_default();
        let response = harness.request("GET", "/v1/bootstrap", &headers, "").await;
        assert_eq!(response.status, 401);
        assert_eq!(response.json()["error"]["code"], "authentication_failed");
        assert_eq!(response.headers["www-authenticate"], "Bearer");
    }
    let wrong_method_path = format!(
        "/v1/conversations/{}/messages",
        harness.identity.conversation_id
    );
    let unauthenticated_method = harness.request("GET", &wrong_method_path, &[], "").await;
    assert_eq!(unauthenticated_method.status, 401);
    let authenticated_method = harness
        .request(
            "GET",
            &wrong_method_path,
            &[("Authorization", format!("Bearer {}", harness.token))],
            "",
        )
        .await;
    assert_eq!(authenticated_method.status, 405);
    assert_eq!(
        authenticated_method.json()["error"]["code"],
        "method_not_allowed"
    );

    harness.health.mark_ready().unwrap();
    let response = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_message_id.to_string()),
            &body,
        )
        .await;
    assert_eq!(response.status, 202);
    let fresh = response.json();
    assert_eq!(fresh["protocol_version"], 1);
    assert_eq!(fresh["duplicate"], false);
    assert!(fresh.get("request_hash").is_none());
    assert!(
        !serde_json::to_string(&fresh)
            .unwrap()
            .contains(MESSAGE_BODY_CANARY)
    );
    let work_id = WorkId::parse_canonical(fresh["work_id"].as_str().unwrap()).unwrap();
    let internal_canaries =
        insert_dynamic_redaction_canaries(&harness.store, harness.identity, work_id).await;

    let replay = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_message_id.to_string()),
            &body,
        )
        .await;
    assert_eq!(replay.status, 202);
    let duplicate = replay.json();
    assert_eq!(duplicate["duplicate"], true);
    for field in ["message_id", "work_id", "committed_cursor"] {
        assert_eq!(fresh[field], duplicate[field]);
    }

    let conflict = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_message_id.to_string()),
            &message_body(client_message_id, "different"),
        )
        .await;
    assert_eq!(conflict.status, 409);
    assert_eq!(conflict.json()["error"]["code"], "idempotency_conflict");
    assert!(
        !String::from_utf8(conflict.body.clone())
            .unwrap()
            .contains(MESSAGE_BODY_CANARY)
    );

    let mismatch = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_id().to_string()),
            &body,
        )
        .await;
    assert_eq!(mismatch.status, 400);
    let unknown = body.trim_end_matches('}').to_owned() + ",\"unknown\":true}";
    let rejected = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_id().to_string()),
            &unknown,
        )
        .await;
    assert_eq!(rejected.status, 400);
    assert!(
        !String::from_utf8(rejected.body.clone())
            .unwrap()
            .contains(MESSAGE_BODY_CANARY)
    );

    let oversized = "x".repeat(crate::protocol::MESSAGE_BODY_LIMIT + 1);
    let too_large = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&client_id().to_string()),
            &oversized,
        )
        .await;
    assert_eq!(too_large.status, 413);

    let bootstrap = harness
        .request(
            "GET",
            "/v1/bootstrap",
            &[("Authorization", format!("Bearer {}", harness.token))],
            "",
        )
        .await;
    assert_eq!(bootstrap.status, 200);
    let bootstrap_text = String::from_utf8(bootstrap.body).unwrap();
    assert!(bootstrap_text.contains(MESSAGE_BODY_CANARY));
    for canary in [
        TOKEN,
        &harness.device_id.to_string(),
        "/secret/workspace",
        "/secret/resolved/workspace",
        "/bin/secret-shell",
        "stage11-secret-architecture",
    ] {
        assert!(!bootstrap_text.contains(canary), "leaked {canary}");
    }
    for canary in &internal_canaries {
        assert!(!bootstrap_text.contains(canary), "leaked {canary}");
    }

    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(ReplayCursor::START))
        .await
        .unwrap();
    let mut public_frames = String::new();
    loop {
        let frame = receive_json(&mut socket).await;
        public_frames.push_str(&frame.to_string());
        if frame["event_type"] == "sync.complete" {
            break;
        }
    }
    assert!(public_frames.contains(MESSAGE_BODY_CANARY));
    for canary in [
        TOKEN,
        &harness.device_id.to_string(),
        "/secret/workspace",
        "/secret/resolved/workspace",
        "/bin/secret-shell",
        "stage11-secret-architecture",
    ] {
        assert!(!public_frames.contains(canary), "leaked {canary}");
    }
    for canary in &internal_canaries {
        assert!(!public_frames.contains(canary), "leaked {canary}");
    }
    socket.close(None).await.unwrap();
    harness
        .health
        .mark_fatal(crate::bootstrap::health::FatalReasonCode::Internal)
        .unwrap();
    let fatal_live = harness.request("GET", "/health/live", &[], "").await;
    assert_eq!(fatal_live.status, 503);
    assert_eq!(fatal_live.json()["status"], "fatal");
    let fatal_ready = harness.request("GET", "/health/ready", &[], "").await;
    assert_eq!(fatal_ready.status, 503);
    assert_eq!(fatal_ready.json()["status"], "fatal");
    harness.close().await;
}

#[tokio::test]
async fn real_http_cancellation_status_replay_not_found_and_content_type() {
    let harness = Harness::new().await;
    let message_id = client_id();
    let work_id = accept_without_hint(&harness, message_id, "cancel me").await;
    let cancellation_command_id = command_id();
    let cancel_body = json!({
        "protocol_version": 1,
        "client_command_id": cancellation_command_id,
    })
    .to_string();
    let cancel = harness
        .authenticated_json(
            "POST",
            &format!("/v1/work-items/{work_id}/cancel"),
            Some(&cancellation_command_id.to_string()),
            &cancel_body,
        )
        .await;
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.json()["work_state"], "cancelled");
    assert_eq!(cancel.json()["cleanup_pending"], false);
    let replay = harness
        .authenticated_json(
            "POST",
            &format!("/v1/work-items/{work_id}/cancel"),
            Some(&cancellation_command_id.to_string()),
            &cancel_body,
        )
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.json()["duplicate"], true);

    let noop_id = command_id();
    let noop = harness
        .authenticated_json(
            "POST",
            &format!("/v1/work-items/{work_id}/cancel"),
            Some(&noop_id.to_string()),
            &json!({"protocol_version":1,"client_command_id":noop_id}).to_string(),
        )
        .await;
    assert_eq!(noop.status, 200);
    assert_eq!(noop.json()["duplicate"], false);
    assert_eq!(noop.json()["work_state"], "cancelled");

    let conflict = harness
        .authenticated_json(
            "POST",
            &format!("/v1/work-items/{}/cancel", WorkId::generate()),
            Some(&cancellation_command_id.to_string()),
            &cancel_body,
        )
        .await;
    assert_eq!(conflict.status, 409);

    let missing_id = command_id();
    let missing = harness
        .authenticated_json(
            "POST",
            &format!("/v1/work-items/{}/cancel", WorkId::generate()),
            Some(&missing_id.to_string()),
            &json!({"protocol_version":1,"client_command_id":missing_id}).to_string(),
        )
        .await;
    assert_eq!(missing.status, 404);

    let no_content_type = harness
        .request(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            &[("Authorization", format!("Bearer {}", harness.token))],
            "{}",
        )
        .await;
    assert_eq!(no_content_type.status, 415);
    harness.close().await;
}

#[tokio::test]
async fn route_methods_and_authenticated_v1_fallback_boundary_are_real() {
    let harness = Harness::new().await;
    assert_eq!(
        harness.request("GET", "/health/live", &[], "").await.status,
        200
    );
    assert_eq!(
        harness
            .request("POST", "/health/live", &[], "")
            .await
            .status,
        405
    );
    let authorization = [("Authorization", format!("Bearer {}", harness.token))];
    for (method, path) in [
        ("POST", "/v1/bootstrap".to_owned()),
        (
            "GET",
            format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
        ),
        (
            "GET",
            format!("/v1/work-items/{}/cancel", WorkId::generate()),
        ),
        ("POST", "/v1/events?after=0".to_owned()),
    ] {
        assert_eq!(
            harness
                .request(method, &path, &authorization, "")
                .await
                .status,
            405,
            "wrong method was not rejected for {path}"
        );
    }
    assert_eq!(
        harness
            .request("GET", "/v1/not-a-route", &authorization, "")
            .await
            .status,
        404
    );
    assert_eq!(
        harness
            .request("GET", "/v1/not-a-route", &[], "")
            .await
            .status,
        401
    );
    assert_eq!(
        harness.request("GET", "/not-a-route", &[], "").await.status,
        404
    );
    harness.close().await;
}

#[tokio::test]
async fn real_http_lost_postcommit_response_retries_exactly_once_over_new_connection() {
    let gate = Arc::new(TestPostCommitGate::armed());
    let harness = Harness::new_with_post_commit_gate(Arc::clone(&gate)).await;
    harness.health.mark_ready().unwrap();
    let mut committed_hints = harness.cursors.subscribe();
    let id = client_id();
    let body = message_body(id, "lost-response-message-canary");
    let before = journal_count(&harness.store).await;
    let path = format!(
        "/v1/conversations/{}/messages",
        harness.identity.conversation_id
    );
    let mut first_connection = tokio::net::TcpStream::connect(&harness.authority)
        .await
        .unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nIdempotency-Key: {id}\r\nContent-Length: {}\r\n\r\n{body}",
        harness.authority,
        harness.token,
        body.len(),
    );
    first_connection
        .write_all(request.as_bytes())
        .await
        .unwrap();
    gate.wait_until_reached().await;
    first_connection.shutdown().await.unwrap();
    drop(first_connection);
    gate.release();
    let first_hint = committed_hints.recv().await.unwrap();

    let retry = harness
        .authenticated_json("POST", &path, Some(&id.to_string()), &body)
        .await;
    assert_eq!(retry.status, 202);
    let retry_json = retry.json();
    assert_eq!(retry_json["duplicate"], true);
    assert_eq!(retry_json["committed_cursor"], first_hint.get());
    assert!(matches!(
        committed_hints.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    let mut connection = harness.store.runtime.acquire().await.unwrap();
    let message_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE client_message_id = ?")
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    let command_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM client_commands WHERE idempotency_key = ?")
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    let persisted_response: String =
        sqlx::query_scalar("SELECT response_json FROM client_commands WHERE idempotency_key = ?")
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    drop(connection);
    let persisted: Value = serde_json::from_str(&persisted_response).unwrap();
    assert_eq!(message_count, 1);
    assert_eq!(command_count, 1);
    assert_eq!(journal_count(&harness.store).await, before + 2);
    for field in ["message_id", "work_id", "committed_cursor"] {
        assert_eq!(retry_json[field], persisted[field]);
    }
    harness.close().await;
}

#[tokio::test]
async fn real_websocket_replay_sync_live_fallback_policy_reconnect_and_shutdown() {
    let mut harness = Harness::new().await;
    harness.health.mark_ready().unwrap();
    let first_id = client_id();
    let first = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&first_id.to_string()),
            &message_body(first_id, "first"),
        )
        .await;
    assert_eq!(first.status, 202);

    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(ReplayCursor::START))
        .await
        .unwrap();
    let mut durable = Vec::new();
    let through = loop {
        let value = receive_json(&mut socket).await;
        if value["event_type"] == "sync.complete" {
            break value["through_cursor"].as_u64().unwrap();
        }
        assert_eq!(value["delivery_kind"], "durable");
        durable.push(value);
    };
    assert_eq!(
        durable
            .iter()
            .map(|event| event["event_type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["message.accepted", "work.queued"]
    );
    assert!(
        durable
            .windows(2)
            .all(|pair| pair[0]["cursor"].as_u64() < pair[1]["cursor"].as_u64())
    );

    let second_id = client_id();
    let second = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&second_id.to_string()),
            &message_body(second_id, "live"),
        )
        .await;
    assert_eq!(second.status, 202);
    let live = receive_json(&mut socket).await;
    assert_eq!(live["event_type"], "message.accepted");
    assert!(live["cursor"].as_u64().unwrap() > through);
    let last_applied = live["cursor"].as_u64().unwrap();

    let before_mutation = journal_count(&harness.store).await;
    socket
        .send(WebSocketMessage::Text("mutate".into()))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if matches!(frame, WebSocketMessage::Close(_)) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Policy)
    );
    assert_eq!(journal_count(&harness.store).await, before_mutation);

    let (mut reconnect, _) = tokio_tungstenite::connect_async(
        harness.ws_request(ReplayCursor::try_new(last_applied).unwrap()),
    )
    .await
    .unwrap();
    let mut saw_second_work = false;
    loop {
        let value = receive_json(&mut reconnect).await;
        if value["event_type"] == "sync.complete" {
            break;
        }
        saw_second_work |= value["event_type"] == "work.queued";
    }
    assert!(saw_second_work);

    let lag_id = client_id();
    let _ = accept_without_hint(&harness, lag_id, "broadcast lag").await;
    let committed = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap()
        .as_journal_offset()
        .unwrap();
    for _ in 0..=crate::protocol::CURSOR_BROADCAST_CAPACITY {
        harness.cursors.publish(committed);
    }
    let lagged = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = receive_json(&mut reconnect).await;
            if value["event_type"] == "message.accepted"
                && value["payload"]["client_message_id"] == lag_id.to_string()
            {
                break value;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(lagged["payload"]["content"][0]["text"], "broadcast lag");

    let fallback_id = client_id();
    let _ = accept_without_hint(&harness, fallback_id, "lost notification").await;
    let fallback = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = receive_json(&mut reconnect).await;
            if value["event_type"] == "message.accepted"
                && value["payload"]["client_message_id"] == fallback_id.to_string()
            {
                break value;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fallback["payload"]["content"][0]["text"],
        "lost notification"
    );

    harness.stop_server().await;
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = reconnect.next().await.unwrap().unwrap();
            if matches!(frame, WebSocketMessage::Close(_)) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Away));
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn websocket_auth_future_cursor_and_revocation_fail_before_upgrade() {
    let harness = Harness::new().await;
    let missing = format!("ws://{}/v1/events?after=0", harness.authority);
    let error = tokio_tungstenite::connect_async(missing).await.unwrap_err();
    assert!(matches!(error, WebSocketError::Http(response) if response.status().as_u16() == 401));

    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let future = ReplayCursor::try_new(head.get() + 1).unwrap();
    let error = tokio_tungstenite::connect_async(harness.ws_request(future))
        .await
        .unwrap_err();
    assert!(matches!(error, WebSocketError::Http(response) if response.status().as_u16() == 400));

    DeviceProvisioningService::new(harness.store.as_ref())
        .revoke(harness.device_id, at("2020-01-01T03:00:02.000000Z"))
        .await
        .unwrap();
    let error = tokio_tungstenite::connect_async(harness.ws_request(ReplayCursor::START))
        .await
        .unwrap_err();
    assert!(matches!(error, WebSocketError::Http(response) if response.status().as_u16() == 401));
    harness.close().await;
}

#[tokio::test]
async fn websocket_slow_consumer_closes_1013_without_durable_change_and_reconnect_recovers() {
    let stall = Arc::new(AtomicBool::new(true));
    let harness = Harness::new_with_stall(Some(Arc::clone(&stall))).await;
    let id = client_id();
    let _ = accept_without_hint(&harness, id, "backpressure").await;
    let before = journal_count(&harness.store).await;

    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(ReplayCursor::START))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if matches!(frame, WebSocketMessage::Close(_)) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Again)
    );
    assert_eq!(journal_count(&harness.store).await, before);

    stall.store(false, Ordering::Release);
    let (mut reconnect, _) =
        tokio_tungstenite::connect_async(harness.ws_request(ReplayCursor::START))
            .await
            .unwrap();
    let mut recovered = false;
    loop {
        let value = receive_json(&mut reconnect).await;
        if value["event_type"] == "sync.complete" {
            break;
        }
        recovered |= value["event_type"] == "message.accepted"
            && value["payload"]["client_message_id"] == id.to_string();
    }
    assert!(recovered);
    reconnect.close(None).await.unwrap();
    harness.close().await;
}

#[tokio::test]
async fn websocket_connection_limit_rejects_thirty_third_upgrade_retryably() {
    let gate = Arc::new(TestUpgradeGate::held());
    let harness = Harness::new_with_upgrade_gate(Arc::clone(&gate)).await;
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let mut sockets = Vec::new();
    for _ in 0..crate::protocol::WEBSOCKET_CONNECTION_LIMIT {
        let (socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
            .await
            .unwrap();
        sockets.push(socket);
    }
    gate.wait_for_entries(crate::protocol::WEBSOCKET_CONNECTION_LIMIT)
        .await;
    assert_eq!(
        harness.connections.pending(),
        crate::protocol::WEBSOCKET_CONNECTION_LIMIT
    );
    assert_eq!(harness.connections.active(), 0);
    let error = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap_err();
    assert!(matches!(error, WebSocketError::Http(response) if response.status().as_u16() == 503));
    gate.release();
    for mut socket in sockets {
        let sync = receive_json(&mut socket).await;
        assert_eq!(sync["event_type"], "sync.complete");
        socket.close(None).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), harness.connections.wait_empty())
        .await
        .unwrap();
    assert_eq!(
        harness.connections.observed_callbacks(),
        crate::protocol::WEBSOCKET_CONNECTION_LIMIT
    );
    assert_eq!(
        harness.connections.observed_completions(),
        crate::protocol::WEBSOCKET_CONNECTION_LIMIT
    );
    harness.close().await;
}

#[tokio::test]
async fn shutdown_waits_for_pending_upgrade_then_initial_replay_observes_latched_shutdown() {
    let gate = Arc::new(TestUpgradeGate::held());
    let mut harness = Harness::new_with_upgrade_gate(Arc::clone(&gate)).await;
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap();
    gate.wait_for_entries(1).await;
    assert_eq!(harness.connections.pending(), 1);
    assert_eq!(harness.connections.active(), 0);

    harness
        .shutdown_authority
        .request_controlled_shutdown()
        .await
        .unwrap();
    let server = harness.server.take().unwrap();
    server.stop_accepting();
    server.close_websockets();
    let mut joining = tokio::spawn(server.join());
    tokio::task::yield_now().await;
    assert!(!joining.is_finished());
    assert_eq!(harness.connections.owned(), 1);

    gate.release();
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if matches!(frame, WebSocketMessage::Close(_)) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Away));
    (&mut joining).await.unwrap().unwrap();
    assert_eq!(harness.connections.owned(), 0);
    assert_eq!(harness.connections.observed_callbacks(), 1);
    assert_eq!(harness.connections.observed_completions(), 1);
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn stage11_second_repair_deadline_keeps_pending_connection_owned_until_callback_terminal_record_is_consumed()
 {
    let gate = Arc::new(TestUpgradeGate::held_after_cancellation());
    let mut harness = Harness::new_with_upgrade_gate(Arc::clone(&gate)).await;
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let (socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap();
    gate.wait_for_entries(1).await;
    assert_eq!(harness.connections.owned(), 1);
    assert_eq!(harness.connections.observed_completions(), 0);

    harness
        .shutdown_authority
        .request_controlled_shutdown()
        .await
        .unwrap();
    let server = harness.server.take().unwrap();
    server.stop_accepting();
    server.close_websockets();
    let mut joining =
        tokio::spawn(server.join_before(tokio::time::Instant::now() + Duration::from_millis(25)));
    gate.wait_for_cancellation().await;

    assert!(!joining.is_finished());
    assert_eq!(harness.connections.owned(), 1);
    assert_eq!(harness.connections.observed_completions(), 0);

    gate.release_cancellation();
    let error = (&mut joining).await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::ShutdownDeadline);
    assert_eq!(harness.connections.owned(), 0);
    assert_eq!(harness.connections.observed_callbacks(), 1);
    assert_eq!(harness.connections.observed_completions(), 1);
    drop(socket);
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn upgrade_callback_panic_is_observed_and_isolated() {
    let gate = Arc::new(TestUpgradeGate::panic_once());
    let harness = Harness::new_with_upgrade_gate(Arc::clone(&gate)).await;
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let (_socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap();
    gate.wait_for_entries(1).await;
    tokio::time::timeout(Duration::from_secs(3), harness.connections.wait_empty())
        .await
        .unwrap();
    assert!(harness.connections.observed_panics() >= 1);
    assert_eq!(harness.connections.observed_completions(), 1);
    assert_ne!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    harness.close().await;
}

#[tokio::test]
async fn binary_websocket_application_frame_closes_1008_without_mutation() {
    let harness = Harness::new().await;
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let before = journal_count(&harness.store).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut socket).await["event_type"],
        "sync.complete"
    );
    socket
        .send(WebSocketMessage::Binary(vec![0, 1, 2, 3].into()))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if matches!(frame, WebSocketMessage::Close(_)) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Policy)
    );
    tokio::time::timeout(Duration::from_secs(3), harness.connections.wait_empty())
        .await
        .unwrap();
    assert_eq!(journal_count(&harness.store).await, before);
    assert_ne!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    harness.close().await;
}

#[tokio::test]
async fn shared_server_failure_supervisor_triggers_existing_shutdown_and_preserves_cause() {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    let served = harness.request("GET", "/health/live", &[], "").await;
    assert_eq!(served.status, 200);
    let head = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(harness.ws_request(head))
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut socket).await["event_type"],
        "sync.complete"
    );
    let server = harness.server.take().unwrap();
    server.inject_shared_failure();
    let error = server.join().await.unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::InjectedSharedFailure);
    assert_eq!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    assert!(*harness.fatal_receiver.borrow());
    assert!(!harness.admission.is_accepting());
    assert_eq!(harness.connections.owned(), 0);
    assert!(
        harness
            .shutdown_controller
            .as_ref()
            .unwrap()
            .monotonic_deadline()
            .await
            .is_ok()
    );
    let close = socket.next().await.unwrap().unwrap();
    assert!(matches!(close, WebSocketMessage::Close(Some(frame)) if frame.code == CloseCode::Away));
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn stage11_second_repair_server_return_after_accept_stop_but_before_stage10_latch_is_unexpected_and_fatal()
 {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    assert_eq!(
        harness.request("GET", "/health/live", &[], "").await.status,
        200
    );
    let server = harness.server.take().unwrap();
    server.stop_accepting();
    assert!(
        !harness
            .shutdown_controller
            .as_ref()
            .unwrap()
            .shutdown_is_requested()
    );
    server.inject_server_return();
    let error = server.join().await.unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::UnexpectedExit);
    assert_eq!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    assert!(*harness.fatal_receiver.borrow());
    assert!(
        harness
            .shutdown_controller
            .as_ref()
            .unwrap()
            .shutdown_is_requested()
    );
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn stage11_second_repair_server_return_after_stage10_latch_is_expected_and_not_fatal() {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    harness
        .shutdown_controller
        .as_ref()
        .unwrap()
        .request()
        .await
        .unwrap();
    let server = harness.server.take().unwrap();
    server.stop_accepting();
    server.inject_server_return();
    server.join().await.unwrap();
    assert_ne!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    assert!(!*harness.fatal_receiver.borrow());
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn stage11_second_repair_primary_server_failure_precedes_connection_cleanup_failure() {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    let server = harness.server.take().unwrap();
    let secondary = server.secondary_cleanup_failures();
    server.inject_connection_supervisor_panic();
    server.inject_shared_failure();
    let error = server.join().await.unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::InjectedSharedFailure);
    assert_eq!(secondary.load(Ordering::Acquire), 1);
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn stage11_second_repair_connection_cleanup_failure_surfaces_when_server_completion_is_graceful()
 {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    harness
        .shutdown_controller
        .as_ref()
        .unwrap()
        .request()
        .await
        .unwrap();
    let server = harness.server.take().unwrap();
    server.inject_connection_supervisor_panic();
    server.stop_accepting();
    server.inject_server_return();
    let error = server.join().await.unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::ConnectionSupervisorTask);
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn shared_server_child_panic_is_observed_with_join_cause() {
    let mut harness = Harness::new_with_controlled_shutdown().await;
    assert_eq!(
        harness.request("GET", "/health/live", &[], "").await.status,
        200
    );
    let server = harness.server.take().unwrap();
    server.inject_server_panic();
    let error = server.join().await.unwrap_err();
    assert_eq!(error.kind(), ServerErrorKind::ExecutionTask);
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(
        harness.health.snapshot().state(),
        crate::bootstrap::health::HealthState::Fatal
    );
    assert!(!harness.admission.is_accepting());
    assert_eq!(harness.connections.owned(), 0);
    harness
        .shutdown_controller
        .take()
        .unwrap()
        .finish()
        .await
        .unwrap();
    harness.guard.runtime().close().await;
}

#[tokio::test]
async fn bootstrap_first_head_barrier_defines_both_sides_of_concurrent_commit() {
    let harness = Harness::new().await;

    let before_head = Arc::new(Stage11SnapshotBarrier::new());
    harness
        .store
        .set_stage11_snapshot_hook(Some(Stage11SnapshotTestHook::new(
            Stage11SnapshotPoint::BeforeHeadRead,
            Arc::clone(&before_head),
        )));
    let snapshot_store = Arc::clone(&harness.store);
    let including = tokio::spawn(async move {
        PublicStateService::new(snapshot_store.as_ref())
            .bootstrap()
            .await
            .unwrap()
    });
    before_head.wait_until_reached().await;
    let included_id = client_id();
    let _ = accept_without_hint(&harness, included_id, "committed before head").await;
    before_head.release();
    let including = including.await.unwrap();
    assert!(
        including
            .messages
            .iter()
            .any(|message| message.client_message_id == Some(included_id))
    );

    let after_head = Arc::new(Stage11SnapshotBarrier::new());
    harness
        .store
        .set_stage11_snapshot_hook(Some(Stage11SnapshotTestHook::new(
            Stage11SnapshotPoint::AfterHeadRead,
            Arc::clone(&after_head),
        )));
    let snapshot_store = Arc::clone(&harness.store);
    let excluding = tokio::spawn(async move {
        PublicStateService::new(snapshot_store.as_ref())
            .bootstrap()
            .await
            .unwrap()
    });
    after_head.wait_until_reached().await;
    let excluded_id = client_id();
    let _ = accept_without_hint(&harness, excluded_id, "committed after head").await;
    after_head.release();
    let excluding = excluding.await.unwrap();
    harness.store.set_stage11_snapshot_hook(None);
    assert!(
        excluding
            .messages
            .iter()
            .all(|message| message.client_message_id != Some(excluded_id))
    );
    let through = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let replay = PublicStateService::new(harness.store.as_ref())
        .replay_page(
            ReplayCursor::from_journal_offset(excluding.snapshot_cursor),
            through,
        )
        .await
        .unwrap();
    assert!(replay.events.iter().any(|event| {
        event.event_type == "message.accepted"
            && event.payload["client_message_id"] == excluded_id.to_string()
    }));
    harness.close().await;
}

#[tokio::test]
async fn replay_over_three_pages_crosses_mixed_and_all_filtered_underlying_rows() {
    let harness = Harness::new().await;
    for index in 0..63 {
        let _ = accept_without_hint(&harness, client_id(), &format!("before-filter-{index}")).await;
    }
    insert_filtered_initialization_clones(&harness.store, 140).await;
    for index in 0..67 {
        let _ = accept_without_hint(&harness, client_id(), &format!("after-filter-{index}")).await;
    }
    let through = PublicStateService::new(harness.store.as_ref())
        .current_high_water()
        .await
        .unwrap();
    let post_boundary_id = client_id();
    let _ = accept_without_hint(&harness, post_boundary_id, "after fixed boundary").await;

    let mut cursor = ReplayCursor::START;
    let mut public = Vec::new();
    let mut pages = 0;
    let mut saw_all_filtered_page = false;
    while cursor < through {
        let raw = harness
            .store
            .list_replay_page_inner(ListPublicJournalRequest {
                after: cursor.as_journal_offset(),
                through: through.as_journal_offset().unwrap(),
                limit: crate::protocol::REPLAY_PAGE_ROWS,
            })
            .await
            .unwrap();
        assert!(raw.candidates.len() <= crate::protocol::REPLAY_PAGE_ROWS as usize);
        let page = PublicStateService::new(harness.store.as_ref())
            .replay_page(cursor, through)
            .await
            .unwrap();
        pages += 1;
        assert!(page.scanned_through > cursor);
        assert_eq!(
            page.scanned_through,
            ReplayCursor::from_journal_offset(raw.scanned_through)
        );
        saw_all_filtered_page |= page.events.is_empty();
        public.extend(page.events);
        cursor = page.scanned_through;
    }
    assert!(pages >= 3);
    assert!(saw_all_filtered_page);
    assert_eq!(cursor, through);
    assert_eq!(public.len(), 260);
    let unique_ids = public
        .iter()
        .map(|event| event.event_id)
        .collect::<HashSet<_>>();
    assert_eq!(unique_ids.len(), public.len());
    assert!(
        public
            .windows(2)
            .all(|pair| pair[0].cursor < pair[1].cursor)
    );
    assert!(
        public
            .iter()
            .all(|event| { event.payload["client_message_id"] != post_boundary_id.to_string() })
    );
    harness.close().await;
}

#[tokio::test]
async fn atomic_bootstrap_releases_read_transaction_and_replay_pages_cross_filtered_gaps() {
    let harness = Harness::new().await;
    let before = PublicStateService::new(harness.store.as_ref())
        .bootstrap()
        .await
        .unwrap();
    assert!(before.messages.is_empty());

    for index in 0..70 {
        let id = client_id();
        CommandService::new(harness.store.as_ref())
            .accept_message(
                AuthenticatedDevice::new(harness.device_id),
                AcceptMessageCommand {
                    idempotency_key: IdempotencyKey::for_message(id),
                    client_message_id: id,
                    conversation_id: harness.identity.conversation_id,
                    content: MessageContent::try_new(vec![
                        ContentBlock::text(format!("page-{index}")).unwrap(),
                    ])
                    .unwrap(),
                    accepted_at: at("2020-01-01T03:00:03.000000Z"),
                },
            )
            .await
            .unwrap();
    }
    let checkpoint = harness.guard.runtime().checkpoint_passive().await.unwrap();
    assert_eq!(
        checkpoint.busy(),
        0,
        "snapshot transaction must be released"
    );

    let after = PublicStateService::new(harness.store.as_ref())
        .bootstrap()
        .await
        .unwrap();
    assert_eq!(after.messages.len(), 70);
    assert!(after.snapshot_cursor > before.snapshot_cursor);
    let mut cursor = ReplayCursor::from_journal_offset(before.snapshot_cursor);
    let through = ReplayCursor::from_journal_offset(after.snapshot_cursor);
    let mut public = Vec::new();
    let mut pages = 0;
    while cursor < through {
        let page = PublicStateService::new(harness.store.as_ref())
            .replay_page(cursor, through)
            .await
            .unwrap();
        pages += 1;
        assert!(page.scanned_through > cursor);
        public.extend(page.events);
        cursor = page.scanned_through;
    }
    assert!(pages >= 2);
    assert_eq!(public.len(), 140);
    assert!(
        public
            .windows(2)
            .all(|pair| pair[0].cursor < pair[1].cursor)
    );
    assert_eq!(cursor, through);
    harness.close().await;
}

#[tokio::test]
async fn mutation_quiescence_commits_inflight_before_runtime_stopping_and_rejects_late_command() {
    let harness = Harness::new().await;
    let runtime_instance_id = RuntimeInstanceId::generate();
    let clock = TestClock::new(at(T0).to_offset_datetime(), Duration::ZERO);
    let runtime = bootstrap_runtime(
        harness.store.as_ref(),
        runtime_evidence(harness.identity, runtime_instance_id),
        0,
        &clock,
    )
    .await
    .unwrap();

    let permit = harness.admission.admit().await.unwrap();
    let closing = harness.admission.clone();
    let mut quiesced = tokio::spawn(async move {
        closing.close_and_wait().await;
    });
    tokio::task::yield_now().await;
    assert!(!quiesced.is_finished());

    let inflight_id = client_id();
    let _ = accept_without_hint(&harness, inflight_id, "inflight before stopping").await;
    drop(permit);
    (&mut quiesced).await.unwrap();

    let stopping = harness
        .store
        .begin_runtime_stopping(BeginRuntimeStoppingRequest {
            event: RuntimeStoppingV1 {
                runtime_instance_id,
                shutdown_requested_at: at("2020-01-01T03:00:02.000000Z"),
                shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
                grace_deadline: at("2020-01-01T03:00:12.000000Z"),
                active_work_count: 0,
                active_task_count: 0,
            },
            event_id: JournalEventId::generate(),
            correlation_id: runtime.correlation_id,
        })
        .await
        .unwrap();
    assert!(stopping.began);

    harness.health.mark_ready().unwrap();
    let late_id = client_id();
    let late = harness
        .authenticated_json(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                harness.identity.conversation_id
            ),
            Some(&late_id.to_string()),
            &message_body(late_id, "must not commit"),
        )
        .await;
    assert_eq!(late.status, 503);

    let mut connection = harness.store.runtime.acquire().await.unwrap();
    let stopping_cursor: i64 = sqlx::query_scalar(
        "SELECT journal_offset FROM journal_events WHERE event_type = 'runtime.stopping'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let commands_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM journal_events WHERE journal_offset > ? AND event_type IN \
         ('message.accepted','work.queued','work.cancel_requested','work.cancelled')",
    )
    .bind(stopping_cursor)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(commands_after, 0);
    drop(connection);
    harness.close().await;
}

#[test]
fn public_protocol_version_constant_is_one() {
    assert_eq!(serde_json::to_value(ProtocolVersion).unwrap(), 1);
}
