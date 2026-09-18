#![cfg(unix)]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::os::fd::{AsRawFd as _, RawFd};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craxii_server::adapters::sqlite::{
    SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore,
};
use craxii_server::adapters::telegram::{TELEGRAM_PROVIDER_KEY, telegram_delivery_profile};
use craxii_server::application::channel_ingress::{
    DurableInboundOutcome, InboundAdmissionDisposition, VerifiedInboundPayload,
};
use craxii_server::application::channel_topology::ChannelTopologyService;
use craxii_server::application::runtime::bootstrap_runtime;
use craxii_server::bootstrap::{config, credential::load_credentials};
use craxii_server::domain::{
    ChannelAccountId, ChannelDispatchResult, ChannelProviderId, ClientMessageId, ContentBlock,
    CorrelationId, CraxiiId, DeliveryFailure, DeliveryFailureClass, DeliveryFailureCode,
    DiagnosticPid, ExternalAccountId, ExternalConversationId, ExternalEventId, ExternalMessageId,
    ExternalSubjectId, GitRevision, InboundDeliveryId, JournalEventId, LinuxBootId, MessageContent,
    MessageId, ModelToolCallId, OutboundDeliveryAttemptId, OutboundDeliveryId,
    OutboundDeliveryState, PackageVersion, RuntimeInstanceId, RuntimeStartEvidence,
    RuntimeStartEvidenceInput, SchemaVersion, Sha256Digest, UserId, UtcTimestamp, WorkId,
    WorkspaceId, WorkstationGeneration, WorkstationId,
};
use craxii_server::ports::channel_ingress::{
    ChannelIngressStore as _, ClassifyInboundRequest, InboundCandidates,
};
use craxii_server::ports::clock::TestClock;
use craxii_server::ports::delivery_store::{
    ClaimDeliveryRequest, DeliveryClaim, DeliveryStore as _, ListDeliverySummariesRequest,
    PersistDispatchResultRequest,
};
use craxii_server::ports::state_store::{
    BootstrapObservation, BootstrapStateStore as _, ExecutionCapabilityObservation,
    LoadOrBootstrapIdentityRequest, V0IdentityReference,
};
use serde_json::json;
use stage18_harness::{
    EstimatorMode, ProgramPlan, Stage18Harness, Stage18Root, ToolPlan, programs,
};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::fmt::writer::MakeWriter;

const LOCAL_CONFIG: &str = include_str!("fixtures/config/valid/local.toml");

#[derive(Clone)]
struct TraceCapture(Arc<Mutex<Vec<u8>>>);

