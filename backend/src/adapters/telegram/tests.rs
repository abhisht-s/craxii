use std::collections::VecDeque;
use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::kill;
use nix::unistd::Pid;
use serde_json::{Value, json};
use sqlx::Row as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::fmt::writer::MakeWriter;

use crate::adapters::artifacts::LocalArtifactStore;
use crate::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
use crate::adapters::sqlite::{SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore};
use crate::application::channel_ingress::ChannelIngressService;
use crate::application::channel_topology::ChannelTopologyService;
use crate::application::command_service::CommandPostCommit;
use crate::application::delivery_planner::ChannelDeliveryProfileRegistry;
use crate::application::delivery_worker::{
    DeliveryJitterSource, DeliveryNotifier, start_delivery_worker,
};
use crate::application::runtime::bootstrap_runtime;
use crate::application::scheduler::{
    SchedulerReadiness, SchedulerStart, WorkCancellation, WorkRunner, WorkRunnerExit,
    WorkRunnerFuture, WorkRunnerStartError, start_scheduler,
};
use crate::application::transport::MutationAdmission;
use crate::bootstrap::credential::{CredentialRef, CredentialSourceConfig, load_credentials};
use crate::bootstrap::health::{Health, HealthState};
use crate::bootstrap::secret::SecretString;
use crate::domain::{
    ChannelAccountId, ChannelDispatchResult, ChannelProviderId, CorrelationId, CraxiiId,
    DeliveryFailureClass, DiagnosticPid, ExecutionId, ExternalAccountId, ExternalConversationId,
    ExternalSubjectId, ExternalThreadId, GitRevision, JournalEventId, LinuxBootId,
    LogicalPathReference, MonotonicDuration, OperationId, OutboundDeliveryAttemptId,
    OutboundDeliveryId, OutboundDeliveryState, PackageVersion, PreparedChannelDispatch,
    PrivilegeMode, RuntimeInstanceId, RuntimeStartEvidence, RuntimeStartEvidenceInput,
    SchemaVersion, Sha256Digest, UserId, UtcTimestamp, WorkspaceId, WorkstationGeneration,
    WorkstationId,
};
use crate::ports::artifact_store::ArtifactStore;
use crate::ports::channel_delivery::{
    ChannelDeliveryAdapter, ChannelDeliveryAdapterRegistry, ChannelDeliveryFuture,
};
use crate::ports::channel_ingress::{
    ChannelIngressFuture, ChannelIngressStore, ChannelIngressStoreError,
    ChannelIngressStoreErrorKind, ChannelProviderFuture, ClassifyInboundRequest,
};
use crate::ports::clock::{Clock, MonotonicInstant, TestClock};
use crate::ports::delivery_store::{
    ClaimDeliveryRequest, DeliveryClaim, DeliveryStore, ListDeliverySummariesRequest,
    RecoverDeliveriesRequest,
};
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ClaimedWork, ExecutionCapabilityObservation,
    LoadOrBootstrapIdentityRequest, V0IdentityReference,
};
use crate::ports::workstation::{
    ExecutionCancellationRequest, ExecutionCancellationState, ExecutionCapturePolicy,
    ExecutionCleanupPolicy, ExecutionRequest, ExecutionResult, ExecutionResultKind,
    ExecutionStdinPolicy, HARD_EXECUTION_STREAM_CAPTURE_BYTES, HARD_FILE_READ_MAX_BYTES,
    Workstation,
};
use crate::ports::workstation_preparation::{
    RequiredWorkstationCapability, WorkstationPreparation, WorkstationPreparationRequest,
};

use super::client::TelegramClient;
use super::polling::{TelegramOwnerTopology, TelegramUpdateSink};
use super::wire::RawUpdate;
use super::{
    TELEGRAM_PROVIDER_KEY, TelegramDeliveryAdapter, TelegramInboundProcessor, TelegramJitterSource,
    TelegramPollBatch, TelegramPollerError, TelegramPollerErrorKind, TelegramStartupFailureBudget,
    start_long_polling, startup_probe, telegram_delivery_profile, verify_startup_identity,
};

const TOKEN: &str = "123456:abcdefghijklmnopqrstuvwxyzABCDEFGH";
const T0: &str = "2026-09-16T01:02:03.000000Z";
const T1: &str = "2026-09-16T01:02:04.000000Z";

#[derive(Clone)]
struct ResponseSpec {
    status: u16,
    body: Vec<u8>,
    declared_length: Option<usize>,
}

impl ResponseSpec {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&value).unwrap(),
            declared_length: None,
        }
    }

    fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            declared_length: None,
        }
    }
}

#[derive(Clone)]
struct RecordedRequest {
    path: String,
    body: Vec<u8>,
}

async fn fake_server(
    responses: Vec<ResponseSpec>,
) -> (
    String,
    Arc<Mutex<Vec<RecordedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let output = Arc::clone(&recorded);
    let mut responses = VecDeque::from(responses);
    let join = tokio::spawn(async move {
        while let Some(response) = responses.pop_front() {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let (header_end, content_length) = loop {
                let mut buffer = [0_u8; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let end = end + 4;
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length: ")
                                .or_else(|| line.strip_prefix("Content-Length: "))
                        })
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (end, length);
                }
            };
            while request.len() < header_end + content_length {
                let mut buffer = [0_u8; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let request_line = String::from_utf8_lossy(&request[..header_end])
                .lines()
                .next()
                .unwrap()
                .to_owned();
            let path = request_line.split_whitespace().nth(1).unwrap().to_owned();
            output.lock().unwrap().push(RecordedRequest {
                path,
                body: request[header_end..].to_vec(),
            });
            let phrase = match response.status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                403 => "Forbidden",
                409 => "Conflict",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                503 => "Service Unavailable",
                _ => "Synthetic",
            };
            let declared = response.declared_length.unwrap_or(response.body.len());
            let headers = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.status, phrase, declared
            );
            if stream.write_all(headers.as_bytes()).await.is_err() {
                continue;
            }
            if stream.write_all(&response.body).await.is_err() {
                continue;
            }
            let _ = stream.shutdown().await;
        }
    });
    (format!("http://{address}/"), recorded, join)
}

async fn client_with(
    responses: Vec<ResponseSpec>,
) -> (
    Arc<TelegramClient>,
    Arc<Mutex<Vec<RecordedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let (origin, recorded, join) = fake_server(responses).await;
    let client = TelegramClient::for_test(SecretString::new(TOKEN.to_owned()), &origin).unwrap();
    (Arc::new(client), recorded, join)
}

fn dispatch(account: ChannelAccountId, route: &str, text: &str) -> PreparedChannelDispatch {
    PreparedChannelDispatch {
        outbound_delivery_id: OutboundDeliveryId::generate(),
        outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
        attempt_number: 1,
        channel_account_id: account,
        provider_id: ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY).unwrap(),
        external_conversation_id: ExternalConversationId::try_new(route).unwrap(),
        external_thread_id: None,
        text: text.to_owned(),
        payload_sha256: Sha256Digest::hash_bytes(text.as_bytes()),
        dispatch_material_sha256: Sha256Digest::hash_bytes(b"test dispatch"),
        part_ordinal: 1,
        part_count: 1,
    }
}

fn assert_failure(
    result: &ChannelDispatchResult,
    expected_class: DeliveryFailureClass,
    expected_code: &str,
) {
    let failure = match result {
        ChannelDispatchResult::RetryableFailure { failure, .. }
        | ChannelDispatchResult::PermanentFailure { failure }
        | ChannelDispatchResult::OutcomeUnknown { failure } => failure,
        ChannelDispatchResult::Accepted { .. } => panic!("expected failure"),
    };
    assert_eq!(failure.class(), expected_class);
    assert_eq!(failure.code().unwrap().as_str(), expected_code);
}

#[test]
fn telegram_profile_and_client_diagnostics_are_exact_and_secret_safe() {
    let profile = telegram_delivery_profile();
    assert_eq!(profile.provider_id().as_str(), TELEGRAM_PROVIDER_KEY);
    assert_eq!(profile.max_text_utf8_bytes(), 4096);
    assert_eq!(profile.max_parts(), 64);

    let client =
        TelegramClient::for_test(SecretString::new(TOKEN.to_owned()), "http://127.0.0.1:1/")
            .unwrap();
    let rendered = format!("{client:?}");
    assert!(!rendered.contains(TOKEN));
    assert!(!rendered.contains("127.0.0.1"));
    for bad in [
        "",
        "1:short",
        "01:abcdefghijklmnopqrstuvwxyzABCDEFGH",
        "1:bad token value xxxxxxxxx",
    ] {
        let error =
            TelegramClient::for_test(SecretString::new(bad.to_owned()), "http://127.0.0.1:1/")
                .unwrap_err();
        let diagnostic = format!("{error}{error:?}");
        if !bad.is_empty() {
            assert!(!diagnostic.contains(bad));
        }
        assert!(!diagnostic.contains("http://"));
    }
}

#[test]
fn ch6_telegram_server_adapter_loads_only_its_synthetic_credential_by_normal_mechanism() {
    let root = TestRoot::new();
    let token = [
        "424243",
        ":",
        "CXR_FAKE_CH6_SERVER_ADAPTER_CANARY_LOCAL_ONLY",
    ]
    .concat();
    let credential = root.path().join("telegram_bot");
    fs::write(&credential, &token).unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let reference = CredentialRef::new("telegram_bot".to_owned());
    let mut loaded = load_credentials(
        &CredentialSourceConfig::LocalDirectory {
            directory: root.path().to_owned(),
        },
        [&reference],
    )
    .unwrap();
    assert_eq!(loaded.len(), 1);
    let client = Arc::new(
        TelegramClient::for_test(
            loaded.remove("telegram_bot").unwrap(),
            "http://127.0.0.1:1/",
        )
        .unwrap(),
    );
    let adapter = TelegramDeliveryAdapter::new(ChannelAccountId::generate(), client);
    let diagnostics = format!("{adapter:?}");
    assert!(!diagnostics.contains(&token));
    assert!(!diagnostics.contains(root.path().to_str().unwrap()));
    assert_eq!(diagnostics.matches("[REDACTED]").count(), 1);
}

#[tokio::test]
async fn get_me_webhook_and_poll_requests_are_minimal_bounded_and_identity_pinned() {
    let (client, recorded, join) = client_with(vec![
        ResponseSpec::json(
            200,
            json!({"ok":true,"result":{"id":10001,"is_bot":true,"username":"ignored"}}),
        ),
        ResponseSpec::json(200, json!({"ok":true,"result":{"url":""}})),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    client.verify_get_me(10001).await.unwrap();
    client.verify_webhook_absent().await.unwrap();
    assert!(
        client
            .get_updates(None, true)
            .await
            .unwrap()
            .updates
            .is_empty()
    );
    assert!(
        client
            .get_updates(Some(42), false)
            .await
            .unwrap()
            .updates
            .is_empty()
    );
    join.await.unwrap();

    let requests = recorded.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].path.ends_with("/getMe"));
    assert!(requests[1].path.ends_with("/getWebhookInfo"));
    assert!(requests[2].path.ends_with("/getUpdates"));
    assert!(requests[3].path.ends_with("/getUpdates"));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        json!({})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].body).unwrap(),
        json!({"limit":32,"timeout":1,"allowed_updates":["message"]})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[3].body).unwrap(),
        json!({"offset":42,"limit":32,"timeout":25,"allowed_updates":["message"]})
    );
    assert!(requests.iter().all(|request| {
        !request
            .body
            .windows("drop_pending_updates".len())
            .any(|window| window == b"drop_pending_updates")
            && !request.path.ends_with("/deleteWebhook")
    }));
}

#[tokio::test]
async fn get_me_and_webhook_fail_closed_without_sensitive_provider_values() {
    let cases = [
        (
            ResponseSpec::json(200, json!({"ok":true,"result":{"id":9,"is_bot":true}})),
            super::TelegramClientErrorKind::FatalIdentityMismatch,
        ),
        (
            ResponseSpec::json(200, json!({"ok":true,"result":{"id":10001,"is_bot":false}})),
            super::TelegramClientErrorKind::FatalIdentityMismatch,
        ),
        (
            ResponseSpec::json(
                200,
                json!({"ok":true,"result":{"id":10001,"is_bot":true,"has_topics_enabled":true}}),
            ),
            super::TelegramClientErrorKind::FatalPrivateTopics,
        ),
        (
            ResponseSpec::json(
                401,
                json!({"ok":false,"error_code":401,"description":"secret description"}),
            ),
            super::TelegramClientErrorKind::FatalAuthentication,
        ),
        (
            ResponseSpec::bytes(200, b"not-json".to_vec()),
            super::TelegramClientErrorKind::MalformedResponse,
        ),
    ];
    for (response, expected) in cases {
        let (client, _, join) = client_with(vec![response]).await;
        let error = client.verify_get_me(10001).await.unwrap_err();
        assert_eq!(error.kind(), expected);
        let rendered = format!("{error}{error:?}");
        assert!(!rendered.contains(TOKEN));
        assert!(!rendered.contains("secret description"));
        join.await.unwrap();
    }

    let (client, _, join) = client_with(vec![ResponseSpec::json(
        200,
        json!({"ok":true,"result":{"url":"https://forbidden.invalid/secret"}}),
    )])
    .await;
    let error = client.verify_webhook_absent().await.unwrap_err();
    assert_eq!(
        error.kind(),
        super::TelegramClientErrorKind::FatalWebhookConfigured
    );
    assert!(!format!("{error}{error:?}").contains("forbidden"));
    join.await.unwrap();

    let oversized = vec![b'x'; 64 * 1024 + 1];
    let (client, _, join) = client_with(vec![ResponseSpec::bytes(200, oversized)]).await;
    assert_eq!(
        client.verify_get_me(10001).await.unwrap_err().kind(),
        super::TelegramClientErrorKind::OversizedResponse
    );
    join.await.unwrap();
}

#[tokio::test]
async fn polling_http_errors_and_bounded_protocol_failures_are_closed_and_typed() {
    let cases = [
        (
            ResponseSpec::json(
                429,
                json!({"ok":false,"error_code":429,"parameters":{"retry_after":9}}),
            ),
            super::TelegramClientErrorKind::RetryableRateLimited,
            Some(Duration::from_secs(9)),
        ),
        (
            ResponseSpec::json(503, json!({"ok":false,"error_code":503})),
            super::TelegramClientErrorKind::RetryableServerRejected,
            None,
        ),
        (
            ResponseSpec::json(500, json!({"unexpected":true})),
            super::TelegramClientErrorKind::MalformedResponse,
            None,
        ),
        (
            ResponseSpec::json(403, json!({"ok":false,"error_code":403})),
            super::TelegramClientErrorKind::FatalAuthentication,
            None,
        ),
        (
            ResponseSpec::json(409, json!({"ok":false,"error_code":409})),
            super::TelegramClientErrorKind::FatalConflict,
            None,
        ),
        (
            ResponseSpec::json(200, json!({"ok":true,"result":[{"update_id":"bad"}]})),
            super::TelegramClientErrorKind::MalformedResponse,
            None,
        ),
    ];
    for (response, expected, retry_after) in cases {
        let (client, _, server) = client_with(vec![response]).await;
        let error = match client.get_updates(None, false).await {
            Err(error) => error,
            Ok(_) => panic!("polling error expected"),
        };
        assert_eq!(error.kind(), expected);
        assert_eq!(error.retry_after(), retry_after);
        server.await.unwrap();
    }

    let (client, _, server) = client_with(vec![ResponseSpec::bytes(
        200,
        vec![b'x'; 4 * 1024 * 1024 + 1],
    )])
    .await;
    let error = match client.get_updates(None, false).await {
        Err(error) => error,
        Ok(_) => panic!("oversized polling response must fail"),
    };
    assert_eq!(
        error.kind(),
        super::TelegramClientErrorKind::OversizedResponse
    );
    server.await.unwrap();
}

#[tokio::test]
async fn send_message_shape_success_and_provider_failure_mapping_are_exact() {
    let success_text = "  exact é payload  ";
    let responses = vec![
        ResponseSpec::json(
            200,
            json!({"ok":true,"result":{"message_id":77,"date":1789516923,"chat":{"id":-10001,"type":"private"}}}),
        ),
        ResponseSpec::json(
            429,
            json!({"ok":false,"error_code":429,"description":"ignored","parameters":{"retry_after":7}}),
        ),
        ResponseSpec::json(
            503,
            json!({"ok":false,"error_code":503,"description":"ignored"}),
        ),
        ResponseSpec::json(
            400,
            json!({"ok":false,"error_code":400,"description":"ignored","parameters":{"migrate_to_chat_id":999}}),
        ),
    ];
    let (client, recorded, join) = client_with(responses).await;
    let account = ChannelAccountId::generate();
    let adapter = TelegramDeliveryAdapter::new(account, client);

    let accepted = adapter
        .dispatch(dispatch(account, "-10001", success_text))
        .await;
    let ChannelDispatchResult::Accepted {
        external_message_id: Some(external_message_id),
    } = accepted
    else {
        panic!("accepted Telegram message expected");
    };
    assert_eq!(external_message_id.as_str(), "77");

    let rate_limited = adapter.dispatch(dispatch(account, "-10001", "rate")).await;
    assert_failure(
        &rate_limited,
        DeliveryFailureClass::ProviderRetryable,
        "telegram_rate_limited",
    );
    assert!(matches!(
        rate_limited,
        ChannelDispatchResult::RetryableFailure {
            retry_after: Some(value), ..
        } if value == Duration::from_secs(7)
    ));
    let server = adapter
        .dispatch(dispatch(account, "-10001", "server"))
        .await;
    assert_failure(
        &server,
        DeliveryFailureClass::ProviderRetryable,
        "telegram_server_rejected",
    );
    let permanent = adapter.dispatch(dispatch(account, "-10001", "bad")).await;
    assert_failure(
        &permanent,
        DeliveryFailureClass::ProviderPermanent,
        "telegram_request_rejected",
    );
    join.await.unwrap();

    let requests = recorded.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({"chat_id":-10001,"text":success_text})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn send_message_ambiguous_responses_are_terminal_unknown() {
    let responses = vec![
        ResponseSpec::json(
            200,
            json!({"ok":true,"result":{"message_id":1,"date":1789516923,"chat":{"id":999,"type":"private"}}}),
        ),
        ResponseSpec::bytes(200, b"malformed".to_vec()),
        ResponseSpec::json(200, json!({"ok":true,"result":{"chat":{"id":123}}})),
        ResponseSpec::json(500, json!({"unexpected":true})),
        ResponseSpec::bytes(200, vec![b'x'; 64 * 1024 + 1]),
        ResponseSpec {
            status: 200,
            body: b"{\"ok\":".to_vec(),
            declared_length: Some(100),
        },
    ];
    let (client, _, join) = client_with(responses).await;
    let account = ChannelAccountId::generate();
    let adapter = TelegramDeliveryAdapter::new(account, client);
    for _ in 0..6 {
        let result = adapter.dispatch(dispatch(account, "123", "payload")).await;
        assert!(matches!(
            result,
            ChannelDispatchResult::OutcomeUnknown { .. }
        ));
        let diagnostic = format!("{result:?}");
        assert!(!diagnostic.contains("payload"));
        assert!(!diagnostic.contains("123"));
    }
    join.await.unwrap();
}

#[tokio::test]
async fn route_account_and_payload_preflight_is_permanent_without_http() {
    let client = Arc::new(
        TelegramClient::for_test(SecretString::new(TOKEN.to_owned()), "http://127.0.0.1:1/")
            .unwrap(),
    );
    let account = ChannelAccountId::generate();
    let adapter = TelegramDeliveryAdapter::new(account, client);
    let mut cases = Vec::new();
    let mut wrong_provider = dispatch(account, "123", "payload");
    wrong_provider.provider_id = ChannelProviderId::try_new("other").unwrap();
    cases.push(wrong_provider);
    let mut wrong_account = dispatch(account, "123", "payload");
    wrong_account.channel_account_id = ChannelAccountId::generate();
    cases.push(wrong_account);
    let mut thread = dispatch(account, "123", "payload");
    thread.external_thread_id = Some(ExternalThreadId::try_new("1").unwrap());
    cases.push(thread);
    cases.push(dispatch(account, "123", ""));
    cases.push(dispatch(account, "+123", "payload"));
    cases.push(dispatch(account, "0", "payload"));
    cases.push(dispatch(account, "4503599627370496", "payload"));
    cases.push(dispatch(account, "123", &"x".repeat(4097)));
    let mut hash_mismatch = dispatch(account, "123", "payload");
    hash_mismatch.payload_sha256 = Sha256Digest::hash_bytes(b"different");
    cases.push(hash_mismatch);

    for case in cases {
        let result = adapter.dispatch(case).await;
        assert!(matches!(
            result,
            ChannelDispatchResult::PermanentFailure { .. }
        ));
    }
}

#[tokio::test]
async fn definite_connection_establishment_failure_is_retryable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = Arc::new(
        TelegramClient::for_test(
            SecretString::new(TOKEN.to_owned()),
            &format!("http://{address}/"),
        )
        .unwrap(),
    );
    let account = ChannelAccountId::generate();
    let result = TelegramDeliveryAdapter::new(account, client)
        .dispatch(dispatch(account, "123", "payload"))
        .await;
    assert_failure(
        &result,
        DeliveryFailureClass::ProviderRetryable,
        "telegram_connect",
    );
}