impl<'writer> MakeWriter<'writer> for TraceCapture {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for TraceCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct EnvironmentGuard(Vec<(&'static str, Option<OsString>)>);

impl EnvironmentGuard {
    fn set(values: &[(&'static str, &OsStr)]) -> Self {
        let previous = values
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in values {
            // SAFETY: this integration-test binary contains one test, and it restores every
            // mutation before returning. No sibling test thread can observe partial mutation.
            unsafe { std::env::set_var(name, value) };
        }
        Self(previous)
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..) {
            // SAFETY: see `EnvironmentGuard::set`; restoration is single-threaded here.
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                } else {
                    std::env::remove_var(name);
                }
            }
        }
    }
}

fn configure_telegram_credential(root: &Path, token: &str) -> (PathBuf, PathBuf) {
    let credentials = root.join("credentials");
    fs::create_dir(&credentials).unwrap();
    fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700)).unwrap();
    for (name, value) in [
        ("openai_primary", "synthetic-model-credential"),
        ("openai_secondary", "synthetic-disabled-model-credential"),
        ("telegram_bot", token),
    ] {
        let path = credentials.join(name);
        fs::write(&path, value).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config_path = root.join("config.toml");
    let config = LOCAL_CONFIG
        .replace("/tmp/craxii-dev/credentials", credentials.to_str().unwrap())
        .replace(
            "declared = [\"openai_primary\", \"openai_secondary\"]",
            "declared = [\"openai_primary\", \"openai_secondary\", \"telegram_bot\"]",
        )
        + &format!(
            "\n[telegram]\nenabled = true\nchannel_account_id = \"{}\"\ncredential = \"telegram_bot\"\nexpected_bot_user_id = 10001\nowner_telegram_user_id = 20002\n",
            uuid::Uuid::now_v7()
        );
    fs::write(&config_path, config).unwrap();
    (credentials, config_path)
}

fn admin_config(path: &Path, root: &Stage18Root) {
    let config = LOCAL_CONFIG
        .replace(
            "state_root = \"/tmp/craxii-dev/state\"",
            &format!("state_root = {:?}", root.state_root()),
        )
        .replace(
            "artifact_root = \"/tmp/craxii-dev/state/artifacts\"",
            &format!("artifact_root = {:?}", root.artifact_root()),
        )
        .replace(
            "primary_workspace_root = \"/tmp/craxii-dev/workspaces/primary\"",
            &format!("primary_workspace_root = {:?}", root.workspace()),
        );
    fs::write(path, config).unwrap();
}

fn assert_file_absent(path: &Path, canary: &[u8]) {
    let bytes = fs::read(path).unwrap();
    assert!(
        !bytes.windows(canary.len()).any(|window| window == canary),
        "credential canary leaked to {}",
        path.display()
    );
}

fn assert_tree_absent(root: &Path, canary: &[u8]) {
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                assert_file_absent(&path, canary);
            }
        }
    }
}

fn clear_cloexec(fd: RawFd) -> i32 {
    // SAFETY: `fd` is live and owned by this test for the duration of this operation.
    let flags = unsafe { nix::libc::fcntl(fd, nix::libc::F_GETFD) };
    assert_ne!(flags, -1);
    // SAFETY: `fd` remains live, and only its close-on-exec flag is changed.
    assert_ne!(
        unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFD, flags & !nix::libc::FD_CLOEXEC) },
        -1
    );
    flags
}

fn restore_fd_flags(fd: RawFd, flags: i32) {
    // SAFETY: `fd` is still live and the saved flags came from `F_GETFD` on this descriptor.
    assert_ne!(
        unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFD, flags) },
        -1
    );
}