struct FixedJitter;

impl TelegramJitterSource for FixedJitter {
    fn sample_inclusive(&mut self, lower_millis: u64, _: u64) -> u64 {
        lower_millis
    }
}

struct FixedDeliveryJitter(u64);

impl DeliveryJitterSource for FixedDeliveryJitter {
    fn sample_inclusive(&mut self, upper_bound_millis: u64) -> u64 {
        self.0.min(upper_bound_millis)
    }
}

struct CountingTelegramAdapter {
    inner: TelegramDeliveryAdapter,
    calls: Arc<AtomicUsize>,
}

impl ChannelDeliveryAdapter for CountingTelegramAdapter {
    fn provider_id(&self) -> &ChannelProviderId {
        self.inner.provider_id()
    }

    fn dispatch(&self, dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.inner.dispatch(dispatch)
    }
}

#[derive(Clone)]
struct TraceCapture {
    bytes: Arc<Mutex<Vec<u8>>>,
    changed: Arc<tokio::sync::Notify>,
}

impl<'writer> MakeWriter<'writer> for TraceCapture {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for TraceCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        self.changed.notify_one();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn trace_dispatch() -> (tracing::Dispatch, TraceCapture) {
    let capture = TraceCapture {
        bytes: Arc::new(Mutex::new(Vec::new())),
        changed: Arc::new(tokio::sync::Notify::new()),
    };
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(capture.clone())
            .finish(),
    );
    (dispatch, capture)
}

fn trace_output(capture: &TraceCapture) -> String {
    String::from_utf8(capture.bytes.lock().unwrap().clone()).unwrap()
}

async fn wait_for_trace(
    capture: &TraceCapture,
    expected: &str,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let changed = capture.changed.notified();
            if trace_output(capture).contains(expected) {
                return;
            }
            changed.await;
        }
    })
    .await
}

fn event_count(output: &str, event_name: &str) -> usize {
    output
        .lines()
        .filter(|line| line.contains(&format!(r#""event_name":"{event_name}""#)))
        .count()
}

#[derive(Default)]
struct PollingSink(Mutex<Vec<i64>>);

impl TelegramUpdateSink for PollingSink {
    fn process_update(
        &self,
        update: RawUpdate,
    ) -> Pin<Box<dyn Future<Output = Result<(), TelegramPollerError>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push(update.update_id.as_i64().unwrap());
            Ok(())
        })
    }
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn retryable_polling_degradation_is_safe_bounded_and_recovers_once() {
    const RAW_DESCRIPTION: &str = "RAW_PROVIDER_DESCRIPTION_SENTINEL";
    const USER_ID: &str = "TELEGRAM_USER_ID_SENTINEL_20002";
    const CHAT_ID: &str = "TELEGRAM_CHAT_ID_SENTINEL_30003";
    const MESSAGE_TEXT: &str = "TELEGRAM_MESSAGE_TEXT_SENTINEL";
    const WEBHOOK_URL: &str = "https://webhook.invalid/WEBHOOK_URL_SENTINEL";
    let description = format!(
        "{RAW_DESCRIPTION} token={TOKEN} url=https://api.telegram.org/bot{TOKEN}/getUpdates \
         user={USER_ID} chat={CHAT_ID} text={MESSAGE_TEXT} webhook={WEBHOOK_URL}"
    );
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::json(
            503,
            json!({"ok":false,"error_code":503,"description":description}),
        ),
        ResponseSpec::json(
            503,
            json!({"ok":false,"error_code":503,"description":description}),
        ),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    let health = Health::new();
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let (dispatch, traces) = trace_dispatch();
    let exercise = async {
        let mut handle = start_long_polling(
            client,
            TelegramPollBatch {
                updates: Vec::new(),
            },
            Arc::new(PollingSink::default()),
            health.clone(),
            fatal,
            FixedJitter,
        );
        handle.wait_started().await.unwrap();
        wait_for_trace(&traces, r#""consecutive_retry_count":2"#)
            .await
            .unwrap();
        assert_eq!(health.snapshot().state(), HealthState::Ready);
        assert!(!*fatal_receiver.borrow());
        let recovery_observed =
            wait_for_trace(&traces, r#""event_name":"telegram_polling_recovered""#).await;
        assert!(
            recovery_observed.is_ok(),
            "recovery timed out after {} requests with health {:?} and traces: {}",
            recorded.lock().unwrap().len(),
            health.snapshot().state(),
            trace_output(&traces)
        );
        assert_eq!(health.snapshot().state(), HealthState::Ready);
        assert!(!*fatal_receiver.borrow());
        handle
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
    };
    exercise.with_subscriber(dispatch).await;
    server.await.unwrap();

    let output = trace_output(&traces);
    let server_rejections: Vec<_> = output
        .lines()
        .filter(|line| line.contains(r#""failure_class":"server_rejected""#))
        .collect();
    assert_eq!(server_rejections.len(), 2, "{output}");
    assert!(
        server_rejections[0].contains(r#""consecutive_retry_count":1"#)
            && server_rejections[0].contains(r#""retry_delay_ms":500"#),
        "{output}"
    );
    assert!(
        server_rejections[1].contains(r#""consecutive_retry_count":2"#)
            && server_rejections[1].contains(r#""retry_delay_ms":1000"#),
        "{output}"
    );
    assert_eq!(event_count(&output, "telegram_polling_recovered"), 1);
    assert!(output.contains(r#""provider":"telegram""#));
    assert!(output.contains(r#""polling_state":"degraded_retrying""#));
    assert!(output.contains(r#""result_class":"retry_scheduled""#));
    assert!(output.contains(r#""polling_state":"healthy""#));
    assert!(output.contains(r#""prior_retry_count":2"#));
    for forbidden in [
        TOKEN,
        &format!("bot{TOKEN}"),
        RAW_DESCRIPTION,
        USER_ID,
        CHAT_ID,
        MESSAGE_TEXT,
        WEBHOOK_URL,
    ] {
        assert!(!output.contains(forbidden), "leaked {forbidden}: {output}");
    }
}

#[tokio::test]
async fn healthy_polling_without_prior_degradation_emits_no_recovery() {
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    let health = Health::new();
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let (dispatch, traces) = trace_dispatch();
    let exercise = async {
        let mut handle = start_long_polling(
            client,
            TelegramPollBatch {
                updates: Vec::new(),
            },
            Arc::new(PollingSink::default()),
            health.clone(),
            fatal,
            FixedJitter,
        );
        handle.wait_started().await.unwrap();
        wait_for_trace(&traces, r#""failure_class":"transport_unavailable""#)
            .await
            .unwrap();
        assert_eq!(health.snapshot().state(), HealthState::Ready);
        assert!(!*fatal_receiver.borrow());
        handle
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
    };
    exercise.with_subscriber(dispatch).await;
    server.await.unwrap();

    assert_eq!(recorded.lock().unwrap().len(), 2);
    let output = trace_output(&traces);
    assert_eq!(event_count(&output, "telegram_polling_recovered"), 0);
}

#[tokio::test]
async fn empty_confirmation_clears_offset_and_accepts_a_lower_long_idle_update() {
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
        ResponseSpec::json(200, json!({"ok":true,"result":[{"update_id":5}]})),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    let initial = TelegramPollBatch {
        updates: vec![update(json!({"update_id":100}))],
    };
    let sink = Arc::new(PollingSink::default());
    let health = Health::new();
    assert_eq!(health.snapshot().state(), HealthState::LiveUnready);
    let (fatal, _) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        initial,
        Arc::clone(&sink),
        health.clone(),
        fatal,
        FixedJitter,
    );
    handle.wait_started().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Ready);
    wait_until(|| recorded.lock().unwrap().len() == 3).await;
    wait_until(|| sink.0.lock().unwrap().as_slice() == [100, 5]).await;
    handle
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();

    let requests = recorded.lock().unwrap();
    let first = serde_json::from_slice::<Value>(&requests[0].body).unwrap();
    let second = serde_json::from_slice::<Value>(&requests[1].body).unwrap();
    let third = serde_json::from_slice::<Value>(&requests[2].body).unwrap();
    assert_eq!(first["offset"], 101);
    assert!(second.get("offset").is_none());
    assert_eq!(third["offset"], 6);
}

#[tokio::test]
async fn three_consecutive_malformed_polls_are_fatal_but_a_valid_poll_resets_the_budget() {
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::bytes(200, b"bad-1".to_vec()),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
        ResponseSpec::bytes(200, b"bad-2".to_vec()),
        ResponseSpec::bytes(200, b"bad-3".to_vec()),
        ResponseSpec::bytes(200, b"bad-4".to_vec()),
    ])
    .await;
    let sink = Arc::new(PollingSink::default());
    let health = Health::new();
    let (fatal, mut fatal_receiver) = tokio::sync::watch::channel(false);
    // This poller emits a recovery event after the valid second response. Keep that task under a
    // scoped dispatcher so concurrent observability tests never register the shared callsite
    // against a no-subscriber dispatcher.
    let (dispatch, _traces) = trace_dispatch();
    let mut handle = async {
        start_long_polling(
            client,
            TelegramPollBatch {
                updates: Vec::new(),
            },
            sink,
            health.clone(),
            fatal,
            FixedJitter,
        )
    }
    .with_subscriber(dispatch)
    .await;
    handle.wait_started().await.unwrap();
    wait_until(|| recorded.lock().unwrap().len() >= 4).await;
    assert!(!*fatal_receiver.borrow());
    tokio::time::timeout(Duration::from_secs(5), fatal_receiver.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*fatal_receiver.borrow());
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    assert_eq!(recorded.lock().unwrap().len(), 5);
    assert_eq!(
        handle
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .unwrap_err()
            .kind(),
        TelegramPollerErrorKind::FatalProtocol
    );
    server.await.unwrap();
}

#[tokio::test]
async fn startup_connectivity_budget_stops_after_exactly_three_failures() {
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::bytes(200, b"bad-1".to_vec()),
        ResponseSpec::bytes(200, b"bad-2".to_vec()),
        ResponseSpec::bytes(200, b"bad-3".to_vec()),
    ])
    .await;
    let mut budget = TelegramStartupFailureBudget::default();
    let error = verify_startup_identity(client.as_ref(), 10001, &mut FixedJitter, &mut budget)
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        super::TelegramClientErrorKind::MalformedResponse
    );
    assert_eq!(recorded.lock().unwrap().len(), 3);
    assert!(
        recorded
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.path.ends_with("/getMe"))
    );
    server.await.unwrap();
}

#[tokio::test]
async fn fatal_poll_conflict_reaches_runtime_health_after_started_handshake() {
    let (client, _, server) = client_with(vec![ResponseSpec::json(
        409,
        json!({"ok":false,"error_code":409,"description":"must stay private"}),
    )])
    .await;
    let health = Health::new();
    let (fatal, mut fatal_receiver) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        TelegramPollBatch {
            updates: Vec::new(),
        },
        Arc::new(PollingSink::default()),
        health.clone(),
        fatal,
        FixedJitter,
    );
    handle.wait_started().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), fatal_receiver.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*fatal_receiver.borrow());
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    assert_eq!(
        handle
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .unwrap_err()
            .kind(),
        TelegramPollerErrorKind::FatalProvider
    );
    server.await.unwrap();
}

#[tokio::test]
async fn active_long_poll_is_cancelled_and_joined_on_shutdown() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let (accepted, accepted_wait) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buffer = [0_u8; 1024];
            let count = stream.read(&mut buffer).await.unwrap();
            if count == 0 {
                return true;
            }
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let _ = accepted.send(());
        let mut byte = [0_u8; 1];
        stream.read(&mut byte).await.unwrap() == 0
    });
    let client =
        Arc::new(TelegramClient::for_test(SecretString::new(TOKEN.to_owned()), &origin).unwrap());
    let health = Health::new();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        TelegramPollBatch {
            updates: Vec::new(),
        },
        Arc::new(PollingSink::default()),
        health,
        fatal,
        FixedJitter,
    );
    handle.wait_started().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), accepted_wait)
        .await
        .unwrap()
        .unwrap();
    handle
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
    );
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-ch5-telegram-{}-{}",
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

#[derive(Clone, Default)]
struct Effects;

impl CommandPostCommit for Effects {
    fn message_committed(&self, _: crate::domain::WorkId, _: crate::domain::JournalOffset) {}

    fn active_cancellation_committed(
        &self,
        _: crate::domain::WorkId,
        _: crate::domain::JournalOffset,
    ) {
    }

    fn direct_cancellation_committed(
        &self,
        _: crate::domain::WorkId,
        _: crate::domain::JournalOffset,
    ) {
    }
}

type Processor =
    TelegramInboundProcessor<SqliteStateStore, Effects, SqliteChannelIdentityStore, TestClock>;

struct InboundFixture {
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: Arc<SqliteStateStore>,
    topology: TelegramOwnerTopology,
    topology_service: Arc<ChannelTopologyService<SqliteChannelIdentityStore>>,
    ingress: Arc<ChannelIngressService<SqliteStateStore, Effects>>,
    clock: Arc<TestClock>,
    health: Health,
}

impl InboundFixture {
    fn processor(&self) -> Arc<Processor> {
        Arc::new(TelegramInboundProcessor::new(
            self.topology,
            Arc::clone(&self.topology_service),
            Arc::clone(&self.ingress),
            Arc::clone(&self.clock),
        ))
    }

    async fn count(&self, table: &str) -> i64 {
        let mut connection = self.guard.runtime().acquire_for_test().await.unwrap();
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
            .fetch_one(&mut *connection)
            .await
            .unwrap()
    }
}

async fn inbound_fixture() -> InboundFixture {
    let health = Health::new();
    health.mark_ready().unwrap();
    inbound_fixture_with_health(health).await
}

async fn inbound_fixture_with_health(health: Health) -> InboundFixture {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let at: crate::domain::UtcTimestamp = T0.parse().unwrap();
    let owner = store
        .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
            proposed: V0IdentityReference {
                craxii_id: CraxiiId::generate(),
                user_id: UserId::generate(),
                conversation_id: crate::domain::ConversationId::generate(),
                workstation_id: WorkstationId::generate(),
                workspace_id: WorkspaceId::generate(),
            },
            initialized_event_id: JournalEventId::generate(),
            conversation_created_event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
            created_at: at,
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".into(),
                os_release: "ch5-test".into(),
                default_shell: "/bin/sh".into(),
                workspace_logical_name: "primary".into(),
                workspace_logical_root: "/workspace".into(),
                workspace_resolved_root: "/workspace".into(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    let topology_service = Arc::new(ChannelTopologyService::new(Arc::new(
        SqliteChannelIdentityStore::new(guard.runtime().clone()),
    )));
    let account_id = ChannelAccountId::generate();
    let ensured = topology_service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY).unwrap(),
            ExternalAccountId::try_new("10001").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("20002").unwrap(),
            at,
        )
        .await
        .unwrap();
    let profiles =
        Arc::new(ChannelDeliveryProfileRegistry::try_new([telegram_delivery_profile()]).unwrap());
    let ingress = Arc::new(
        ChannelIngressService::new(
            Arc::clone(&store),
            health.clone(),
            MutationAdmission::new(),
            Effects,
        )
        .with_delivery(profiles, DeliveryNotifier::new()),
    );
    InboundFixture {
        _root: root,
        guard,
        store,
        topology: TelegramOwnerTopology {
            channel_account_id: account_id,
            external_identity_id: ensured.external_identity_id,
            craxii_id: owner.craxii_id,
            user_id: owner.user_id,
            conversation_id: owner.conversation_id,
            owner_telegram_user_id: 20002,
        },
        topology_service,
        ingress,
        clock: Arc::new(TestClock::new(
            time::OffsetDateTime::parse(T1, &time::format_description::well_known::Rfc3339)
                .unwrap(),
            Duration::ZERO,
        )),
        health,
    }
}

fn delivery_profiles() -> Arc<ChannelDeliveryProfileRegistry> {
    Arc::new(ChannelDeliveryProfileRegistry::try_new([telegram_delivery_profile()]).unwrap())
}

fn clock_at(value: &str) -> Arc<TestClock> {
    Arc::new(TestClock::new(
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).unwrap(),
        Duration::ZERO,
    ))
}

async fn fresh_runtime(
    store: &SqliteStateStore,
    topology: TelegramOwnerTopology,
    clock: &TestClock,
) -> RuntimeInstanceId {
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let started_at = UtcTimestamp::from_offset_datetime(clock.utc_now().unwrap()).unwrap();
    bootstrap_runtime(
        store,
        RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: RuntimeInstanceId::generate(),
            craxii_id: topology.craxii_id,
            workstation_id: snapshot.workstation.workstation_id(),
            workstation_generation: snapshot.workstation.generation(),
            linux_boot_id: Some(LinuxBootId::try_new("ch6-local-restart").unwrap()),
            diagnostic_pid: Some(DiagnosticPid::try_new(std::process::id().into()).unwrap()),
            package_version: PackageVersion::try_new("0.0.1").unwrap(),
            git_revision: GitRevision::try_new("ch6-local").unwrap(),
            schema_version: SchemaVersion::try_new(7).unwrap(),
            started_at,
        }),
        0,
        clock,
    )
    .await
    .unwrap()
    .runtime_instance_id
}

async fn recover_deliveries(store: &SqliteStateStore, clock: &TestClock) {
    store
        .recover_stale_deliveries(RecoverDeliveriesRequest {
            recovered_at: UtcTimestamp::from_offset_datetime(clock.utc_now().unwrap()).unwrap(),
        })
        .await
        .unwrap();
}

fn telegram_registry(
    account_id: ChannelAccountId,
    client: Arc<TelegramClient>,
    calls: Arc<AtomicUsize>,
) -> Arc<ChannelDeliveryAdapterRegistry> {
    let profiles = delivery_profiles();
    let provider = ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY).unwrap();
    let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(CountingTelegramAdapter {
        inner: TelegramDeliveryAdapter::new(account_id, client),
        calls,
    });
    Arc::new(ChannelDeliveryAdapterRegistry::try_new(&profiles, [(provider, adapter)]).unwrap())
}

async fn wait_for_single_delivery_state(guard: &SqliteRuntimeGuard, expected: &str) -> (i64, i64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut connection = guard.runtime().acquire_for_test().await.unwrap();
            let row = sqlx::query(
                "SELECT state, attempt_count, \
                        (SELECT COUNT(*) FROM outbound_delivery_attempts) AS attempts \
                 FROM outbound_deliveries",
            )
            .fetch_optional(&mut *connection)
            .await
            .unwrap();
            if let Some(row) = row
                && row.try_get::<String, _>("state").unwrap() == expected
            {
                return (
                    row.try_get::<i64, _>("attempt_count").unwrap(),
                    row.try_get::<i64, _>("attempts").unwrap(),
                );
            }
            drop(connection);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("delivery did not reach expected state")
}

fn reopened_processor(
    guard: &SqliteRuntimeGuard,
    topology: TelegramOwnerTopology,
    clock: Arc<TestClock>,
    notifier: DeliveryNotifier,
) -> Arc<Processor> {
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let topology_service = Arc::new(ChannelTopologyService::new(Arc::new(
        SqliteChannelIdentityStore::new(guard.runtime().clone()),
    )));
    let health = Health::new();
    health.mark_ready().unwrap();
    let ingress = Arc::new(
        ChannelIngressService::new(store, health, MutationAdmission::new(), Effects)
            .with_delivery(delivery_profiles(), notifier),
    );
    Arc::new(TelegramInboundProcessor::new(
        topology,
        topology_service,
        ingress,
        clock,
    ))
}

struct CancellationProcessRunner {
    workstation: Arc<LocalWorkstation>,
    command: String,
    result: Arc<Mutex<Option<ExecutionResult>>>,
}