#[tokio::test]
async fn ch6_admin_inspects_restart_delivery_states_without_payload_or_external_ids() {
    const T0: &str = "2026-09-16T01:01:00.000000Z";
    const T1: &str = "2026-09-16T01:02:00.000000Z";

    let root = Stage18Root::new("ch6-admin-restart-state");
    let guard = SqliteRuntimeGuard::start(&root.state_root(), 4)
        .await
        .unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let created_at: UtcTimestamp = T0.parse().unwrap();
    let owner = store
        .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
            proposed: V0IdentityReference {
                craxii_id: CraxiiId::generate(),
                user_id: UserId::generate(),
                conversation_id: craxii_server::domain::ConversationId::generate(),
                workstation_id: WorkstationId::generate(),
                workspace_id: WorkspaceId::generate(),
            },
            initialized_event_id: JournalEventId::generate(),
            conversation_created_event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
            created_at,
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".to_owned(),
                os_release: "ch6-local".to_owned(),
                default_shell: "/bin/sh".to_owned(),
                workspace_logical_name: "primary".to_owned(),
                workspace_logical_root: "/workspace".to_owned(),
                workspace_resolved_root: "/workspace".to_owned(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    let topology = ChannelTopologyService::new(Arc::new(SqliteChannelIdentityStore::new(
        guard.runtime().clone(),
    )));
    let channel_account_id = ChannelAccountId::generate();
    let external_conversation = ExternalConversationId::try_new("20002").unwrap();
    let ensured = topology
        .ensure_account_and_owner_identity(
            channel_account_id,
            owner.craxii_id,
            ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY).unwrap(),
            ExternalAccountId::try_new("10001").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("20002").unwrap(),
            created_at,
        )
        .await
        .unwrap();
    topology
        .ensure_first_binding(
            channel_account_id,
            ensured.external_identity_id,
            owner.craxii_id,
            owner.user_id,
            owner.conversation_id,
            external_conversation.clone(),
            created_at,
        )
        .await
        .unwrap();

    for ordinal in 1_u8..=4 {
        let event_id = format!("operator-event-{ordinal}");
        let external_message_id = format!("operator-message-{ordinal}");
        let material = format!("operator-material-{ordinal}");
        let classified = store
            .classify_inbound(ClassifyInboundRequest {
                channel_account_id,
                external_event_id: ExternalEventId::try_new(event_id).unwrap(),
                external_message_id: Some(ExternalMessageId::try_new(external_message_id).unwrap()),
                sender_subject_id: ExternalSubjectId::try_new("20002").unwrap(),
                external_conversation_id: external_conversation.clone(),
                external_thread_id: None,
                payload: VerifiedInboundPayload::Text(
                    MessageContent::try_new(vec![ContentBlock::text("/cancel").unwrap()]).unwrap(),
                ),
                provider_occurred_at: Some(created_at),
                observed_at: created_at,
                material_digest: Sha256Digest::hash_bytes(material.as_bytes()),
                is_control: true,
                delivery_profile: Some(telegram_delivery_profile()),
                candidates: InboundCandidates {
                    inbound_delivery_id: InboundDeliveryId::generate(),
                    message_id: MessageId::generate(),
                    work_id: WorkId::generate(),
                    acceptance_event_id: JournalEventId::generate(),
                    queued_event_id: JournalEventId::generate(),
                    cancellation_event_id: JournalEventId::generate(),
                    outbound_delivery_id: OutboundDeliveryId::generate(),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            classified.result.disposition,
            InboundAdmissionDisposition::NewlyClassified
        );
        assert!(matches!(
            classified.result.outcome,
            DurableInboundOutcome::ControlNoOp { .. }
        ));
    }

    let clock = TestClock::new(
        time::OffsetDateTime::parse(T1, &time::format_description::well_known::Rfc3339).unwrap(),
        Duration::ZERO,
    );
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let runtime_id = RuntimeInstanceId::generate();
    bootstrap_runtime(
        store.as_ref(),
        RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: runtime_id,
            craxii_id: owner.craxii_id,
            workstation_id: snapshot.workstation.workstation_id(),
            workstation_generation: snapshot.workstation.generation(),
            linux_boot_id: Some(LinuxBootId::try_new("ch6-admin-first-process").unwrap()),
            diagnostic_pid: Some(DiagnosticPid::try_new(std::process::id().into()).unwrap()),
            package_version: PackageVersion::try_new("0.0.1").unwrap(),
            git_revision: GitRevision::try_new("ch6-local").unwrap(),
            schema_version: SchemaVersion::try_new(7).unwrap(),
            started_at: T1.parse().unwrap(),
        }),
        0,
        &clock,
    )
    .await
    .unwrap();

    let accepted_external_id = "accepted-external-message-canary";
    let failure_codes = [
        "retry-provider-canary",
        "unknown-provider-canary",
        "permanent-provider-canary",
    ];
    let mut payloads = Vec::new();
    for state in [
        OutboundDeliveryState::RetryWait,
        OutboundDeliveryState::OutcomeUnknown,
        OutboundDeliveryState::Accepted,
        OutboundDeliveryState::PermanentFailure,
    ] {
        let claim = store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id: runtime_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: T1.parse().unwrap(),
            })
            .await
            .unwrap();
        let DeliveryClaim::Dispatch(dispatch) = claim else {
            panic!("expected a queued delivery to be dispatchable");
        };
        payloads.push(dispatch.text.clone());
        let (result, local_retry_delay) = match state {
            OutboundDeliveryState::RetryWait => (
                ChannelDispatchResult::RetryableFailure {
                    failure: DeliveryFailure::provider(
                        DeliveryFailureClass::ProviderRetryable,
                        Some(DeliveryFailureCode::try_new(failure_codes[0]).unwrap()),
                    )
                    .unwrap(),
                    retry_after: None,
                },
                Some(Duration::from_secs(120)),
            ),
            OutboundDeliveryState::OutcomeUnknown => (
                ChannelDispatchResult::OutcomeUnknown {
                    failure: DeliveryFailure::provider(
                        DeliveryFailureClass::ProviderOutcomeUnknown,
                        Some(DeliveryFailureCode::try_new(failure_codes[1]).unwrap()),
                    )
                    .unwrap(),
                },
                None,
            ),
            OutboundDeliveryState::Accepted => (
                ChannelDispatchResult::Accepted {
                    external_message_id: Some(
                        ExternalMessageId::try_new(accepted_external_id).unwrap(),
                    ),
                },
                None,
            ),
            OutboundDeliveryState::PermanentFailure => (
                ChannelDispatchResult::PermanentFailure {
                    failure: DeliveryFailure::provider(
                        DeliveryFailureClass::ProviderPermanent,
                        Some(DeliveryFailureCode::try_new(failure_codes[2]).unwrap()),
                    )
                    .unwrap(),
                },
                None,
            ),
            _ => unreachable!(),
        };
        store
            .persist_dispatch_result(PersistDispatchResultRequest {
                dispatch,
                runtime_instance_id: runtime_id,
                result,
                local_retry_delay,
                completed_at: T1.parse().unwrap(),
            })
            .await
            .unwrap();
    }
    store.verify_delivery_consistency().await.unwrap();
    guard.shutdown().await;

    let restarted_guard = SqliteRuntimeGuard::start(&root.state_root(), 4)
        .await
        .unwrap();
    let restarted_store = SqliteStateStore::new(restarted_guard.runtime().clone());
    let restarted_snapshot = restarted_store.load_bootstrap_snapshot().await.unwrap();
    bootstrap_runtime(
        &restarted_store,
        RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: RuntimeInstanceId::generate(),
            craxii_id: owner.craxii_id,
            workstation_id: restarted_snapshot.workstation.workstation_id(),
            workstation_generation: restarted_snapshot.workstation.generation(),
            linux_boot_id: Some(LinuxBootId::try_new("ch6-admin-restarted-process").unwrap()),
            diagnostic_pid: Some(DiagnosticPid::try_new(std::process::id().into()).unwrap()),
            package_version: PackageVersion::try_new("0.0.1").unwrap(),
            git_revision: GitRevision::try_new("ch6-local").unwrap(),
            schema_version: SchemaVersion::try_new(7).unwrap(),
            started_at: T1.parse().unwrap(),
        }),
        0,
        &clock,
    )
    .await
    .unwrap();
    let summaries = restarted_store
        .list_delivery_summaries(ListDeliverySummariesRequest {
            states: vec![
                OutboundDeliveryState::RetryWait,
                OutboundDeliveryState::OutcomeUnknown,
                OutboundDeliveryState::Accepted,
                OutboundDeliveryState::PermanentFailure,
            ],
            after: None,
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(summaries.len(), 4);
    for state in [
        OutboundDeliveryState::RetryWait,
        OutboundDeliveryState::OutcomeUnknown,
        OutboundDeliveryState::Accepted,
        OutboundDeliveryState::PermanentFailure,
    ] {
        assert_eq!(summaries.iter().filter(|row| row.state == state).count(), 1);
    }
    restarted_store.verify_delivery_consistency().await.unwrap();
    restarted_guard.shutdown().await;

    let config_path = root.path().join("admin-config.toml");
    admin_config(&config_path, &root);
    let output = Command::new(env!("CARGO_BIN_EXE_craxii-admin"))
        .args([
            OsStr::new("--config"),
            config_path.as_os_str(),
            OsStr::new("delivery"),
            OsStr::new("inspect"),
            OsStr::new("--state"),
            OsStr::new("retry_wait"),
            OsStr::new("--state"),
            OsStr::new("outcome_unknown"),
            OsStr::new("--state"),
            OsStr::new("permanent_failure"),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "admin stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 4);
    let rendered_states = stdout
        .lines()
        .skip(1)
        .map(|line| line.split('\t').nth(3).unwrap())
        .collect::<Vec<_>>();
    for state in ["retry_wait", "outcome_unknown", "permanent_failure"] {
        assert_eq!(
            rendered_states
                .iter()
                .filter(|rendered| **rendered == state)
                .count(),
            1
        );
    }
    assert!(!stdout.contains("accepted\t"));
    assert!(!stdout.contains(external_conversation.as_str()));
    assert!(!stdout.contains(accepted_external_id));
    for ordinal in 1_u8..=4 {
        assert!(!stdout.contains(&format!("operator-event-{ordinal}")));
        assert!(!stdout.contains(&format!("operator-message-{ordinal}")));
    }
    for payload in payloads {
        assert!(!stdout.contains(&payload));
    }
    for code in failure_codes {
        assert!(!stdout.contains(code));
    }

    root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ch6_telegram_credential_is_absent_from_model_tool_state_artifacts_logs_admin_and_recovery()
{
    let fixture_root = std::env::temp_dir().join(format!(
        "craxii-ch6-credential-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    fs::create_dir(&fixture_root).unwrap();
    fs::set_permissions(&fixture_root, fs::Permissions::from_mode(0o700)).unwrap();
    let token = [
        "424242",
        ":",
        "CXR_FAKE_CH6_TELEGRAM_CREDENTIAL_CANARY_LOCAL_ONLY",
    ]
    .concat();
    let (credential_directory, credential_config) =
        configure_telegram_credential(&fixture_root, &token);
    let validated = config::load(&credential_config).unwrap();
    let telegram = validated.telegram().as_enabled().unwrap();
    let mut loaded = load_credentials(
        validated.credentials().source(),
        std::iter::once(telegram.credential()),
    )
    .unwrap();
    let credential = loaded.remove("telegram_bot").unwrap();
    assert_eq!(format!("{credential:?}"), "[REDACTED]");
    assert_eq!(format!("{credential}"), "[REDACTED]");

    let credential_path = credential_directory.join("telegram_bot");
    let inherited_credential = File::open(&credential_path).unwrap();
    let inherited_fd = inherited_credential.as_raw_fd();
    let old_fd_flags = clear_cloexec(inherited_fd);
    let _environment = EnvironmentGuard::set(&[
        ("CREDENTIALS_DIRECTORY", credential_directory.as_os_str()),
        ("TELEGRAM_BOT_TOKEN", OsStr::new(&token)),
    ]);

    let capture = TraceCapture(Arc::new(Mutex::new(Vec::new())));
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(capture.clone())
            .finish(),
    );
    let root = Stage18Root::new("ch6-credential-isolation");
    let user_message = "exercise the deterministic credential-isolation tool";
    let tool_call = ModelToolCallId::try_new("ch6-credential-boundary").unwrap();
    let plans = vec![
        ProgramPlan::Tools(vec![ToolPlan::new(
            tool_call.as_str(),
            "run_shell",
            json!({
                "command": "test -z \"${CREDENTIALS_DIRECTORY-}\"; test -z \"${TELEGRAM_BOT_TOKEN-}\"; for fd in /dev/fd/*; do case \"${fd##*/}\" in 0|1|2) ;; *) test ! -e \"$fd\" || exit 41 ;; esac; done; printf credential-isolated"
            }),
        )]),
        ProgramPlan::Answer {
            text: "Credential isolation confirmed by deterministic local execution.".to_owned(),
            require_tool_result: Some(tool_call),
        },
    ];
    let harness = async {
        Stage18Harness::start(root, programs(&plans), EstimatorMode::Normal)
            .await
            .unwrap()
    }
    .with_subscriber(dispatch.clone())
    .await;
    let response = harness
        .submit_message(
            user_message,
            ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().to_string()).unwrap(),
        )
        .with_subscriber(dispatch.clone())
        .await;
    assert_eq!(response.status, 202);
    let work_id = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        harness
            .wait_terminal(work_id)
            .with_subscriber(dispatch.clone())
            .await,
        "completed"
    );
    let captures = harness.provider.captures();
    assert_eq!(captures.len(), 2);
    for captured in &captures {
        let request = captured.request().canonical_bytes();
        assert!(
            !request
                .windows(token.len())
                .any(|window| window == token.as_bytes())
        );
        assert!(
            !request
                .windows(credential_path.as_os_str().as_encoded_bytes().len())
                .any(|window| window == credential_path.as_os_str().as_encoded_bytes())
        );
    }
    restore_fd_flags(inherited_fd, old_fd_flags);
    drop(inherited_credential);

    let root = harness.shutdown().with_subscriber(dispatch).await;
    let trace = capture.0.lock().unwrap().clone();
    assert!(
        !trace
            .windows(token.len())
            .any(|window| window == token.as_bytes())
    );
    assert!(
        !trace
            .windows(credential_path.as_os_str().as_encoded_bytes().len())
            .any(|window| window == credential_path.as_os_str().as_encoded_bytes())
    );
    assert_tree_absent(root.path(), token.as_bytes());

    let local_admin_config = fixture_root.join("admin-config.toml");
    admin_config(&local_admin_config, &root);
    let admin = Command::new(env!("CARGO_BIN_EXE_craxii-admin"))
        .args([
            OsStr::new("--config"),
            local_admin_config.as_os_str(),
            OsStr::new("delivery"),
            OsStr::new("inspect"),
            OsStr::new("--state"),
            OsStr::new("retry_wait"),
            OsStr::new("--state"),
            OsStr::new("outcome_unknown"),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(admin.status.success());
    assert!(
        !admin
            .stdout
            .windows(token.len())
            .any(|window| window == token.as_bytes())
    );
    assert!(
        !admin
            .stderr
            .windows(token.len())
            .any(|window| window == token.as_bytes())
    );

    let recovery_database = fixture_root.join("recovery.sqlite3");
    let recovery_manifest = fixture_root.join("recovery.manifest.json");
    let recovery_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ops/stage27/recovery-copy.py");
    let python = r#"import importlib.util,pathlib,sys
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('ch6_recovery', sys.argv[1])
module=importlib.util.module_from_spec(spec)
sys.modules['ch6_recovery']=module
spec.loader.exec_module(module)
module.create_recovery_copy(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4]), repository_sha='1'*40, service_state_reader=lambda: module.ServiceState('inactive', 0))
"#;
    let recovery = Command::new("python3")
        .args([
            OsStr::new("-c"),
            OsStr::new(python),
            recovery_script.as_os_str(),
            root.state_root().as_os_str(),
            recovery_database.as_os_str(),
            recovery_manifest.as_os_str(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        recovery.status.success(),
        "recovery stderr: {}",
        String::from_utf8_lossy(&recovery.stderr)
    );
    assert!(
        !recovery
            .stdout
            .windows(token.len())
            .any(|window| window == token.as_bytes())
    );
    assert!(
        !recovery
            .stderr
            .windows(token.len())
            .any(|window| window == token.as_bytes())
    );
    assert_file_absent(&recovery_database, token.as_bytes());
    assert_file_absent(&recovery_manifest, token.as_bytes());

    root.remove();
    fs::remove_dir_all(fixture_root).unwrap();
}