impl WorkRunner for CancellationProcessRunner {
    fn start(
        &self,
        claimed: ClaimedWork,
        mut cancellation: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let workstation = Arc::clone(&self.workstation);
        let command = self.command.clone();
        let result_slot = Arc::clone(&self.result);
        let work_id = claimed.work.work_id();
        let execution_id = ExecutionId::generate();
        Ok(Box::pin(async move {
            let requested_cwd = LogicalPathReference::workspace_relative("cwd").unwrap();
            let preparation = workstation
                .prepare(WorkstationPreparationRequest {
                    operation_id: OperationId::generate(),
                    workstation_id: workstation.workstation_id(),
                    expected_generation: workstation.generation(),
                    workspace_id: workstation.workspace_id(),
                    requested_cwd: requested_cwd.clone(),
                    required_capability: RequiredWorkstationCapability::ForegroundExecute,
                    effective_privilege: PrivilegeMode::User,
                })
                .await;
            let Ok(preparation) = preparation else {
                return WorkRunnerExit::Abnormal;
            };
            let request = ExecutionRequest {
                operation_id: OperationId::generate(),
                execution_id,
                work_id,
                workstation_id: workstation.workstation_id(),
                expected_generation: workstation.generation(),
                workspace_id: workstation.workspace_id(),
                command,
                requested_cwd,
                prepared_cwd: preparation.prepared_cwd,
                effective_privilege: PrivilegeMode::User,
                stdin: ExecutionStdinPolicy::Closed,
                timeout: MonotonicDuration::from_millis(60_000),
                deadline: MonotonicInstant::from_elapsed(Duration::from_secs(120)),
                capture: ExecutionCapturePolicy {
                    stdout_max_bytes: HARD_EXECUTION_STREAM_CAPTURE_BYTES,
                    stderr_max_bytes: HARD_EXECUTION_STREAM_CAPTURE_BYTES,
                },
                cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
            };
            let mut execution = Box::pin(workstation.execute(request));
            tokio::select! {
                completed = &mut execution => {
                    if let Ok(completed) = completed {
                        *result_slot.lock().unwrap() = Some(completed);
                    }
                    WorkRunnerExit::Abnormal
                }
                () = cancellation.requested() => {
                    let cancelled = workstation.cancel_execution(ExecutionCancellationRequest {
                        operation_id: OperationId::generate(),
                        execution_id,
                        workstation_id: workstation.workstation_id(),
                        expected_generation: workstation.generation(),
                    }).await;
                    let completed = execution.await;
                    let confirmed = matches!(
                        cancelled,
                        Ok(value) if matches!(
                            value.state,
                            ExecutionCancellationState::Confirmed
                                | ExecutionCancellationState::AlreadyTerminal
                        )
                    ) && matches!(
                        completed,
                        Ok(ref value)
                            if value.result_kind == ExecutionResultKind::Cancelled
                                && value.cancelled
                                && value.cleanup.confirmed()
                    );
                    if let Ok(completed) = completed {
                        *result_slot.lock().unwrap() = Some(completed);
                    }
                    if confirmed {
                        WorkRunnerExit::CancellationConfirmed
                    } else {
                        WorkRunnerExit::Abnormal
                    }
                }
            }
        }))
    }
}

async fn wait_for_work_state(guard: &SqliteRuntimeGuard, expected: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut connection = guard.runtime().acquire_for_test().await.unwrap();
            let state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM work_items ORDER BY created_at LIMIT 1",
            )
            .fetch_optional(&mut *connection)
            .await
            .unwrap();
            if state.as_deref() == Some(expected) {
                return;
            }
            drop(connection);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("work did not reach expected state");
}

async fn wait_for_path(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("process marker was not created");
}

struct FirstClassificationGate {
    inner: Arc<SqliteStateStore>,
    first: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl FirstClassificationGate {
    fn new(inner: Arc<SqliteStateStore>) -> Self {
        Self {
            inner,
            first: AtomicBool::new(true),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }
}

impl ChannelIngressStore for FirstClassificationGate {
    fn load_channel_provider_id(
        &self,
        channel_account_id: ChannelAccountId,
    ) -> ChannelProviderFuture<'_> {
        self.inner.load_channel_provider_id(channel_account_id)
    }

    fn classify_inbound(&self, request: ClassifyInboundRequest) -> ChannelIngressFuture<'_> {
        Box::pin(async move {
            if self.first.swap(false, Ordering::AcqRel) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.classify_inbound(request).await
        })
    }
}

struct FailFirstClassification {
    inner: Arc<SqliteStateStore>,
    calls: AtomicUsize,
}

impl ChannelIngressStore for FailFirstClassification {
    fn load_channel_provider_id(
        &self,
        channel_account_id: ChannelAccountId,
    ) -> ChannelProviderFuture<'_> {
        self.inner.load_channel_provider_id(channel_account_id)
    }

    fn classify_inbound(&self, request: ClassifyInboundRequest) -> ChannelIngressFuture<'_> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                Err(ChannelIngressStoreError::new(
                    ChannelIngressStoreErrorKind::Storage,
                ))
            } else {
                self.inner.classify_inbound(request).await
            }
        })
    }
}

async fn run_retained_update(fixture: &InboundFixture, retained: Value) {
    assert_eq!(fixture.health.snapshot().state(), HealthState::LiveUnready);
    assert!(fixture.health.snapshot().is_ingress_admission_ready());
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::json(200, json!({"ok":true,"result":[retained]})),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    let mut budget = TelegramStartupFailureBudget::default();
    let initial_batch = startup_probe(client.as_ref(), &mut FixedJitter, &mut budget)
        .await
        .unwrap();
    assert_eq!(fixture.health.snapshot().state(), HealthState::LiveUnready);
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        initial_batch,
        fixture.processor(),
        fixture.health.clone(),
        fatal,
        FixedJitter,
    );
    handle.wait_started().await.unwrap();
    assert_eq!(fixture.health.snapshot().state(), HealthState::Ready);
    assert!(!*fatal_receiver.borrow());
    wait_until(|| recorded.lock().unwrap().len() == 2).await;
    handle
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
}

fn update(value: Value) -> RawUpdate {
    serde_json::from_value(value).unwrap()
}

fn text_update(update_id: i64, text: &str) -> RawUpdate {
    update(text_update_value(update_id, text))
}

fn text_update_value(update_id: i64, text: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id + 100,
            "from": {"id": 20002, "is_bot": false, "username": "never-authority"},
            "chat": {"id": 20002, "type": "private"},
            "date": 1789516923,
            "text": text
        }
    })
}

#[tokio::test]
async fn retained_startup_text_uses_internal_admission_before_public_readiness() {
    let health = Health::new();
    let fixture = inbound_fixture_with_health(health.clone()).await;
    assert!(!health.snapshot().is_ingress_admission_ready());
    health.mark_ingress_admission_ready().unwrap();

    let gate = Arc::new(FirstClassificationGate::new(Arc::clone(&fixture.store)));
    let ingress = Arc::new(
        ChannelIngressService::new(
            Arc::clone(&gate),
            health.clone(),
            MutationAdmission::new(),
            Effects,
        )
        .with_delivery(
            Arc::new(
                ChannelDeliveryProfileRegistry::try_new([telegram_delivery_profile()]).unwrap(),
            ),
            DeliveryNotifier::new(),
        ),
    );
    let processor = Arc::new(TelegramInboundProcessor::new(
        fixture.topology,
        Arc::clone(&fixture.topology_service),
        ingress,
        Arc::clone(&fixture.clock),
    ));
    let (client, recorded, server) = client_with(vec![
        ResponseSpec::json(
            200,
            json!({"ok":true,"result":[text_update_value(10, "retained startup work")]}),
        ),
        ResponseSpec::json(200, json!({"ok":true,"result":[]})),
    ])
    .await;
    let mut budget = TelegramStartupFailureBudget::default();
    let initial_batch = startup_probe(client.as_ref(), &mut FixedJitter, &mut budget)
        .await
        .unwrap();
    assert_eq!(health.snapshot().state(), HealthState::LiveUnready);
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        initial_batch,
        Arc::clone(&processor),
        health.clone(),
        fatal,
        FixedJitter,
    );

    tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
        .await
        .unwrap();
    assert_eq!(health.snapshot().state(), HealthState::LiveUnready);
    assert!(health.snapshot().is_ingress_admission_ready());
    assert!(!health.snapshot().is_ready());
    assert_eq!(fixture.count("inbound_deliveries").await, 0);
    assert_eq!(fixture.count("messages").await, 0);
    assert_eq!(fixture.count("work_items").await, 0);

    gate.release.notify_one();
    handle.wait_started().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Ready);
    assert!(!*fatal_receiver.borrow());
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    assert_eq!(fixture.count("messages").await, 1);
    assert_eq!(fixture.count("work_items").await, 1);
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let row = sqlx::query(
        "SELECT d.receipt_state, d.classification, w.reply_binding_id, \
                b.conversation_binding_id \
         FROM inbound_deliveries d \
         JOIN work_items w ON w.work_id = d.work_id \
         JOIN conversation_bindings b ON b.conversation_binding_id = w.reply_binding_id \
         WHERE d.external_event_id = '10'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<String, _>("receipt_state").unwrap(),
        "classified"
    );
    assert_eq!(
        row.try_get::<String, _>("classification").unwrap(),
        "message"
    );
    assert_eq!(
        row.try_get::<String, _>("reply_binding_id").unwrap(),
        row.try_get::<String, _>("conversation_binding_id").unwrap()
    );
    drop(connection);

    processor
        .process_update(text_update(11, "normal post-startup work"))
        .await
        .unwrap();
    assert_eq!(fixture.count("messages").await, 2);
    assert_eq!(fixture.count("work_items").await, 2);

    wait_until(|| recorded.lock().unwrap().len() == 2).await;
    let request = serde_json::from_slice::<Value>(&recorded.lock().unwrap()[1].body).unwrap();
    assert_eq!(request["offset"], 11);
    handle
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn retained_startup_unsupported_is_durable_before_public_readiness() {
    let health = Health::new();
    let fixture = inbound_fixture_with_health(health.clone()).await;
    health.mark_ingress_admission_ready().unwrap();
    run_retained_update(
        &fixture,
        json!({
            "update_id": 20,
            "message": {
                "message_id": 120,
                "from": {"id": 20002, "is_bot": false},
                "chat": {"id": 20002, "type": "private"},
                "date": 1789516923,
                "photo": [{"file_id": "never-fetched"}]
            }
        }),
    )
    .await;
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    assert_eq!(fixture.count("work_items").await, 0);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let row = sqlx::query(
        "SELECT receipt_state, classification FROM inbound_deliveries \
         WHERE external_event_id = '20' ",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<String, _>("receipt_state").unwrap(),
        "classified"
    );
    assert!(matches!(
        row.try_get::<String, _>("classification").unwrap().as_str(),
        "unsupported" | "rejected"
    ));
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn retained_startup_unauthorized_is_rejected_before_public_readiness() {
    let health = Health::new();
    let fixture = inbound_fixture_with_health(health.clone()).await;
    health.mark_ingress_admission_ready().unwrap();
    run_retained_update(
        &fixture,
        json!({
            "update_id": 30,
            "message": {
                "message_id": 130,
                "from": {"id": 99999, "is_bot": false},
                "chat": {"id": 99999, "type": "private"},
                "date": 1789516923,
                "text": "unauthorized"
            }
        }),
    )
    .await;
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    assert_eq!(fixture.count("conversation_bindings").await, 0);
    assert_eq!(fixture.count("work_items").await, 0);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let classification = sqlx::query_scalar::<_, String>(
        "SELECT classification FROM inbound_deliveries WHERE external_event_id = '30'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(classification, "rejected");
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn retained_startup_control_uses_existing_generic_control_semantics() {
    let health = Health::new();
    let fixture = inbound_fixture_with_health(health.clone()).await;
    health.mark_ingress_admission_ready().unwrap();
    run_retained_update(&fixture, text_update_value(40, "/cancel")).await;
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    assert_eq!(fixture.count("messages").await, 0);
    assert_eq!(fixture.count("work_items").await, 0);
    assert_eq!(fixture.count("outbound_deliveries").await, 1);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let row = sqlx::query(
        "SELECT i.classification, o.control_outcome \
         FROM inbound_deliveries i \
         JOIN outbound_deliveries o ON o.source_inbound_delivery_id = i.inbound_delivery_id \
         WHERE i.external_event_id = '40'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<String, _>("classification").unwrap(),
        "control"
    );
    assert_eq!(
        row.try_get::<String, _>("control_outcome").unwrap(),
        "no_op"
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn retained_startup_storage_failure_blocks_readiness_and_later_updates() {
    let health = Health::new();
    let fixture = inbound_fixture_with_health(health.clone()).await;
    health.mark_ingress_admission_ready().unwrap();
    let failing = Arc::new(FailFirstClassification {
        inner: Arc::clone(&fixture.store),
        calls: AtomicUsize::new(0),
    });
    let ingress = Arc::new(ChannelIngressService::new(
        Arc::clone(&failing),
        health.clone(),
        MutationAdmission::new(),
        Effects,
    ));
    let processor = Arc::new(TelegramInboundProcessor::new(
        fixture.topology,
        Arc::clone(&fixture.topology_service),
        ingress,
        Arc::clone(&fixture.clock),
    ));
    let (client, recorded, server) = client_with(vec![ResponseSpec::json(
        200,
        json!({
            "ok":true,
            "result":[
                text_update_value(50, "must fail"),
                text_update_value(51, "must not skip")
            ]
        }),
    )])
    .await;
    let mut budget = TelegramStartupFailureBudget::default();
    let initial_batch = startup_probe(client.as_ref(), &mut FixedJitter, &mut budget)
        .await
        .unwrap();
    let (fatal, mut fatal_receiver) = tokio::sync::watch::channel(false);
    let mut handle = start_long_polling(
        client,
        initial_batch,
        processor,
        health.clone(),
        fatal,
        FixedJitter,
    );
    assert_eq!(
        handle.wait_started().await.unwrap_err().kind(),
        TelegramPollerErrorKind::RetryableState
    );
    tokio::time::timeout(Duration::from_secs(2), fatal_receiver.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*fatal_receiver.borrow());
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    assert!(!health.snapshot().is_ready());
    assert_eq!(failing.calls.load(Ordering::Acquire), 1);
    assert_eq!(recorded.lock().unwrap().len(), 1);
    assert!(
        serde_json::from_slice::<Value>(&recorded.lock().unwrap()[0].body)
            .unwrap()
            .get("offset")
            .is_none()
    );
    assert_eq!(fixture.count("inbound_deliveries").await, 0);
    assert_eq!(fixture.count("work_items").await, 0);
    assert_eq!(
        handle
            .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .unwrap_err()
            .kind(),
        TelegramPollerErrorKind::RetryableState
    );
    server.await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn owner_private_text_bootstraps_one_binding_and_uses_generic_ingress_exactly() {
    let fixture = inbound_fixture().await;
    let processor = fixture.processor();
    processor
        .process_update(text_update(10, "  exact é text  "))
        .await
        .unwrap();
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    assert_eq!(fixture.count("messages").await, 1);
    assert_eq!(fixture.count("work_items").await, 1);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let row = sqlx::query(
        "SELECT d.external_event_id, d.external_message_id, d.external_subject_id, \
                d.external_conversation_id, d.external_thread_id, d.provider_occurred_at, \
                d.classification, m.content_json, w.reply_binding_id \
         FROM inbound_deliveries d JOIN messages m ON m.message_id = d.message_id \
         JOIN work_items w ON w.work_id = d.work_id",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(row.try_get::<String, _>("external_event_id").unwrap(), "10");
    assert_eq!(
        row.try_get::<String, _>("external_message_id").unwrap(),
        "110"
    );
    assert_eq!(
        row.try_get::<String, _>("external_subject_id").unwrap(),
        "20002"
    );
    assert_eq!(
        row.try_get::<String, _>("external_conversation_id")
            .unwrap(),
        "20002"
    );
    assert!(
        row.try_get::<Option<String>, _>("external_thread_id")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        row.try_get::<String, _>("classification").unwrap(),
        "message"
    );
    assert!(
        row.try_get::<String, _>("content_json")
            .unwrap()
            .contains("  exact é text  ")
    );
    assert!(
        !row.try_get::<String, _>("provider_occurred_at")
            .unwrap()
            .is_empty()
    );
    assert!(
        !row.try_get::<String, _>("reply_binding_id")
            .unwrap()
            .is_empty()
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn telegram_passes_all_exact_control_spellings_to_generic_control_policy() {
    let fixture = inbound_fixture().await;
    let processor = fixture.processor();
    processor
        .process_update(text_update(20, "work"))
        .await
        .unwrap();
    for (id, text) in [(21, "/cancel"), (22, "/stop"), (23, "cancel"), (24, "stop")] {
        processor
            .process_update(text_update(id, text))
            .await
            .unwrap();
    }
    assert_eq!(fixture.count("work_items").await, 1);
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    assert_eq!(fixture.count("outbound_deliveries").await, 4);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM inbound_deliveries WHERE classification = 'control'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        4
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn unauthorized_nonprivate_media_edit_service_bot_topic_and_invalid_updates_are_durable() {
    let fixture = inbound_fixture().await;
    let processor = fixture.processor();
    let huge = "x".repeat(65_537);
    let cases = vec![
        json!({"update_id":30,"message":{"message_id":1,"from":{"id":999,"is_bot":false},"chat":{"id":999,"type":"private"},"date":1789516923,"text":"hostile"}}),
        json!({"update_id":31,"message":{"message_id":2,"from":{"id":20002,"is_bot":false},"chat":{"id":-1,"type":"group"},"date":1789516923,"text":"group"}}),
        json!({"update_id":32,"message":{"message_id":3,"from":{"id":20002,"is_bot":false},"chat":{"id":-2,"type":"supergroup"},"date":1789516923,"text":"supergroup"}}),
        json!({"update_id":33,"message":{"message_id":4,"from":{"id":20002,"is_bot":false},"chat":{"id":-3,"type":"channel"},"date":1789516923,"text":"channel"}}),
        json!({"update_id":34,"message":{"message_id":5,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"photo":[{"file_id":"must-not-download"}]}}),
        json!({"update_id":35,"message":{"message_id":6,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"photo":[{"file_id":"must-not-download"}],"caption":"caption"}}),
        json!({"update_id":36,"edited_message":{"message_id":7,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"text":"edited"}}),
        json!({"update_id":37,"message_reaction":{"chat":{"id":20002}}}),
        json!({"update_id":38,"message":{"message_id":8,"from":{"id":20002,"is_bot":true},"chat":{"id":20002,"type":"private"},"date":1789516923,"text":"bot"}}),
        json!({"update_id":39,"message":{"message_id":9,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"message_thread_id":1,"text":"topic"}}),
        json!({"update_id":40,"message":{"message_id":10,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"business_connection_id":"business","text":"business"}}),
        json!({"update_id":41,"message":{"message_id":11,"chat":{"id":20002,"type":"private"},"date":1789516923,"text":"missing from"}}),
        json!({"update_id":42,"message":{"message_id":0,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"text":"bad message id"}}),
        json!({"update_id":43,"message":{"message_id":13,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":0,"text":"bad timestamp"}}),
        json!({"update_id":44,"message":{"message_id":14,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"text":huge}}),
        json!({"update_id":45,"message":{"message_id":15,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"new_chat_title":"service","text":"service"}}),
        json!({"update_id":46,"message":{"message_id":16,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"effect_id":"rich","text":"rich"}}),
        json!({"update_id":47,"message":{"message_id":17,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"direct_messages_topic":{"topic_id":1},"text":"direct topic"}}),
        json!({"update_id":48,"message":{"message_id":18,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"is_from_offline":true,"text":"offline"}}),
        json!({"update_id":49,"message":{"message_id":19,"from":{"id":20002,"is_bot":false},"chat":{"id":20002,"type":"private"},"date":1789516923,"via_bot":{"id":10001},"text":"via bot"}}),
        json!({"update_id":50}),
    ];
    for value in cases {
        processor.process_update(update(value)).await.unwrap();
    }
    assert_eq!(fixture.count("inbound_deliveries").await, 21);
    assert_eq!(fixture.count("conversation_bindings").await, 0);
    assert_eq!(fixture.count("messages").await, 0);
    assert_eq!(fixture.count("work_items").await, 0);
    assert_eq!(fixture.count("outbound_deliveries").await, 0);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM inbound_deliveries \
             WHERE receipt_state = 'classified' AND classification IN ('rejected','unsupported')",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        21
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM inbound_deliveries \
             WHERE external_subject_id = 'telegram:unroutable:subject' \
                OR external_conversation_id = 'telegram:unroutable:conversation'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        3
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn replay_is_duplicate_but_changed_normalized_material_is_fatal_and_never_duplicates_work() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(50, "original"))
        .await
        .unwrap();
    // A fresh processor models restart with no provider cursor.
    fixture
        .processor()
        .process_update(text_update(50, "original"))
        .await
        .unwrap();
    assert_eq!(fixture.count("work_items").await, 1);
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    let error = fixture
        .processor()
        .process_update(text_update(50, "changed"))
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        TelegramPollerErrorKind::FatalContradictoryReplay
    );
    assert_eq!(fixture.count("work_items").await, 1);
    assert_eq!(fixture.count("inbound_deliveries").await, 1);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn replay_digest_covers_sender_chat_message_timestamp_and_unsupported_kind() {
    let fixture = inbound_fixture().await;
    let processor = fixture.processor();
    let base = |id: i64| {
        json!({
            "update_id":id,
            "message":{
                "message_id":id + 100,
                "from":{"id":20002,"is_bot":false},
                "chat":{"id":20002,"type":"private"},
                "date":1789516923,
                "text":"same text"
            }
        })
    };
    for (id, pointer, replacement) in [
        (80, "/message/from/id", json!(20003)),
        (81, "/message/chat/id", json!(20003)),
        (82, "/message/message_id", json!(9999)),
        (83, "/message/date", json!(1789516924)),
    ] {
        let original = base(id);
        processor
            .process_update(update(original.clone()))
            .await
            .unwrap();
        let mut changed = original;
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(
            processor
                .process_update(update(changed))
                .await
                .unwrap_err()
                .kind(),
            TelegramPollerErrorKind::FatalContradictoryReplay
        );
    }

    let media = json!({
        "update_id":84,
        "message":{
            "message_id":184,
            "from":{"id":20002,"is_bot":false},
            "chat":{"id":20002,"type":"private"},
            "date":1789516923,
            "photo":[{"file_id":"never-fetched"}]
        }
    });
    processor.process_update(update(media)).await.unwrap();
    let edit = json!({
        "update_id":84,
        "edited_message":{
            "message_id":184,
            "from":{"id":20002,"is_bot":false},
            "chat":{"id":20002,"type":"private"},
            "date":1789516923,
            "text":"edited"
        }
    });
    assert_eq!(
        processor
            .process_update(update(edit))
            .await
            .unwrap_err()
            .kind(),
        TelegramPollerErrorKind::FatalContradictoryReplay
    );
    assert_eq!(fixture.count("inbound_deliveries").await, 5);
    assert_eq!(fixture.count("work_items").await, 4);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn binding_created_before_receipt_is_reused_after_crash_boundary() {
    let fixture = inbound_fixture().await;
    fixture
        .topology_service
        .ensure_first_binding(
            fixture.topology.channel_account_id,
            fixture.topology.external_identity_id,
            fixture.topology.craxii_id,
            fixture.topology.user_id,
            fixture.topology.conversation_id,
            ExternalConversationId::try_new("20002").unwrap(),
            T0.parse().unwrap(),
        )
        .await
        .unwrap();
    fixture
        .processor()
        .process_update(text_update(60, "after crash"))
        .await
        .unwrap();
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    assert_eq!(fixture.count("work_items").await, 1);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn conflicting_owner_chat_is_durably_rejected_without_rebinding() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(70, "bind"))
        .await
        .unwrap();
    let conflict = update(json!({
        "update_id":71,
        "message": {
            "message_id":171,
            "from":{"id":20002,"is_bot":false},
            "chat":{"id":20003,"type":"private"},
            "date":1789516923,
            "text":"must not rebind"
        }
    }));
    fixture.processor().process_update(conflict).await.unwrap();
    assert_eq!(fixture.count("conversation_bindings").await, 1);
    assert_eq!(fixture.count("work_items").await, 1);
    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT classification FROM inbound_deliveries WHERE external_event_id = '71'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        "rejected"
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn ch6_queued_delivery_process_restart_dispatches_once() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(600, "/cancel"))
        .await
        .unwrap();
    assert_eq!(fixture.count("outbound_deliveries").await, 1);
    assert_eq!(
        wait_for_single_delivery_state(&fixture.guard, "queued").await,
        (0, 0)
    );

    let root = fixture._root.path().to_owned();
    let topology = fixture.topology;
    fixture.guard.shutdown().await;

    let guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let clock = clock_at("2026-09-16T01:02:10.000000Z");
    let runtime_id = fresh_runtime(store.as_ref(), topology, clock.as_ref()).await;
    recover_deliveries(store.as_ref(), clock.as_ref()).await;
    let (client, recorded, server) = client_with(vec![ResponseSpec::json(
        200,
        json!({"ok":true,"result":{"message_id":901,"date":1789516923,"chat":{"id":20002,"type":"private"}}}),
    )])
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let adapters = telegram_registry(topology.channel_account_id, client, Arc::clone(&calls));
    let delivery_clock: Arc<dyn Clock> = clock;
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let mut worker = start_delivery_worker(
        store.clone(),
        adapters,
        delivery_clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedDeliveryJitter(1_000),
        fatal,
    );
    worker.wait_initial_scan().await.unwrap();
    assert_eq!(
        wait_for_single_delivery_state(&guard, "accepted").await,
        (1, 1)
    );
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
    assert!(!*fatal_rx.borrow());
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(recorded.lock().unwrap().len(), 1);
    store.verify_delivery_consistency().await.unwrap();
    guard.shutdown().await;
}

#[tokio::test]
async fn ch6_retry_wait_process_restart_resumes_only_when_due() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(610, "/cancel"))
        .await
        .unwrap();
    let topology = fixture.topology;
    let first_clock = clock_at(T1);
    let first_runtime = fresh_runtime(fixture.store.as_ref(), topology, first_clock.as_ref()).await;
    let (first_client, first_recorded, first_server) = client_with(vec![ResponseSpec::json(
        503,
        json!({"ok":false,"error_code":503,"description":"synthetic known rejection"}),
    )])
    .await;
    let first_calls = Arc::new(AtomicUsize::new(0));
    let first_adapters = telegram_registry(
        topology.channel_account_id,
        first_client,
        Arc::clone(&first_calls),
    );
    let first_worker_clock: Arc<dyn Clock> = first_clock;
    let (first_fatal, first_fatal_rx) = tokio::sync::watch::channel(false);
    let first_worker = start_delivery_worker(
        fixture.store.clone(),
        first_adapters,
        first_worker_clock,
        first_runtime,
        DeliveryNotifier::new(),
        FixedDeliveryJitter(1_000),
        first_fatal,
    );
    assert_eq!(
        wait_for_single_delivery_state(&fixture.guard, "retry_wait").await,
        (1, 1)
    );
    first_worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    first_server.await.unwrap();
    assert!(!*first_fatal_rx.borrow());
    assert_eq!(first_calls.load(Ordering::Acquire), 1);
    assert_eq!(first_recorded.lock().unwrap().len(), 1);

    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let clock = clock_at(T1);
    let runtime_id = fresh_runtime(store.as_ref(), topology, clock.as_ref()).await;
    recover_deliveries(store.as_ref(), clock.as_ref()).await;
    let (client, recorded, server) = client_with(vec![ResponseSpec::json(
        200,
        json!({"ok":true,"result":{"message_id":902,"date":1789516923,"chat":{"id":20002,"type":"private"}}}),
    )])
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let adapters = telegram_registry(topology.channel_account_id, client, Arc::clone(&calls));
    let notifier = DeliveryNotifier::new();
    let worker_clock: Arc<dyn Clock> = clock.clone();
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let mut worker = start_delivery_worker(
        store.clone(),
        adapters,
        worker_clock,
        runtime_id,
        notifier.clone(),
        FixedDeliveryJitter(1_000),
        fatal,
    );
    worker.wait_initial_scan().await.unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(
        wait_for_single_delivery_state(&guard, "retry_wait").await,
        (1, 1)
    );

    clock.advance_wall(time::Duration::seconds(1)).unwrap();
    notifier.wake();
    assert_eq!(
        wait_for_single_delivery_state(&guard, "accepted").await,
        (2, 2)
    );
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
    assert!(!*fatal_rx.borrow());
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(recorded.lock().unwrap().len(), 1);
    store.verify_delivery_consistency().await.unwrap();
    guard.shutdown().await;
}

#[tokio::test]
async fn ch6_outcome_unknown_and_stale_dispatch_restart_never_resend() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(620, "/cancel"))
        .await
        .unwrap();
    let topology = fixture.topology;
    let first_clock = clock_at(T1);
    let first_runtime = fresh_runtime(fixture.store.as_ref(), topology, first_clock.as_ref()).await;
    let mut ambiguous = ResponseSpec::json(
        200,
        json!({"ok":true,"result":{"message_id":903,"date":1789516923,"chat":{"id":20002,"type":"private"}}}),
    );
    ambiguous.declared_length = Some(ambiguous.body.len() + 50);
    let (first_client, first_recorded, first_server) = client_with(vec![ambiguous]).await;
    let first_calls = Arc::new(AtomicUsize::new(0));
    let first_adapters = telegram_registry(
        topology.channel_account_id,
        first_client,
        Arc::clone(&first_calls),
    );
    let first_worker_clock: Arc<dyn Clock> = first_clock;
    let (first_fatal, _) = tokio::sync::watch::channel(false);
    let first_worker = start_delivery_worker(
        fixture.store.clone(),
        first_adapters,
        first_worker_clock,
        first_runtime,
        DeliveryNotifier::new(),
        FixedDeliveryJitter(1_000),
        first_fatal,
    );
    assert_eq!(
        wait_for_single_delivery_state(&fixture.guard, "outcome_unknown").await,
        (1, 1)
    );
    first_worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    first_server.await.unwrap();
    assert_eq!(first_calls.load(Ordering::Acquire), 1);
    assert_eq!(first_recorded.lock().unwrap().len(), 1);

    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let clock = clock_at("2026-09-16T01:02:30.000000Z");
    let runtime_id = fresh_runtime(store.as_ref(), topology, clock.as_ref()).await;
    recover_deliveries(store.as_ref(), clock.as_ref()).await;
    let (client, _recorded, server) = client_with(Vec::new()).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let adapters = telegram_registry(topology.channel_account_id, client, Arc::clone(&calls));
    let worker_clock: Arc<dyn Clock> = clock;
    let (fatal, fatal_rx) = tokio::sync::watch::channel(false);
    let mut worker = start_delivery_worker(
        store.clone(),
        adapters,
        worker_clock,
        runtime_id,
        DeliveryNotifier::new(),
        FixedDeliveryJitter(1_000),
        fatal,
    );
    worker.wait_initial_scan().await.unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(
        wait_for_single_delivery_state(&guard, "outcome_unknown").await,
        (1, 1)
    );
    let summaries = store
        .list_delivery_summaries(ListDeliverySummariesRequest {
            states: vec![OutboundDeliveryState::OutcomeUnknown],
            after: None,
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0].failure_class,
        Some(DeliveryFailureClass::ProviderOutcomeUnknown)
    );
    worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
    assert!(!*fatal_rx.borrow());
    guard.shutdown().await;

    let stale = inbound_fixture().await;
    stale
        .processor()
        .process_update(text_update(621, "/cancel"))
        .await
        .unwrap();
    let stale_topology = stale.topology;
    let stale_clock = clock_at(T1);
    let stale_runtime =
        fresh_runtime(stale.store.as_ref(), stale_topology, stale_clock.as_ref()).await;
    assert!(matches!(
        stale
            .store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: stale_runtime,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: T1.parse().unwrap(),
            })
            .await
            .unwrap(),
        DeliveryClaim::Dispatch(_)
    ));
    let stale_root = stale._root.path().to_owned();
    stale.guard.shutdown().await;
    let stale_guard = SqliteRuntimeGuard::start(&stale_root, 4).await.unwrap();
    let stale_store = Arc::new(SqliteStateStore::new(stale_guard.runtime().clone()));
    let recovered_clock = clock_at("2026-09-16T01:03:00.000000Z");
    let recovered_runtime = fresh_runtime(
        stale_store.as_ref(),
        stale_topology,
        recovered_clock.as_ref(),
    )
    .await;
    let recovery = stale_store
        .recover_stale_deliveries(RecoverDeliveriesRequest {
            recovered_at: "2026-09-16T01:03:00.000000Z".parse().unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(recovery.dispatches_marked_unknown, 1);
    let (stale_client, _, stale_server) = client_with(Vec::new()).await;
    let stale_calls = Arc::new(AtomicUsize::new(0));
    let stale_adapters = telegram_registry(
        stale_topology.channel_account_id,
        stale_client,
        Arc::clone(&stale_calls),
    );
    let recovered_worker_clock: Arc<dyn Clock> = recovered_clock;
    let (stale_fatal, _) = tokio::sync::watch::channel(false);
    let mut stale_worker = start_delivery_worker(
        stale_store.clone(),
        stale_adapters,
        recovered_worker_clock,
        recovered_runtime,
        DeliveryNotifier::new(),
        FixedDeliveryJitter(1_000),
        stale_fatal,
    );
    stale_worker.wait_initial_scan().await.unwrap();
    assert_eq!(stale_calls.load(Ordering::Acquire), 0);
    assert_eq!(
        wait_for_single_delivery_state(&stale_guard, "outcome_unknown").await,
        (1, 1)
    );
    stale_worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    stale_server.await.unwrap();
    stale_store.verify_delivery_consistency().await.unwrap();
    stale_guard.shutdown().await;
}

#[tokio::test]
async fn ch6_duplicate_ingress_process_restart_is_one_message_work_and_binding() {
    let fixture = inbound_fixture().await;
    fixture
        .processor()
        .process_update(text_update(630, "one durable work"))
        .await
        .unwrap();
    let root = fixture._root.path().to_owned();
    let topology = fixture.topology;
    fixture.guard.shutdown().await;

    let guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let clock = clock_at("2026-09-16T01:02:30.000000Z");
    let processor = reopened_processor(&guard, topology, clock, DeliveryNotifier::new());
    processor
        .process_update(text_update(630, "one durable work"))
        .await
        .unwrap();
    let mut connection = guard.runtime().acquire_for_test().await.unwrap();
    for table in [
        "inbound_deliveries",
        "messages",
        "work_items",
        "conversation_bindings",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
            1,
            "unexpected duplicate in {table}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM work_items WHERE state = 'queued'",)
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    drop(connection);
    let error = processor
        .process_update(text_update(630, "contradictory material"))
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        TelegramPollerErrorKind::FatalContradictoryReplay
    );
    let mut connection = guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM work_items")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ch6_cancel_composes_scheduler_process_cleanup_ack_delivery_and_restart_replay() {
    let fixture = inbound_fixture().await;
    let topology = fixture.topology;
    let workspace = fixture._root.path().join("workspace");
    let cwd = workspace.join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    let artifact_store: Arc<dyn ArtifactStore> =
        Arc::new(LocalArtifactStore::initialize(&fixture._root.path().join("artifacts")).unwrap());
    let clock = clock_at(T1);
    let snapshot = fixture.store.load_bootstrap_snapshot().await.unwrap();
    let workstation = Arc::new(
        LocalWorkstation::new(
            &snapshot.workstation,
            &snapshot.workspace,
            LocalWorkstationOptions {
                default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
                configured_workspace_root: workspace,
                read_hard_limit: HARD_FILE_READ_MAX_BYTES,
                artifact_store,
                administrative_enabled: false,
                user_switch_launcher: None,
                credential_free_direct_execution: true,
                delegated_cgroup_root: None,
                clock: clock.clone(),
            },
        )
        .unwrap(),
    );
    assert!(
        workstation
            .capabilities_snapshot()
            .flags()
            .foreground_execute()
    );
    let process_result = Arc::new(Mutex::new(None));
    let runner = Arc::new(CancellationProcessRunner {
        workstation: Arc::clone(&workstation),
        command: "trap 'printf term > cancellation-term-observed; exit 0' TERM; \
                  /bin/sleep 300 & printf '%s' \"$!\" > cancellation-descendant.pid; \
                  printf started > cancellation-started; \
                  while :; do /bin/sleep 1; done; \
                  printf late > cancellation-late-side-effect"
            .to_owned(),
        result: Arc::clone(&process_result),
    });
    let runtime_id = fresh_runtime(fixture.store.as_ref(), topology, clock.as_ref()).await;
    let (scheduler_fatal, scheduler_fatal_rx) = tokio::sync::watch::channel(false);
    let mut scheduler = start_scheduler(
        fixture.store.clone(),
        runner,
        clock.clone(),
        fixture.health.clone(),
        scheduler_fatal,
        SchedulerStart {
            runtime_instance_id: runtime_id,
            readiness: SchedulerReadiness::RemainLiveUnready,
        },
    )
    .unwrap();
    scheduler.wait_initial_scan().await.unwrap();

    let (client, recorded, server) = client_with(vec![ResponseSpec::json(
        200,
        json!({"ok":true,"result":{"message_id":904,"date":1789516923,"chat":{"id":20002,"type":"private"}}}),
    )])
    .await;
    let delivery_calls = Arc::new(AtomicUsize::new(0));
    let adapters = telegram_registry(
        topology.channel_account_id,
        client,
        Arc::clone(&delivery_calls),
    );
    let delivery_notifier = DeliveryNotifier::new();
    let delivery_clock: Arc<dyn Clock> = clock.clone();
    let (delivery_fatal, delivery_fatal_rx) = tokio::sync::watch::channel(false);
    let mut delivery_worker = start_delivery_worker(
        fixture.store.clone(),
        adapters,
        delivery_clock,
        runtime_id,
        delivery_notifier.clone(),
        FixedDeliveryJitter(1_000),
        delivery_fatal,
    );
    delivery_worker.wait_initial_scan().await.unwrap();

    let ingress = Arc::new(
        ChannelIngressService::new(
            fixture.store.clone(),
            fixture.health.clone(),
            MutationAdmission::new(),
            scheduler.notifier(),
        )
        .with_delivery(delivery_profiles(), delivery_notifier),
    );
    let processor = TelegramInboundProcessor::new(
        topology,
        Arc::clone(&fixture.topology_service),
        ingress,
        clock.clone(),
    );
    processor
        .process_update(text_update(640, "start cancellable local process"))
        .await
        .unwrap();
    wait_for_path(&cwd.join("cancellation-started")).await;
    wait_for_work_state(&fixture.guard, "running").await;

    processor
        .process_update(text_update(641, "/cancel"))
        .await
        .unwrap();
    wait_for_work_state(&fixture.guard, "cancelled").await;
    assert_eq!(
        wait_for_single_delivery_state(&fixture.guard, "accepted").await,
        (1, 1)
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_result.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let result = process_result.lock().unwrap().clone().unwrap();
    assert_eq!(result.result_kind, ExecutionResultKind::Cancelled);
    assert!(result.cancelled);
    assert!(result.cleanup.confirmed());
    assert_eq!(
        fs::read_to_string(cwd.join("cancellation-term-observed")).unwrap(),
        "term"
    );
    assert!(!cwd.join("cancellation-late-side-effect").exists());
    let descendant = fs::read_to_string(cwd.join("cancellation-descendant.pid"))
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert_eq!(
        kill(Pid::from_raw(descendant), None),
        Err(nix::errno::Errno::ESRCH)
    );

    let mut connection = fixture.guard.runtime().acquire_for_test().await.unwrap();
    let acknowledgement = sqlx::query(
        "SELECT d.classification, o.source_kind, o.control_outcome, o.payload_text, o.state, \
                o.accepted_external_message_id \
         FROM inbound_deliveries d JOIN outbound_deliveries o \
           ON o.source_inbound_delivery_id = d.inbound_delivery_id \
         WHERE d.external_event_id = '641'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        acknowledgement
            .try_get::<String, _>("classification")
            .unwrap(),
        "control"
    );
    assert_eq!(
        acknowledgement.try_get::<String, _>("source_kind").unwrap(),
        "control"
    );
    assert_eq!(
        acknowledgement
            .try_get::<String, _>("control_outcome")
            .unwrap(),
        "applied"
    );
    assert_eq!(
        acknowledgement
            .try_get::<String, _>("payload_text")
            .unwrap(),
        "Cancellation requested."
    );
    assert_eq!(
        acknowledgement.try_get::<String, _>("state").unwrap(),
        "accepted"
    );
    assert_eq!(
        acknowledgement
            .try_get::<Option<String>, _>("accepted_external_message_id")
            .unwrap(),
        Some("904".to_owned())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE event_type IN \
             ('work.cancel_requested','work.cancelled')",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    drop(connection);
    assert_eq!(delivery_calls.load(Ordering::Acquire), 1);
    assert_eq!(recorded.lock().unwrap().len(), 1);
    let sent = serde_json::from_slice::<Value>(&recorded.lock().unwrap()[0].body).unwrap();
    assert_eq!(
        sent,
        json!({"chat_id":20002,"text":"Cancellation requested."})
    );
    assert!(!*scheduler_fatal_rx.borrow());
    assert!(!*delivery_fatal_rx.borrow());

    scheduler.stop_and_join().await.unwrap();
    delivery_worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    server.await.unwrap();
    workstation
        .shutdown_executions_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();

    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let guard = SqliteRuntimeGuard::start(&root, 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let restarted_clock = clock_at("2026-09-16T01:03:00.000000Z");
    let restarted_runtime = fresh_runtime(store.as_ref(), topology, restarted_clock.as_ref()).await;
    recover_deliveries(store.as_ref(), restarted_clock.as_ref()).await;
    let (replay_client, _, replay_server) = client_with(Vec::new()).await;
    let replay_calls = Arc::new(AtomicUsize::new(0));
    let replay_adapters = telegram_registry(
        topology.channel_account_id,
        replay_client,
        Arc::clone(&replay_calls),
    );
    let replay_notifier = DeliveryNotifier::new();
    let replay_worker_clock: Arc<dyn Clock> = restarted_clock.clone();
    let (replay_fatal, _) = tokio::sync::watch::channel(false);
    let mut replay_worker = start_delivery_worker(
        store.clone(),
        replay_adapters,
        replay_worker_clock,
        restarted_runtime,
        replay_notifier.clone(),
        FixedDeliveryJitter(1_000),
        replay_fatal,
    );
    replay_worker.wait_initial_scan().await.unwrap();
    let replay_processor = reopened_processor(&guard, topology, restarted_clock, replay_notifier);
    replay_processor
        .process_update(text_update(641, "/cancel"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(replay_calls.load(Ordering::Acquire), 0);
    let mut connection = guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM inbound_deliveries WHERE external_event_id = '641'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outbound_deliveries")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM outbound_delivery_attempts WHERE result_kind = 'accepted'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE event_type IN \
             ('work.cancel_requested','work.cancelled')",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        2
    );
    drop(connection);
    replay_worker
        .shutdown_before(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    replay_server.await.unwrap();
    store.verify_delivery_consistency().await.unwrap();
    guard.shutdown().await;
}
