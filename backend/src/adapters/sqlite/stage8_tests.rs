use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "test-failpoints")]
use std::sync::Arc;
#[cfg(feature = "test-failpoints")]
use std::time::Duration;

use sqlx::Row;

use crate::adapters::artifacts::LocalArtifactStore;
#[cfg(feature = "test-failpoints")]
use crate::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
use crate::adapters::sqlite::SqliteEvidenceQueryStore;
#[cfg(feature = "test-failpoints")]
#[cfg(feature = "test-failpoints")]
use crate::application::authority::{AuthorityEvaluator, V0AuthorityEvaluator};
use crate::application::evidence_inspection::{EvidenceInspectionService, EvidenceOutputFormat};
#[cfg(feature = "test-failpoints")]
use crate::application::tool_execution_service::{
    ToolExecutionCall, ToolExecutionService, ToolRuntimeLimits,
};
#[cfg(feature = "test-failpoints")]
use crate::application::tool_registry::{ToolRegistry, ToolSemanticPolicy};
use crate::domain::*;
use crate::ports::artifact_store::{ArtifactStore, BeginArtifactCapture};
#[cfg(feature = "test-failpoints")]
use crate::ports::clock::{Clock, MonotonicInstant, TestClock};
use crate::ports::evidence_query::{EvidenceQueryStore, VerificationIssue};
use crate::ports::state_store::*;
#[cfg(feature = "test-failpoints")]
use crate::ports::workstation::{HARD_FILE_READ_MAX_BYTES, Workstation};
#[cfg(feature = "test-failpoints")]
use crate::ports::workstation_preparation::WorkstationPreparation;
use crate::ports::workstation_preparation::{
    PreparedCwdEvidence, PreparedCwdObjectIdentity, PreparedCwdObjectType,
};

use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::transaction::WriteTransaction;
use super::{SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2026-08-28T01:02:03.000000Z";
const T1: &str = "2026-08-28T01:02:04.000000Z";
const T2: &str = "2026-08-28T01:02:05.000000Z";
const T3: &str = "2026-08-28T01:02:06.000000Z";
const T4: &str = "2026-08-28T01:02:07.000000Z";
const T5: &str = "2026-08-28T01:02:08.000000Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage8-test-{}-{}",
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

    #[cfg(feature = "test-failpoints")]
    fn at(path: PathBuf) -> Self {
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: SqliteStateStore,
    artifact_store: LocalArtifactStore,
    identity: V0IdentityReference,
    runtime_id: RuntimeInstanceId,
    work_id: WorkId,
    correlation_id: CorrelationId,
}

async fn fixture() -> Fixture {
    fixture_at(TestRoot::new()).await
}

async fn fixture_at(root: TestRoot) -> Fixture {
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let artifact_store = LocalArtifactStore::initialize(&root.path().join("artifacts")).unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let bootstrap = store
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
            created_at: T0.parse().unwrap(),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".to_owned(),
                os_release: "macos".to_owned(),
                default_shell: "/bin/zsh".to_owned(),
                workspace_logical_name: "primary".to_owned(),
                workspace_logical_root: "/workspace".to_owned(),
                workspace_resolved_root: "/workspace".to_owned(),
                execution_capabilities:
                    crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap();
    let runtime_id = RuntimeInstanceId::generate();
    let work_id = WorkId::generate();
    let correlation_id = CorrelationId::generate();
    store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence: RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: bootstrap.identity.craxii_id,
                workstation_id: bootstrap.identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("stage8-test-boot").unwrap()),
                diagnostic_pid: Some(DiagnosticPid::try_new(42).unwrap()),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: GitRevision::try_new("stage8-test").unwrap(),
                schema_version: SchemaVersion::try_new(4).unwrap(),
                started_at: T0.parse().unwrap(),
            }),
            event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
        })
        .await
        .unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO work_items (work_id, craxii_id, conversation_id, \
         conversation_work_ordinal, kind, state, state_version, priority, workspace_id, \
         runtime_instance_id, current_model_invocation_id, current_tool_execution_id, \
         correlation_id, created_at, queued_at, started_at, cancel_requested_at, \
         cancellation_reason_code, terminal_at, terminal_reason_code, terminal_detail_json) \
         VALUES (?, ?, ?, 1, 'conversational', 'running', 2, 0, ?, ?, NULL, NULL, ?, ?, ?, ?, \
         NULL, NULL, NULL, NULL, NULL)",
    )
    .bind(work_id.to_string())
    .bind(bootstrap.identity.craxii_id.to_string())
    .bind(bootstrap.identity.conversation_id.to_string())
    .bind(bootstrap.identity.workspace_id.to_string())
    .bind(runtime_id.to_string())
    .bind(correlation_id.to_string())
    .bind(T0)
    .bind(T0)
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    Fixture {
        _root: root,
        guard,
        store,
        artifact_store,
        identity: bootstrap.identity,
        runtime_id,
        work_id,
        correlation_id,
    }
}

async fn make_fixture_journal_consistent(fixture: &Fixture) {
    let device_id = DeviceId::generate();
    let message_id = MessageId::generate();
    let client_message_id =
        ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap();
    let message_event_id = JournalEventId::generate();
    let queued_event_id = JournalEventId::generate();
    let started_event_id = JournalEventId::generate();
    let content =
        MessageContent::try_new(vec![ContentBlock::text("fixture trigger").unwrap()]).unwrap();
    let (content_json, content_sha256) = super::codec::encode_message_content(&content).unwrap();
    let conversation_created_id = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        let id = sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM journal_events WHERE event_type = 'conversation.created'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO client_devices \
             (device_id, display_name, token_hash, created_at, last_seen_at, revoked_at) \
             VALUES (?, 'stage8-fixture', ?, ?, NULL, NULL)",
        )
        .bind(device_id.to_string())
        .bind(Sha256Digest::hash_bytes(b"stage8-fixture-token").to_string())
        .bind(T0)
        .execute(&mut *connection)
        .await
        .unwrap();
        JournalEventId::parse_canonical(&id).unwrap()
    };
    let input = WorkInputFactV1 {
        input_event_id: message_event_id,
        relationship: WorkInputRelationship::Trigger,
        ordinal_within_work: WorkInputOrdinal::try_new(1).unwrap(),
        attached_at: T0.parse().unwrap(),
        actor: WorkInputActor::User,
    };
    let mut transaction =
        WriteTransaction::begin(fixture.guard.runtime(), "stage8_fixture_journal")
            .await
            .unwrap();
    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: message_event_id,
            craxii_id: fixture.identity.craxii_id,
            stream_id: JournalStreamId::Conversation(fixture.identity.conversation_id),
            conversation_id: Some(fixture.identity.conversation_id),
            work_id: None,
            causation_event_id: Some(conversation_created_id),
            correlation_id: fixture.correlation_id,
            actor: JournalActor::User(Some(device_id)),
            runtime_instance_id: None,
            payload: JournalEventPayload::MessageAccepted(MessageCommittedV1 {
                message_id,
                craxii_id: fixture.identity.craxii_id,
                conversation_id: fixture.identity.conversation_id,
                role: MessageRole::User,
                content,
                content_sha256,
                produced_by_work_id: None,
                device_id: Some(device_id),
                client_message_id: Some(client_message_id),
                committed_at: T0.parse().unwrap(),
            }),
            recorded_at: T0.parse().unwrap(),
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (message_id, craxii_id, conversation_id, role, content_json, \
         content_sha256, produced_by_work_id, client_device_id, client_message_id, committed_at) \
         VALUES (?, ?, ?, 'user', ?, ?, NULL, ?, ?, ?)",
    )
    .bind(message_id.to_string())
    .bind(fixture.identity.craxii_id.to_string())
    .bind(fixture.identity.conversation_id.to_string())
    .bind(content_json)
    .bind(content_sha256.to_string())
    .bind(device_id.to_string())
    .bind(client_message_id.to_string())
    .bind(T0)
    .execute(transaction.connection())
    .await
    .unwrap();
    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: queued_event_id,
            craxii_id: fixture.identity.craxii_id,
            stream_id: JournalStreamId::Work(fixture.work_id),
            conversation_id: Some(fixture.identity.conversation_id),
            work_id: Some(fixture.work_id),
            causation_event_id: Some(message_event_id),
            correlation_id: fixture.correlation_id,
            actor: JournalActor::Craxii(fixture.identity.craxii_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::WorkQueued(WorkQueuedV1 {
                work_id: fixture.work_id,
                craxii_id: fixture.identity.craxii_id,
                conversation_id: fixture.identity.conversation_id,
                conversation_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                kind: WorkKind::Conversational,
                priority: 0,
                workspace_id: fixture.identity.workspace_id,
                correlation_id: fixture.correlation_id,
                state_version: ProjectionVersion::try_new(1).unwrap(),
                created_at: T0.parse().unwrap(),
                queued_at: T0.parse().unwrap(),
                trigger: input.clone(),
            }),
            recorded_at: T0.parse().unwrap(),
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_item_inputs (work_id, input_event_id, relationship, \
         ordinal_within_work, attached_at, attached_by_actor) \
         VALUES (?, ?, 'trigger', 1, ?, 'user')",
    )
    .bind(fixture.work_id.to_string())
    .bind(message_event_id.to_string())
    .bind(T0)
    .execute(transaction.connection())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE conversations SET next_work_ordinal = 2, state_version = 2 \
         WHERE conversation_id = ? AND next_work_ordinal = 1 AND state_version = 1",
    )
    .bind(fixture.identity.conversation_id.to_string())
    .execute(transaction.connection())
    .await
    .unwrap();
    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: started_event_id,
            craxii_id: fixture.identity.craxii_id,
            stream_id: JournalStreamId::Work(fixture.work_id),
            conversation_id: Some(fixture.identity.conversation_id),
            work_id: Some(fixture.work_id),
            causation_event_id: Some(queued_event_id),
            correlation_id: fixture.correlation_id,
            actor: JournalActor::Craxii(fixture.identity.craxii_id),
            runtime_instance_id: Some(fixture.runtime_id),
            payload: JournalEventPayload::WorkStarted(WorkTransitionV1 {
                work_id: fixture.work_id,
                from_state: WorkState::Queued,
                to_state: WorkState::Running,
                expected_state_version: ProjectionVersion::try_new(1).unwrap(),
                expected_runtime_owner: None,
                expected_current_attempt: JournalCurrentAttempt::None,
                expected_cancellation_reason: None,
                state_version: ProjectionVersion::try_new(2).unwrap(),
                runtime_owner: Some(fixture.runtime_id),
                current_attempt: JournalCurrentAttempt::None,
                cancellation_reason: None,
                terminal_reason: None,
                transitioned_at: T0.parse().unwrap(),
            }),
            recorded_at: T0.parse().unwrap(),
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
}

fn provider_model() -> ProviderModelReference {
    ProviderModelReference::new(
        ModelTargetId::try_new("primary").unwrap(),
        ProviderId::try_new("test-provider").unwrap(),
        ProviderModelId::try_new("test-model").unwrap(),
        TargetConfigurationVersion::try_new(1).unwrap(),
        ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(1_000).unwrap(),
            max_output_tokens: TokenCount::try_new(100).unwrap(),
        }),
    )
}

fn finalized_artifact(
    fixture: &Fixture,
    artifact_id: ArtifactId,
    producer: ArtifactProducer,
    bytes: &[u8],
    capture_limit: u64,
    logical_name: &str,
) -> PreparedArtifact {
    let mut capture = fixture
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id,
            hard_capture_limit: CanonicalByteCount::try_new(capture_limit).unwrap(),
        })
        .unwrap();
    capture.write_chunk(bytes).unwrap();
    let finalized = capture.finalize().unwrap();
    let captured_length = capture_limit.min(bytes.len() as u64);
    let digest = Sha256Digest::hash_bytes(&bytes[..captured_length as usize]);
    PreparedArtifact {
        finalized,
        metadata: ArtifactReference::new(ArtifactReferenceInput {
            artifact_id,
            craxii_id: fixture.identity.craxii_id,
            producing_work_id: Some(fixture.work_id),
            producer,
            storage_key: ArtifactStorageKey::from_digest(digest),
            sha256: digest,
            canonical_length: CanonicalByteCount::try_new(captured_length).unwrap(),
            observed_length: Some(CanonicalByteCount::try_new(bytes.len() as u64).unwrap()),
            mime_type: ArtifactMimeType::try_new("application/octet-stream").unwrap(),
            encoding: None,
            logical_name: Some(ArtifactLogicalName::try_new(logical_name).unwrap()),
            retention: ArtifactRetention::CanonicalEvidence,
            truncated: bytes.len() as u64 > captured_length,
            compression: None,
            created_at: T4.parse().unwrap(),
        }),
        event: EventIntent {
            event_id: JournalEventId::generate(),
            correlation_id: fixture.correlation_id,
            causation_event_id: None,
        },
    }
}

fn finalized_read_artifact(
    fixture: &Fixture,
    artifact_id: ArtifactId,
    tool_id: ToolExecutionId,
    bytes: &[u8],
) -> PreparedArtifact {
    let mut prepared = finalized_artifact(
        fixture,
        artifact_id,
        ArtifactProducer::Tool(tool_id),
        bytes,
        bytes.len() as u64,
        "read-file.txt",
    );
    prepared.metadata = ArtifactReference::new(ArtifactReferenceInput {
        artifact_id,
        craxii_id: fixture.identity.craxii_id,
        producing_work_id: Some(fixture.work_id),
        producer: ArtifactProducer::Tool(tool_id),
        storage_key: ArtifactStorageKey::from_digest(Sha256Digest::hash_bytes(bytes)),
        sha256: Sha256Digest::hash_bytes(bytes),
        canonical_length: CanonicalByteCount::try_new(bytes.len() as u64).unwrap(),
        observed_length: Some(CanonicalByteCount::try_new(bytes.len() as u64).unwrap()),
        mime_type: ArtifactMimeType::try_new("text/plain").unwrap(),
        encoding: Some(ArtifactEncoding::try_new("utf-8").unwrap()),
        logical_name: Some(ArtifactLogicalName::try_new("read-file.txt").unwrap()),
        retention: ArtifactRetention::CanonicalEvidence,
        truncated: false,
        compression: None,
        created_at: T4.parse().unwrap(),
    });
    prepared
}

fn final_object_path(root: &Path, digest: Sha256Digest) -> PathBuf {
    let digest = digest.to_string();
    root.join("artifacts")
        .join("sha256")
        .join(&digest[..2])
        .join(&digest)
}

async fn verified_stage8_startup(root: &Path) -> bool {
    let guard = match SqliteRuntimeGuard::start(root, 1).await {
        Ok(value) => value,
        Err(_) => return false,
    };
    let store = SqliteStateStore::new(guard.runtime().clone());
    let result = if store.verify_application_consistency().await.is_err() {
        false
    } else if let Ok(references) = store.load_referenced_artifacts().await {
        if let Ok(artifacts) = LocalArtifactStore::initialize(&root.join("artifacts")) {
            let mut keys = BTreeSet::new();
            let all_verified = references.iter().all(|artifact| {
                keys.insert(artifact.storage_key().clone());
                artifacts.verify(artifact).is_ok()
            });
            all_verified && artifacts.scan_orphans(&keys, T5.parse().unwrap()).is_ok()
        } else {
            false
        }
    } else {
        false
    };
    guard.shutdown().await;
    result
}

async fn committed_request_artifact_fixture() -> (Fixture, PathBuf) {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    begin_and_stream_model(&fixture).await;
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let path = final_object_path(fixture._root.path(), Sha256Digest::hash_bytes(b"request"));
    (fixture, path)
}

async fn completed_tool_fixture() -> (Fixture, ToolExecutionId) {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
    let dispatch_event = dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
    fixture
        .store
        .finish_tool_execution(successful_tool_completion(
            &fixture,
            tool_id,
            waiting,
            dispatch_event,
            Vec::new(),
            (None, None),
            (None, None),
        ))
        .await
        .unwrap();
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    (fixture, tool_id)
}

async fn completed_large_read_fixture() -> (Fixture, ToolExecutionId, ArtifactId) {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let arguments =
        "{\"max_bytes\":65536,\"path\":\"large.txt\",\"path_kind\":\"workspace_relative\"}";
    let (tool_id, requested_event, waiting) =
        request_named_tool(&fixture, &model, 1, 4, "read_file", arguments).await;
    let dispatch_event = dispatch_named_tool(
        &fixture,
        tool_id,
        requested_event,
        waiting,
        "read_file",
        "filesystem_read",
    )
    .await;
    let bytes = vec![b'r'; 40_000];
    let artifact_id = ArtifactId::generate();
    let artifact = finalized_read_artifact(&fixture, artifact_id, tool_id, &bytes);
    let digest = Sha256Digest::hash_bytes(&bytes);
    let mut completion = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![artifact],
        (None, None),
        (None, None),
    );
    completion.outcome.evidence_artifact_ids = vec![artifact_id];
    completion.outcome.result = Some(ToolResultEvidence {
        result_kind: ToolResultClass::Success,
        summary: "File read completed.".to_owned(),
        fields: vec![
            ("artifact_id".to_owned(), artifact_id.to_string()),
            ("byte_length".to_owned(), bytes.len().to_string()),
            ("duration_ms".to_owned(), "1".to_owned()),
            (
                "projection_omitted_bytes".to_owned(),
                bytes.len().to_string(),
            ),
            ("requested_path".to_owned(), "large.txt".to_owned()),
            (
                "resolved_path".to_owned(),
                "/workspace/large.txt".to_owned(),
            ),
            ("sha256".to_owned(), digest.to_string()),
        ],
    });
    fixture
        .store
        .finish_tool_execution(completion)
        .await
        .unwrap();
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    (fixture, tool_id, artifact_id)
}

#[tokio::test]
async fn large_read_generic_artifact_survives_sqlite_restart_without_stream_misuse() {
    let (fixture, tool_id, artifact_id) = completed_large_read_fixture().await;
    let root = fixture._root.path().to_owned();
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        let row = sqlx::query(
            "SELECT stdout_artifact_id, stderr_artifact_id, result_json \
             FROM tool_executions WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert!(row.get::<Option<String>, _>("stdout_artifact_id").is_none());
        assert!(row.get::<Option<String>, _>("stderr_artifact_id").is_none());
        assert!(
            row.get::<String, _>("result_json")
                .contains(&artifact_id.to_string())
        );
    }
    fixture.guard.shutdown().await;
    assert!(verified_stage8_startup(&root).await);

    let guard = SqliteRuntimeGuard::start(&root, 1).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let references = store.load_referenced_artifacts().await.unwrap();
    let expected_digest = Sha256Digest::hash_bytes(&vec![b'r'; 40_000]);
    let reference = references
        .iter()
        .find(|reference| reference.sha256() == expected_digest)
        .unwrap();
    assert_eq!(reference.captured_byte_count().get(), 40_000);
    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? \
             AND producer_kind = 'tool_execution' AND producer_id = ? \
             AND mime_type = 'text/plain' AND encoding = 'utf-8' \
             AND logical_name = 'read-file.txt'",
        )
        .bind(artifact_id.to_string())
        .bind(tool_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn stage23_evidence_reports_are_deterministic_and_exclude_stored_content() {
    let (fixture, _, _) = completed_large_read_fixture().await;
    let sentinels = [
        "Authorization: Bearer SENTINEL_AUTH_23",
        "SENTINEL_PROVIDER_API_KEY_23",
        "SENTINEL_REQUEST_BODY_23",
        "SENTINEL_USER_MESSAGE_23",
        "SENTINEL_MODEL_PROMPT_23",
        "SENTINEL_MODEL_OUTPUT_23",
        "SENTINEL_MODEL_REFUSAL_23",
        "SENTINEL_TOOL_ARGUMENTS_23",
        "SENTINEL_SHELL_COMMAND_23",
        "SENTINEL_STDOUT_23",
        "SENTINEL_STDERR_23",
        "SENTINEL_FILE_CONTENT_23",
        "SENTINEL_ENV_SECRET_23",
        "SENTINEL_URL_SECRET_23",
        "/Users/sentinel/absolute/path/23",
        "SENTINEL_PROVIDER_ERROR_BODY_23",
        "SENTINEL_KEYCHAIN_TOKEN_23",
    ];
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE messages SET content_json = ?")
        .bind(format!(
            "{{\"version\":1,\"blocks\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}",
            sentinels[3]
        ))
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE tool_executions SET arguments_json = ?, requested_cwd = ?")
        .bind(format!("{{\"secret\":\"{}\"}}", sentinels[7]))
        .bind(sentinels[14])
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE model_invocations SET normalized_output_json = ?")
        .bind(format!(
            "{{\"items\":[{{\"kind\":\"text\",\"text\":\"{}\"}}]}}",
            sentinels[5]
        ))
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);

    let queries = SqliteEvidenceQueryStore::new(fixture.guard.runtime().clone());
    let service = EvidenceInspectionService::new(&queries, &fixture.artifact_store);
    let first = service
        .inspect_work(fixture.work_id, EvidenceOutputFormat::Json)
        .await
        .unwrap();
    let second = service
        .inspect_work(fixture.work_id, EvidenceOutputFormat::Json)
        .await
        .unwrap();
    let markdown = service
        .inspect_work(fixture.work_id, EvidenceOutputFormat::Markdown)
        .await
        .unwrap();
    assert_eq!(first, second);
    assert!(first.contains(&fixture.work_id.to_string()));
    assert!(markdown.starts_with("# Craxii operator evidence"));
    for sentinel in sentinels {
        assert!(!first.contains(sentinel));
        assert!(!markdown.contains(sentinel));
    }
    assert!(!first.contains("payload_json"));
    assert!(!first.contains("arguments_json"));
    assert!(!first.contains("normalized_output_json"));
}

#[tokio::test]
async fn stage23_verify_state_detects_projection_and_artifact_failures_without_repair() {
    let (fixture, _, _) = completed_large_read_fixture().await;
    let queries = SqliteEvidenceQueryStore::new(fixture.guard.runtime().clone());
    let initial = queries.verify_state(&fixture.artifact_store).await.unwrap();
    assert!(initial.consistent, "{initial:?}");
    assert!(initial.referenced_artifact_count > 0);

    let storage_key: String = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query_scalar("SELECT storage_key FROM artifacts ORDER BY artifact_id LIMIT 1")
            .fetch_one(&mut *connection)
            .await
            .unwrap()
    };
    let artifact_path = fixture
        ._root
        .path()
        .join("artifacts/sha256")
        .join(&storage_key[7..9])
        .join(&storage_key[10..]);
    fs::remove_file(artifact_path).unwrap();
    let missing = queries.verify_state(&fixture.artifact_store).await.unwrap();
    assert!(!missing.consistent);
    assert!(
        missing
            .issues
            .contains(&VerificationIssue::ReferencedArtifactMissingOrCorrupt)
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE work_items SET state_version = state_version + 1 WHERE work_id = ?")
        .bind(fixture.work_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    let inconsistent = queries.verify_state(&fixture.artifact_store).await.unwrap();
    assert!(!inconsistent.consistent);
    assert!(
        inconsistent
            .issues
            .contains(&VerificationIssue::JournalProjectionInconsistent)
    );
}

#[tokio::test]
async fn foreign_generic_read_artifact_is_rejected_by_startup_consistency() {
    let (fixture, _, artifact_id) = completed_large_read_fixture().await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("UPDATE artifacts SET producer_id = ? WHERE artifact_id = ?")
            .bind(ToolExecutionId::generate().to_string())
            .bind(artifact_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);
}

fn running_expectation(fixture: &Fixture, version: i64) -> WorkExpectation {
    WorkExpectation {
        work_id: fixture.work_id,
        state: WorkState::Running,
        version: ProjectionVersion::try_new(version).unwrap(),
        runtime_owner: Some(fixture.runtime_id),
        current_attempt: CurrentWorkAttempt::None,
        cancellation_reason: None,
    }
}

fn waiting_model(fixture: &Fixture, id: ModelInvocationId, version: i64) -> WorkLifecycleSnapshot {
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id: fixture.work_id,
        state: WorkState::WaitingOnModel,
        projection_version: ProjectionVersion::try_new(version).unwrap(),
        runtime_owner: Some(fixture.runtime_id),
        current_attempt: CurrentWorkAttempt::Model(id),
        cancellation_reason: None,
        terminal_reason: None,
    })
    .unwrap()
}

fn resumed(fixture: &Fixture, version: i64) -> WorkLifecycleSnapshot {
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id: fixture.work_id,
        state: WorkState::Running,
        projection_version: ProjectionVersion::try_new(version).unwrap(),
        runtime_owner: Some(fixture.runtime_id),
        current_attempt: CurrentWorkAttempt::None,
        cancellation_reason: None,
        terminal_reason: None,
    })
    .unwrap()
}

struct BegunModel {
    invocation_id: ModelInvocationId,
    logical_id: LogicalInvocationId,
    manifest_id: ContextManifestId,
    request_artifact_id: ArtifactId,
    streaming_event_id: JournalEventId,
}

async fn begin_and_stream_model(fixture: &Fixture) -> BegunModel {
    begin_model(fixture, true).await
}

async fn begin_requesting_model(fixture: &Fixture) -> BegunModel {
    begin_model(fixture, false).await
}

async fn begin_model(fixture: &Fixture, stream: bool) -> BegunModel {
    let invocation_id = ModelInvocationId::generate();
    let logical_id = LogicalInvocationId::generate();
    let manifest_id = ContextManifestId::generate();
    let request_sha = Sha256Digest::hash_bytes(b"request");
    let artifact_id = ArtifactId::generate();
    let mut capture = fixture
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id,
            hard_capture_limit: CanonicalByteCount::try_new(7).unwrap(),
        })
        .unwrap();
    capture.write_chunk(b"request").unwrap();
    let finalized = capture.finalize().unwrap();
    let artifact_event = JournalEventId::generate();
    let started_event = JournalEventId::generate();
    let waiting_event = JournalEventId::generate();
    fixture
        .store
        .begin_model_invocation(BeginModelInvocationRequest {
            expected_work: running_expectation(fixture, 2),
            manifest: PreparedContextManifest {
                context_manifest_id: manifest_id,
                work_id: fixture.work_id,
                logical_invocation_id: logical_id,
                provider_model: provider_model(),
                assembler_version: "stage8-test-v1".to_owned(),
                context_policy_version: "stage8-test-v1".to_owned(),
                system_prompt_fingerprint: Sha256Digest::hash_bytes(b"system"),
                toolset_fingerprint: Sha256Digest::hash_bytes(b"tools"),
                eligibility_conversation_id: fixture.identity.conversation_id,
                active_work_ordinal: 1,
                highest_prior_terminal_work_ordinal: None,
                input_event_ids: Vec::new(),
                active_output_record_ids: Vec::new(),
                maximum_journal_offset: JournalOffset::try_new(2).unwrap(),
                canonical_byte_count: CanonicalByteCount::try_new(7).unwrap(),
                rendered_request_byte_count: CanonicalByteCount::try_new(7).unwrap(),
                estimated_input_tokens: 10,
                token_estimator_id: "test-estimator-v1".to_owned(),
                context_window_tokens: 1_000,
                reserved_output_tokens: 100,
                utilization_basis_points: 1_100,
                manifest_sha256: Sha256Digest::hash_bytes(b"manifest"),
                rendered_request_sha256: request_sha,
                rendered_request_artifact_id: Some(artifact_id),
                omitted_source_count: 0,
                transformed_source_count: 0,
                sources: Vec::new(),
                created_at: T1.parse().unwrap(),
            },
            invocation: PreparedModelInvocation {
                attempt: ModelAttemptReference::new(ModelAttemptReferenceInput {
                    logical_invocation_id: logical_id,
                    model_invocation_id: invocation_id,
                    work_id: fixture.work_id,
                    runtime_instance_id: fixture.runtime_id,
                    context_manifest_id: manifest_id,
                    agent_step_no: AgentStepNo::try_new(1).unwrap(),
                    attempt_no: AttemptNo::try_new(1).unwrap(),
                    provider_model: provider_model(),
                    retry_of: None,
                }),
                selection_reason: ModelSelectionReason::ConfiguredDefault,
                required_capabilities: RequiredModelCapabilities {
                    text_input: true,
                    text_output: true,
                    custom_tool_calling: true,
                    streaming: true,
                    ordered_output_items: true,
                    structured_output: false,
                    reasoning_continuation: false,
                },
                provider_options: vec![ProviderOption {
                    key: "temperature".to_owned(),
                    value: ProviderOptionValue::Integer(0),
                }],
                request_sha256: request_sha,
                request_artifact_id: Some(artifact_id),
                retry_evidence: None,
                started_at: T1.parse().unwrap(),
            },
            artifacts: vec![PreparedArtifact {
                finalized,
                metadata: ArtifactReference::new(ArtifactReferenceInput {
                    artifact_id,
                    craxii_id: fixture.identity.craxii_id,
                    producing_work_id: Some(fixture.work_id),
                    producer: ArtifactProducer::Model(invocation_id),
                    storage_key: ArtifactStorageKey::from_digest(request_sha),
                    sha256: request_sha,
                    canonical_length: CanonicalByteCount::try_new(7).unwrap(),
                    observed_length: Some(CanonicalByteCount::try_new(7).unwrap()),
                    mime_type: ArtifactMimeType::try_new("application/json").unwrap(),
                    encoding: Some(ArtifactEncoding::try_new("utf-8").unwrap()),
                    logical_name: Some(ArtifactLogicalName::try_new("model-request").unwrap()),
                    retention: ArtifactRetention::CanonicalEvidence,
                    truncated: false,
                    compression: None,
                    created_at: T1.parse().unwrap(),
                }),
                event: EventIntent {
                    event_id: artifact_event,
                    correlation_id: fixture.correlation_id,
                    causation_event_id: None,
                },
            }],
            work_next: waiting_model(fixture, invocation_id, 3),
            invocation_event: EventIntent {
                event_id: started_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: None,
            },
            work_event: EventIntent {
                event_id: waiting_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(started_event),
            },
        })
        .await
        .unwrap();

    let streaming_event_id = if stream {
        let streaming_event_id = JournalEventId::generate();
        fixture
            .store
            .mark_model_streaming(MarkModelStreamingRequest {
                expected_work: WorkExpectation {
                    work_id: fixture.work_id,
                    state: WorkState::WaitingOnModel,
                    version: ProjectionVersion::try_new(3).unwrap(),
                    runtime_owner: Some(fixture.runtime_id),
                    current_attempt: CurrentWorkAttempt::Model(invocation_id),
                    cancellation_reason: None,
                },
                expected_model: ModelExpectation {
                    model_invocation_id: invocation_id,
                    state: ModelInvocationState::Requesting,
                },
                observation: ModelStreamingObservation {
                    first_byte_at: T2.parse().unwrap(),
                    first_output_at: Some(T2.parse().unwrap()),
                    provider_request_id: Some("request-1".to_owned()),
                    provider_response_id: None,
                    draft_exposed: false,
                },
                event: EventIntent {
                    event_id: streaming_event_id,
                    correlation_id: fixture.correlation_id,
                    causation_event_id: Some(started_event),
                },
            })
            .await
            .unwrap();
        streaming_event_id
    } else {
        started_event
    };
    BegunModel {
        invocation_id,
        logical_id,
        manifest_id,
        request_artifact_id: artifact_id,
        streaming_event_id,
    }
}

fn model_completion_request(
    fixture: &Fixture,
    model: &BegunModel,
    artifacts: Vec<PreparedArtifact>,
    response_artifact_id: Option<ArtifactId>,
) -> FinishModelInvocationRequest {
    let completed_event = JournalEventId::generate();
    FinishModelInvocationRequest {
        expected_work: WorkExpectation {
            work_id: fixture.work_id,
            state: WorkState::WaitingOnModel,
            version: ProjectionVersion::try_new(3).unwrap(),
            runtime_owner: Some(fixture.runtime_id),
            current_attempt: CurrentWorkAttempt::Model(model.invocation_id),
            cancellation_reason: None,
        },
        expected_model: ModelExpectation {
            model_invocation_id: model.invocation_id,
            state: ModelInvocationState::Streaming,
        },
        outcome: ModelTerminalOutcome {
            state: ModelInvocationState::Completed,
            response_sha256: Some(Sha256Digest::hash_bytes(b"response")),
            response_artifact_id,
            normalized_output: Some(NormalizedModelOutput {
                items: vec![NormalizedModelOutputItem::Text {
                    text: "done".to_owned(),
                }],
            }),
            provider_request_id: Some("request-1".to_owned()),
            provider_response_id: Some("response-1".to_owned()),
            first_byte_at: Some(T2.parse().unwrap()),
            first_output_at: Some(T2.parse().unwrap()),
            completed_at: T3.parse().unwrap(),
            usage: Some(ModelUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 5,
                reasoning_tokens: 0,
                total_tokens: 15,
            }),
            usage_status: crate::ports::model_provider::ModelUsageStatus::Reported,
            provider_error_kind: None,
            provider_outcome_certainty:
                crate::ports::model_provider::ProviderOutcomeCertainty::DefinitelyCompleted,
            billing_ambiguity: false,
            stop_reason: Some("stop".to_owned()),
            tool_call_count: Some(0),
            draft_exposed: false,
            normalized_error: None,
        },
        artifacts,
        work_next: resumed(fixture, 4),
        model_event: EventIntent {
            event_id: completed_event,
            correlation_id: fixture.correlation_id,
            causation_event_id: Some(model.streaming_event_id),
        },
        work_event: EventIntent {
            event_id: JournalEventId::generate(),
            correlation_id: fixture.correlation_id,
            causation_event_id: Some(completed_event),
        },
    }
}

async fn complete_model(fixture: &Fixture, model: &BegunModel) {
    fixture
        .store
        .finish_model_invocation(model_completion_request(fixture, model, Vec::new(), None))
        .await
        .unwrap();
}

fn retry_model_request(
    fixture: &Fixture,
    model: &BegunModel,
    attempt_no: i64,
) -> (ModelInvocationId, BeginModelInvocationRequest) {
    let invocation_id = ModelInvocationId::generate();
    let artifact_id = ArtifactId::generate();
    let request_sha = Sha256Digest::hash_bytes(b"request");
    let mut capture = fixture
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id,
            hard_capture_limit: CanonicalByteCount::try_new(7).unwrap(),
        })
        .unwrap();
    capture.write_chunk(b"request").unwrap();
    let finalized = capture.finalize().unwrap();
    let started_event = JournalEventId::generate();
    (
        invocation_id,
        BeginModelInvocationRequest {
            expected_work: running_expectation(fixture, 4),
            manifest: PreparedContextManifest {
                context_manifest_id: model.manifest_id,
                work_id: fixture.work_id,
                logical_invocation_id: model.logical_id,
                provider_model: provider_model(),
                assembler_version: "stage8-test-v1".to_owned(),
                context_policy_version: "stage8-test-v1".to_owned(),
                system_prompt_fingerprint: Sha256Digest::hash_bytes(b"system"),
                toolset_fingerprint: Sha256Digest::hash_bytes(b"tools"),
                eligibility_conversation_id: fixture.identity.conversation_id,
                active_work_ordinal: 1,
                highest_prior_terminal_work_ordinal: None,
                input_event_ids: Vec::new(),
                active_output_record_ids: Vec::new(),
                maximum_journal_offset: JournalOffset::try_new(2).unwrap(),
                canonical_byte_count: CanonicalByteCount::try_new(7).unwrap(),
                rendered_request_byte_count: CanonicalByteCount::try_new(7).unwrap(),
                estimated_input_tokens: 10,
                token_estimator_id: "test-estimator-v1".to_owned(),
                context_window_tokens: 1_000,
                reserved_output_tokens: 100,
                utilization_basis_points: 1_100,
                manifest_sha256: Sha256Digest::hash_bytes(b"manifest"),
                rendered_request_sha256: request_sha,
                rendered_request_artifact_id: Some(model.request_artifact_id),
                omitted_source_count: 0,
                transformed_source_count: 0,
                sources: Vec::new(),
                created_at: T1.parse().unwrap(),
            },
            invocation: PreparedModelInvocation {
                attempt: ModelAttemptReference::new(ModelAttemptReferenceInput {
                    logical_invocation_id: model.logical_id,
                    model_invocation_id: invocation_id,
                    work_id: fixture.work_id,
                    runtime_instance_id: fixture.runtime_id,
                    context_manifest_id: model.manifest_id,
                    agent_step_no: AgentStepNo::try_new(1).unwrap(),
                    attempt_no: AttemptNo::try_new(attempt_no).unwrap(),
                    provider_model: provider_model(),
                    retry_of: Some(model.invocation_id),
                }),
                selection_reason: ModelSelectionReason::ConfiguredDefault,
                required_capabilities: RequiredModelCapabilities {
                    text_input: true,
                    text_output: true,
                    custom_tool_calling: true,
                    streaming: true,
                    ordered_output_items: true,
                    structured_output: false,
                    reasoning_continuation: false,
                },
                provider_options: vec![ProviderOption {
                    key: "temperature".to_owned(),
                    value: ProviderOptionValue::Integer(0),
                }],
                request_sha256: request_sha,
                request_artifact_id: Some(artifact_id),
                retry_evidence: Some(crate::ports::model_provider::ProviderRetryEvidence {
                    reason: crate::ports::model_provider::RetryReasonCode::ClassifiedTransientBeforeOutput,
                    delay: std::time::Duration::ZERO,
                    provider_retry_after: None,
                }),
                started_at: T4.parse().unwrap(),
            },
            artifacts: vec![PreparedArtifact {
                finalized,
                metadata: ArtifactReference::new(ArtifactReferenceInput {
                    artifact_id,
                    craxii_id: fixture.identity.craxii_id,
                    producing_work_id: Some(fixture.work_id),
                    producer: ArtifactProducer::Model(invocation_id),
                    storage_key: ArtifactStorageKey::from_digest(request_sha),
                    sha256: request_sha,
                    canonical_length: CanonicalByteCount::try_new(7).unwrap(),
                    observed_length: Some(CanonicalByteCount::try_new(7).unwrap()),
                    mime_type: ArtifactMimeType::try_new("application/json").unwrap(),
                    encoding: Some(ArtifactEncoding::try_new("utf-8").unwrap()),
                    logical_name: Some(
                        ArtifactLogicalName::try_new("model-retry-request").unwrap(),
                    ),
                    retention: ArtifactRetention::CanonicalEvidence,
                    truncated: false,
                    compression: None,
                    created_at: T4.parse().unwrap(),
                }),
                event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id: fixture.correlation_id,
                    causation_event_id: None,
                },
            }],
            work_next: waiting_model(fixture, invocation_id, 5),
            invocation_event: EventIntent {
                event_id: started_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: None,
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(started_event),
            },
        },
    )
}

async fn request_tool(
    fixture: &Fixture,
    model: &BegunModel,
    tool_ordinal: i64,
    running_version: i64,
) -> (ToolExecutionId, JournalEventId, WorkExpectation) {
    request_named_tool(
        fixture,
        model,
        tool_ordinal,
        running_version,
        "run-shell",
        "{\"command\":\"true\"}",
    )
    .await
}

async fn request_named_tool(
    fixture: &Fixture,
    model: &BegunModel,
    tool_ordinal: i64,
    running_version: i64,
    tool_name: &str,
    arguments_json: &str,
) -> (ToolExecutionId, JournalEventId, WorkExpectation) {
    let tool_id = ToolExecutionId::generate();
    let requested_event = JournalEventId::generate();
    let waiting_version = running_version + 1;
    fixture
        .store
        .request_tool_execution(RequestToolExecutionRequest {
            expected_work: running_expectation(fixture, running_version),
            tool: PreparedToolExecution {
                lifecycle: ToolLifecycleReference::new(
                    tool_id,
                    ExecutionId::generate(),
                    fixture.work_id,
                    fixture.runtime_id,
                    model.invocation_id,
                    AgentStepNo::try_new(1).unwrap(),
                    ToolOrdinal::try_new(tool_ordinal).unwrap(),
                ),
                provider_tool_call_id: Some(format!("call-{tool_ordinal}")),
                tool_name: ToolName::try_new(tool_name).unwrap(),
                tool_version: ToolVersion::try_new("1.0.0").unwrap(),
                tool_schema_version: 1,
                arguments_json: arguments_json.to_owned(),
                arguments_sha256: Sha256Digest::hash_bytes(arguments_json.as_bytes()),
                workstation_id: fixture.identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                workspace_id: fixture.identity.workspace_id,
                requested_cwd: LogicalPathReference::absolute("/workspace").unwrap(),
                requested_privilege: PrivilegeMode::User,
                timeout_ms: 1_000,
                output_policy: ToolOutputPolicy {
                    stdout_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    stderr_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    combined_inline_limit: CanonicalByteCount::try_new(50).unwrap(),
                    per_stream_inline_limit: CanonicalByteCount::try_new(25).unwrap(),
                },
                requested_at: T3.parse().unwrap(),
            },
            work_next: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id: fixture.work_id,
                state: WorkState::WaitingOnTool,
                projection_version: ProjectionVersion::try_new(waiting_version).unwrap(),
                runtime_owner: Some(fixture.runtime_id),
                current_attempt: CurrentWorkAttempt::Tool(tool_id),
                cancellation_reason: None,
                terminal_reason: None,
            })
            .unwrap(),
            tool_event: EventIntent {
                event_id: requested_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: None,
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(requested_event),
            },
        })
        .await
        .unwrap();
    (
        tool_id,
        requested_event,
        WorkExpectation {
            work_id: fixture.work_id,
            state: WorkState::WaitingOnTool,
            version: ProjectionVersion::try_new(waiting_version).unwrap(),
            runtime_owner: Some(fixture.runtime_id),
            current_attempt: CurrentWorkAttempt::Tool(tool_id),
            cancellation_reason: None,
        },
    )
}

async fn dispatch_tool(
    fixture: &Fixture,
    tool_id: ToolExecutionId,
    requested_event: JournalEventId,
    waiting: WorkExpectation,
) -> JournalEventId {
    dispatch_named_tool(
        fixture,
        tool_id,
        requested_event,
        waiting,
        "run_shell",
        "foreground_execute",
    )
    .await
}

async fn dispatch_named_tool(
    fixture: &Fixture,
    tool_id: ToolExecutionId,
    requested_event: JournalEventId,
    waiting: WorkExpectation,
    tool_name: &str,
    required_capability: &str,
) -> JournalEventId {
    let dispatch_event = JournalEventId::generate();
    let authority = AuthorityDecisionSnapshot::new(
        AuthorityDecision::Allow,
        PrivilegeMode::User,
        AuthorityReasonCode::try_new("allowed").unwrap(),
    );
    let prepared_cwd = fixture_prepared_cwd(fixture);
    let authority_evidence_json =
        authority_evidence(fixture, tool_name, required_capability, &prepared_cwd);
    assert_eq!(
        serde_json::to_string(
            &serde_json::from_str::<serde_json::Value>(&authority_evidence_json).unwrap()
        )
        .unwrap(),
        authority_evidence_json
    );
    assert!(
        super::stage8_codec::validate_dispatch_evidence(
            &authority_evidence_json,
            &authority,
            &prepared_cwd,
        )
        .is_ok()
    );
    fixture
        .store
        .commit_tool_dispatch_intent(CommitToolDispatchIntentRequest {
            expected_work: waiting,
            expected_tool: ToolExpectation {
                tool_execution_id: tool_id,
                state: ToolExecutionState::Requested,
            },
            dispatch: ToolDispatchIntent {
                authority,
                dispatch_evidence_json: authority_evidence_json,
                effective_privilege: PrivilegeMode::User,
                prepared_cwd,
                timeout_ms: 1_000,
                output_policy: ToolOutputPolicy {
                    stdout_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    stderr_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    combined_inline_limit: CanonicalByteCount::try_new(50).unwrap(),
                    per_stream_inline_limit: CanonicalByteCount::try_new(25).unwrap(),
                },
                dispatch_intent_at: T4.parse().unwrap(),
            },
            event: EventIntent {
                event_id: dispatch_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(requested_event),
            },
        })
        .await
        .unwrap();
    dispatch_event
}

fn fixture_prepared_cwd(fixture: &Fixture) -> PreparedCwdEvidence {
    PreparedCwdEvidence::new(
        ResolvedPathEvidence::try_new(
            fixture.identity.workstation_id,
            WorkstationGeneration::try_new(1).unwrap(),
            fixture.identity.workspace_id,
            LogicalPathReference::absolute("/workspace").unwrap(),
            "/workspace",
        )
        .unwrap(),
        PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory).unwrap(),
    )
}

fn authority_evidence(
    fixture: &Fixture,
    tool_name: &str,
    required_capability: &str,
    prepared_cwd: &PreparedCwdEvidence,
) -> String {
    let resolved = prepared_cwd.resolved_cwd();
    let identity = prepared_cwd.object_identity();
    serde_json::to_string(&serde_json::json!({
        "arguments_sha256": Sha256Digest::hash_bytes(b"{}").to_string(),
        "authority_facts": {
            "administrative_allowed": true,
            "authority_widening_attempt": false,
            "malformed_arguments": false,
            "requested_stderr_bytes": 100,
            "requested_stdout_bytes": 100,
            "requested_timeout_ms": 1000,
            "tool_allowed": true,
            "work_cancelled": false
        },
        "canonical_argument_bytes": 2,
        "capabilities": {
            "cancel_execution": true,
            "filesystem_read": true,
            "foreground_execute": true,
            "inspect_execution": true,
            "max_execution_timeout_ms": 900000,
            "max_stderr_bytes": 8388608,
            "max_stdout_bytes": 8388608,
            "privilege_administrative": false,
            "privilege_user": true,
            "workspace_present": true
        },
        "craxii_id": fixture.identity.craxii_id.to_string(),
        "decision": "allow",
        "effective_privilege": "user",
        "policy": "v0-development-workstation",
        "prepared_cwd": {
            "device": identity.device(),
            "inode": identity.inode(),
            "object_type": "directory",
            "requested_cwd": resolved.requested_path().canonical(),
            "resolved_cwd": resolved.resolved_absolute_path(),
            "version": 1,
            "workspace_id": resolved.workspace_id().to_string(),
            "workstation_generation": resolved.workstation_generation().get(),
            "workstation_id": resolved.workstation_id().to_string()
        },
        "reason_code": "allowed",
        "required_capability": required_capability,
        "requested_privilege": "user",
        "runtime_instance_id": fixture.runtime_id.to_string(),
        "schema_version": 1,
        "tool_name": tool_name,
        "tool_version": "1.0.0",
        "version": 2,
        "work_id": fixture.work_id.to_string(),
        "workspace_id": fixture.identity.workspace_id.to_string(),
        "workstation_generation": 1,
        "workstation_id": fixture.identity.workstation_id.to_string()
    }))
    .unwrap()
}

fn successful_tool_completion(
    fixture: &Fixture,
    tool_id: ToolExecutionId,
    waiting: WorkExpectation,
    dispatch_event: JournalEventId,
    artifacts: Vec<PreparedArtifact>,
    (stdout_artifact_id, stdout_counts): (Option<ArtifactId>, Option<ToolStreamCounts>),
    (stderr_artifact_id, stderr_counts): (Option<ArtifactId>, Option<ToolStreamCounts>),
) -> FinishToolExecutionRequest {
    let tool_event = JournalEventId::generate();
    FinishToolExecutionRequest {
        expected_work: waiting,
        expected_tool: ToolExpectation {
            tool_execution_id: tool_id,
            state: ToolExecutionState::Dispatching,
        },
        outcome: ToolTerminalOutcome {
            state: ToolExecutionState::Completed,
            predispatch_authority: None,
            started_at: Some(T4.parse().unwrap()),
            completed_at: T5.parse().unwrap(),
            exit_code: Some(0),
            signal: None,
            timed_out: Some(false),
            cancelled: Some(false),
            cleanup_confirmed: None,
            result: Some(ToolResultEvidence {
                result_kind: ToolResultClass::Success,
                summary: "completed".to_owned(),
                fields: Vec::new(),
            }),
            evidence_artifact_ids: Vec::new(),
            stdout_artifact_id,
            stderr_artifact_id,
            stdout_counts,
            stderr_counts,
            truncated: stdout_counts.is_some_and(|value| value.observed > value.captured)
                || stderr_counts.is_some_and(|value| value.observed > value.captured),
            normalized_error: None,
        },
        artifacts,
        work_next: resumed(fixture, 6),
        tool_event: EventIntent {
            event_id: tool_event,
            correlation_id: fixture.correlation_id,
            causation_event_id: Some(dispatch_event),
        },
        work_event: EventIntent {
            event_id: JournalEventId::generate(),
            correlation_id: fixture.correlation_id,
            causation_event_id: Some(tool_event),
        },
    }
}

#[tokio::test]
async fn committed_artifact_metadata_and_bytes_survive_artifact_root_relocation() {
    let (fixture, _) = committed_request_artifact_fixture().await;
    let old_root = fixture._root.path().to_owned();
    let storage_key = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query_scalar::<_, String>("SELECT storage_key FROM artifacts")
            .fetch_one(&mut *connection)
            .await
            .unwrap()
    };
    assert!(!storage_key.starts_with('/'));
    assert!(!storage_key.contains(old_root.to_str().unwrap()));
    fixture.guard.shutdown().await;

    let relocated = TestRoot::new();
    fs::rename(old_root.join("db"), relocated.path().join("db")).unwrap();
    fs::rename(
        old_root.join("artifacts"),
        relocated.path().join("artifacts"),
    )
    .unwrap();
    assert!(verified_stage8_startup(relocated.path()).await);

    let guard = SqliteRuntimeGuard::start(relocated.path(), 1)
        .await
        .unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let references = store.load_referenced_artifacts().await.unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].storage_key().as_str(), storage_key);
    let artifacts = LocalArtifactStore::initialize(&relocated.path().join("artifacts")).unwrap();
    assert_eq!(artifacts.read_verified(&references[0]).unwrap(), b"request");
    guard.shutdown().await;
}

#[tokio::test]
async fn referenced_missing_corrupt_truncated_unsafe_linked_or_symlinked_objects_fail_startup() {
    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    fs::remove_file(&path).unwrap();
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    fs::write(&path, b"corrupt").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(3).unwrap();
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let sibling = path.with_extension("hardlink");
    fs::hard_link(&path, &sibling).unwrap();
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, path) = committed_request_artifact_fixture().await;
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let target = path.with_extension("target");
    fs::write(&target, b"request").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(!verified_stage8_startup(&root).await);
}

#[tokio::test]
async fn producer_work_model_tool_and_reference_mismatches_fail_startup_consistency() {
    let fixture = self::fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (retry_id, retry) = retry_model_request(&fixture, &model, 2);
    fixture.store.begin_model_invocation(retry).await.unwrap();
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("UPDATE artifacts SET producer_id = ? WHERE artifact_id = ?")
            .bind(retry_id.to_string())
            .bind(model.request_artifact_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);

    let fixture = self::fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (_, retry) = retry_model_request(&fixture, &model, 2);
    let retry_artifact_id = retry.invocation.request_artifact_id.unwrap();
    fixture.store.begin_model_invocation(retry).await.unwrap();
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "UPDATE model_invocations SET response_sha256 = ?, response_artifact_id = ? \
             WHERE model_invocation_id = ?",
        )
        .bind(Sha256Digest::hash_bytes(b"request").to_string())
        .bind(retry_artifact_id.to_string())
        .bind(model.invocation_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);

    let fixture = self::fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    begin_and_stream_model(&fixture).await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE artifacts SET producing_work_id = ?")
            .bind(WorkId::generate().to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);

    let fixture = self::fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
    let dispatch_event = dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
    let stdout_id = ArtifactId::generate();
    let stdout = finalized_artifact(
        &fixture,
        stdout_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "stdout",
    );
    let counts = ToolStreamCounts {
        observed: CanonicalByteCount::try_new(3).unwrap(),
        captured: CanonicalByteCount::try_new(3).unwrap(),
        returned_inline: CanonicalByteCount::try_new(0).unwrap(),
        omitted: CanonicalByteCount::try_new(3).unwrap(),
    };
    fixture
        .store
        .finish_tool_execution(successful_tool_completion(
            &fixture,
            tool_id,
            waiting,
            dispatch_event,
            vec![stdout],
            (Some(stdout_id), Some(counts)),
            (None, None),
        ))
        .await
        .unwrap();
    let (other_tool_id, _, _) = request_tool(&fixture, &model, 2, 6).await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("UPDATE artifacts SET producer_id = ? WHERE artifact_id = ?")
            .bind(other_tool_id.to_string())
            .bind(stdout_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);
}

#[tokio::test]
async fn requested_cwd_is_concrete_not_null_canonical_and_matches_dispatch_evidence() {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        let column = sqlx::query(
            "SELECT [notnull] FROM pragma_table_info('tool_executions') WHERE name = 'requested_cwd'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(column.get::<i64, _>("notnull"), 1);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT requested_cwd FROM tool_executions WHERE tool_execution_id = ?",
            )
            .bind(tool_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
            "/workspace"
        );
        assert!(
            sqlx::query(
                "UPDATE tool_executions SET requested_cwd = NULL WHERE tool_execution_id = ?"
            )
            .bind(tool_id.to_string())
            .execute(&mut *connection)
            .await
            .is_err()
        );
    }
    dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;

    let fixture = self::fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (tool_id, _, _) = request_tool(&fixture, &model, 1, 4).await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "UPDATE tool_executions SET requested_cwd = 'src/../bin' WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);
}

#[tokio::test]
async fn tool_result_corruption_rejects_unknown_missing_required_and_forbidden_outer_fields() {
    let (fixture, tool_id) = completed_tool_fixture().await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "UPDATE tool_executions SET result_json = \
             '{\"version\":1,\"result_kind\":\"unknown_kind\",\"summary\":\"bad\",\"fields\":[]}' \
             WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, tool_id) = completed_tool_fixture().await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "UPDATE tool_executions SET result_json = \
             '{\"version\":1,\"result_kind\":\"process_exit\",\"summary\":\"bad\",\"fields\":[]}', \
             exit_code = NULL WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);

    let (fixture, tool_id) = completed_tool_fixture().await;
    {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("UPDATE tool_executions SET signal = 9 WHERE tool_execution_id = ?")
            .bind(tool_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    assert!(!verified_stage8_startup(&root).await);
}

#[tokio::test]
async fn named_model_transactions_reject_nonexact_or_incoherent_artifact_sets_before_insert() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;

    let (missing_id, mut missing) = retry_model_request(&fixture, &model, 2);
    missing.artifacts.clear();
    assert!(fixture.store.begin_model_invocation(missing).await.is_err());

    let (extra_id, mut extra) = retry_model_request(&fixture, &model, 2);
    extra.artifacts.push(finalized_artifact(
        &fixture,
        ArtifactId::generate(),
        ArtifactProducer::Model(extra_id),
        b"extra",
        5,
        "extra-model-request",
    ));
    assert!(fixture.store.begin_model_invocation(extra).await.is_err());

    let (_, mut duplicate) = retry_model_request(&fixture, &model, 2);
    duplicate.artifacts.push(duplicate.artifacts[0].clone());
    assert!(
        fixture
            .store
            .begin_model_invocation(duplicate)
            .await
            .is_err()
    );

    let (_, mut count_mismatch) = retry_model_request(&fixture, &model, 2);
    count_mismatch.manifest.rendered_request_byte_count = CanonicalByteCount::try_new(6).unwrap();
    assert!(
        fixture
            .store
            .begin_model_invocation(count_mismatch)
            .await
            .is_err()
    );

    let (_, mut digest_mismatch) = retry_model_request(&fixture, &model, 2);
    digest_mismatch.invocation.request_sha256 = Sha256Digest::hash_bytes(b"wrong");
    assert!(
        fixture
            .store
            .begin_model_invocation(digest_mismatch)
            .await
            .is_err()
    );

    let (_, mut missing_context_source) = retry_model_request(&fixture, &model, 2);
    missing_context_source
        .manifest
        .sources
        .push(PreparedContextSource {
            position: 1,
            kind: ContextSourceKind::ArtifactContent,
            identity: ContextSourceIdentity::Artifact(ArtifactId::generate()),
            model_role: Some(ContextModelRole::User),
            item_class: Some("artifact".to_owned()),
            source_content_sha256: Sha256Digest::hash_bytes(b"missing"),
            rendered_byte_contribution: CanonicalByteCount::try_new(7).unwrap(),
            transform: ContextTransformKind::Identity,
            transformed: false,
        });
    assert!(
        fixture
            .store
            .begin_model_invocation(missing_context_source)
            .await
            .is_err()
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM model_invocations")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts WHERE producer_id IN (?, ?)",)
            .bind(missing_id.to_string())
            .bind(extra_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn model_terminal_rejects_missing_extra_duplicate_and_digest_mismatched_artifacts() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;

    let missing =
        model_completion_request(&fixture, &model, Vec::new(), Some(ArtifactId::generate()));
    assert!(
        fixture
            .store
            .finish_model_invocation(missing)
            .await
            .is_err()
    );

    let extra_artifact = finalized_artifact(
        &fixture,
        ArtifactId::generate(),
        ArtifactProducer::Model(model.invocation_id),
        b"response",
        8,
        "extra-model-response",
    );
    let extra = model_completion_request(&fixture, &model, vec![extra_artifact], None);
    assert!(fixture.store.finish_model_invocation(extra).await.is_err());

    let duplicate_id = ArtifactId::generate();
    let duplicate_artifact = finalized_artifact(
        &fixture,
        duplicate_id,
        ArtifactProducer::Model(model.invocation_id),
        b"response",
        8,
        "duplicate-model-response",
    );
    let duplicate = model_completion_request(
        &fixture,
        &model,
        vec![duplicate_artifact.clone(), duplicate_artifact],
        Some(duplicate_id),
    );
    assert!(
        fixture
            .store
            .finish_model_invocation(duplicate)
            .await
            .is_err()
    );

    let wrong_digest_id = ArtifactId::generate();
    let wrong_digest_artifact = finalized_artifact(
        &fixture,
        wrong_digest_id,
        ArtifactProducer::Model(model.invocation_id),
        b"not-response",
        12,
        "wrong-model-response",
    );
    let wrong_digest = model_completion_request(
        &fixture,
        &model,
        vec![wrong_digest_artifact],
        Some(wrong_digest_id),
    );
    assert!(
        fixture
            .store
            .finish_model_invocation(wrong_digest)
            .await
            .is_err()
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE event_type = 'artifact.recorded'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    drop(connection);

    let valid_id = ArtifactId::generate();
    let valid_artifact = finalized_artifact(
        &fixture,
        valid_id,
        ArtifactProducer::Model(model.invocation_id),
        b"response",
        8,
        "model-response",
    );
    fixture
        .store
        .finish_model_invocation(model_completion_request(
            &fixture,
            &model,
            vec![valid_artifact],
            Some(valid_id),
        ))
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn tool_terminal_rejects_nonexact_count_producer_digest_and_stream_reuse_artifacts() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
    let dispatch_event = dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
    let counts = ToolStreamCounts {
        observed: CanonicalByteCount::try_new(3).unwrap(),
        captured: CanonicalByteCount::try_new(3).unwrap(),
        returned_inline: CanonicalByteCount::try_new(0).unwrap(),
        omitted: CanonicalByteCount::try_new(3).unwrap(),
    };

    let missing = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        Vec::new(),
        (Some(ArtifactId::generate()), Some(counts)),
        (None, None),
    );
    assert!(fixture.store.finish_tool_execution(missing).await.is_err());

    let extra_artifact = finalized_artifact(
        &fixture,
        ArtifactId::generate(),
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "extra-stdout",
    );
    let extra = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![extra_artifact],
        (None, None),
        (None, None),
    );
    assert!(fixture.store.finish_tool_execution(extra).await.is_err());

    let duplicate_id = ArtifactId::generate();
    let duplicate_artifact = finalized_artifact(
        &fixture,
        duplicate_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "duplicate-stdout",
    );
    let duplicate = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![duplicate_artifact.clone(), duplicate_artifact],
        (Some(duplicate_id), Some(counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(duplicate)
            .await
            .is_err()
    );

    let wrong_count_id = ArtifactId::generate();
    let wrong_count_artifact = finalized_artifact(
        &fixture,
        wrong_count_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "wrong-count-stdout",
    );
    let wrong_counts = ToolStreamCounts {
        observed: CanonicalByteCount::try_new(3).unwrap(),
        captured: CanonicalByteCount::try_new(2).unwrap(),
        returned_inline: CanonicalByteCount::try_new(0).unwrap(),
        omitted: CanonicalByteCount::try_new(3).unwrap(),
    };
    let wrong_count = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![wrong_count_artifact],
        (Some(wrong_count_id), Some(wrong_counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(wrong_count)
            .await
            .is_err()
    );

    let wrong_observed_id = ArtifactId::generate();
    let wrong_observed_artifact = finalized_artifact(
        &fixture,
        wrong_observed_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "wrong-observed-stdout",
    );
    let wrong_observed_counts = ToolStreamCounts {
        observed: CanonicalByteCount::try_new(4).unwrap(),
        captured: CanonicalByteCount::try_new(3).unwrap(),
        returned_inline: CanonicalByteCount::try_new(0).unwrap(),
        omitted: CanonicalByteCount::try_new(4).unwrap(),
    };
    let wrong_observed = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![wrong_observed_artifact],
        (Some(wrong_observed_id), Some(wrong_observed_counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(wrong_observed)
            .await
            .is_err()
    );

    let wrong_truncation_id = ArtifactId::generate();
    let wrong_truncation_artifact = finalized_artifact(
        &fixture,
        wrong_truncation_id,
        ArtifactProducer::Tool(tool_id),
        b"abcdef",
        3,
        "wrong-truncation-stdout",
    );
    let wrong_truncation = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![wrong_truncation_artifact],
        (Some(wrong_truncation_id), Some(counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(wrong_truncation)
            .await
            .is_err()
    );

    let stderr_id = ArtifactId::generate();
    let stderr_artifact = finalized_artifact(
        &fixture,
        stderr_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "wrong-count-stderr",
    );
    let stderr_mismatch = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![stderr_artifact],
        (None, None),
        (Some(stderr_id), Some(wrong_counts)),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(stderr_mismatch)
            .await
            .is_err()
    );

    let wrong_producer_id = ArtifactId::generate();
    let wrong_producer_artifact = finalized_artifact(
        &fixture,
        wrong_producer_id,
        ArtifactProducer::Model(model.invocation_id),
        b"abc",
        3,
        "wrong-producer-stdout",
    );
    let wrong_producer = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![wrong_producer_artifact],
        (Some(wrong_producer_id), Some(counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(wrong_producer)
            .await
            .is_err()
    );

    let reused_id = ArtifactId::generate();
    let reused_artifact = finalized_artifact(
        &fixture,
        reused_id,
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "reused-stream",
    );
    let reused = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![reused_artifact],
        (Some(reused_id), Some(counts)),
        (Some(reused_id), Some(counts)),
    );
    assert!(fixture.store.finish_tool_execution(reused).await.is_err());

    let mut digest_mismatch = finalized_artifact(
        &fixture,
        ArtifactId::generate(),
        ArtifactProducer::Tool(tool_id),
        b"abc",
        3,
        "wrong-digest-stdout",
    );
    let digest_id = digest_mismatch.metadata.artifact_id();
    let wrong_digest = Sha256Digest::hash_bytes(b"xyz");
    digest_mismatch.metadata = ArtifactReference::new(ArtifactReferenceInput {
        artifact_id: digest_id,
        craxii_id: fixture.identity.craxii_id,
        producing_work_id: Some(fixture.work_id),
        producer: ArtifactProducer::Tool(tool_id),
        storage_key: ArtifactStorageKey::from_digest(wrong_digest),
        sha256: wrong_digest,
        canonical_length: CanonicalByteCount::try_new(3).unwrap(),
        observed_length: Some(CanonicalByteCount::try_new(3).unwrap()),
        mime_type: ArtifactMimeType::try_new("application/octet-stream").unwrap(),
        encoding: None,
        logical_name: Some(ArtifactLogicalName::try_new("wrong-digest-stdout").unwrap()),
        retention: ArtifactRetention::CanonicalEvidence,
        truncated: false,
        compression: None,
        created_at: T4.parse().unwrap(),
    });
    let digest_mismatch = successful_tool_completion(
        &fixture,
        tool_id,
        waiting,
        dispatch_event,
        vec![digest_mismatch],
        (Some(digest_id), Some(counts)),
        (None, None),
    );
    assert!(
        fixture
            .store
            .finish_tool_execution(digest_mismatch)
            .await
            .is_err()
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM artifacts WHERE producer_kind = 'tool_execution'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE event_type = 'artifact.recorded'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn model_begin_stream_and_terminal_are_atomic_ordered_and_clear_work_link() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let model_row = sqlx::query(
        "SELECT state, logical_invocation_id, response_sha256, normalized_output_json \
         FROM model_invocations WHERE model_invocation_id = ?",
    )
    .bind(model.invocation_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(model_row.get::<String, _>("state"), "completed");
    assert_eq!(
        model_row.get::<String, _>("logical_invocation_id"),
        model.logical_id.to_string()
    );
    assert!(
        model_row
            .get::<Option<String>, _>("response_sha256")
            .is_some()
    );
    assert!(
        model_row
            .get::<Option<String>, _>("normalized_output_json")
            .is_some()
    );
    let work = sqlx::query(
        "SELECT state, state_version, current_model_invocation_id FROM work_items WHERE work_id = ?",
    )
    .bind(fixture.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work.get::<String, _>("state"), "running");
    assert_eq!(work.get::<i64, _>("state_version"), 4);
    assert!(
        work.get::<Option<String>, _>("current_model_invocation_id")
            .is_none()
    );
    let event_types = sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM journal_events WHERE work_id = ? ORDER BY journal_offset",
    )
    .bind(fixture.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        event_types,
        [
            "artifact.recorded",
            "model.invocation_started",
            "work.waiting_on_model",
            "model.invocation_streaming",
            "model.invocation_completed",
            "work.resumed",
        ]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn assistant_completion_is_one_atomic_authoritative_message_and_work_commit() {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let terminal_model_event: JournalEventId = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM journal_events WHERE work_id = ? \
             AND event_type = 'model.invocation_completed'",
        )
        .bind(fixture.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap()
        .parse()
        .unwrap()
    };
    let running = resumed(&fixture, 4);
    let completed = decide_work_transition(
        &running,
        WorkTransitionGuard::for_snapshot(&running),
        WorkTransitionRequest::Complete {
            reason: WorkCompletionReason::Answered,
            evidence: WorkCompletionEvidence::SATISFIED,
        },
    )
    .unwrap()
    .into_next();
    let message = Message::try_new(MessageInput {
        message_id: MessageId::generate(),
        craxii_id: fixture.identity.craxii_id,
        conversation_id: fixture.identity.conversation_id,
        role: MessageRole::Assistant,
        content: MessageContent::try_new(vec![ContentBlock::text("done").unwrap()]).unwrap(),
        produced_by_work_id: Some(fixture.work_id),
        device_id: None,
        client_message_id: None,
        committed_at: T4.parse().unwrap(),
    })
    .unwrap();
    let message_id = message.message_id();
    let assistant_event = JournalEventId::generate();
    fixture
        .store
        .commit_assistant_completion(CommitAssistantCompletionRequest {
            expected_work: WorkExpectation::for_snapshot(&running),
            expected_model: ModelExpectation {
                model_invocation_id: model.invocation_id,
                state: ModelInvocationState::Completed,
            },
            assistant_message: message,
            assistant_event: EventIntent {
                event_id: assistant_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(terminal_model_event),
            },
            completion_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(assistant_event),
            },
            work_next: completed,
        })
        .await
        .unwrap();

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let message_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE message_id = ? AND role = 'assistant'",
    )
    .bind(message_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let work_state: String = sqlx::query_scalar(
        "SELECT state FROM work_items WHERE work_id = ? AND runtime_instance_id IS NULL",
    )
    .bind(fixture.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let terminal_events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM journal_events WHERE work_id = ? \
         AND event_type IN ('assistant.message_committed','work.completed') ORDER BY journal_offset",
    )
    .bind(fixture.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(message_count, 1);
    assert_eq!(work_state, "completed");
    assert_eq!(
        terminal_events,
        ["assistant.message_committed", "work.completed"]
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn model_begin_failure_at_final_event_boundary_rolls_back_every_detail_row() {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let existing_event: String = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query_scalar("SELECT event_id FROM journal_events ORDER BY journal_offset LIMIT 1")
            .fetch_one(&mut *connection)
            .await
            .unwrap()
    };
    let invocation_id = ModelInvocationId::generate();
    let logical_id = LogicalInvocationId::generate();
    let manifest_id = ContextManifestId::generate();
    let request_sha = Sha256Digest::hash_bytes(b"rollback-request");
    let artifact_id = ArtifactId::generate();
    let mut capture = fixture
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id,
            hard_capture_limit: CanonicalByteCount::try_new(16).unwrap(),
        })
        .unwrap();
    capture.write_chunk(b"rollback-request").unwrap();
    let finalized = capture.finalize().unwrap();
    let object = finalized.object_reference().clone();
    let result = fixture
        .store
        .begin_model_invocation(BeginModelInvocationRequest {
            expected_work: running_expectation(&fixture, 2),
            manifest: PreparedContextManifest {
                context_manifest_id: manifest_id,
                work_id: fixture.work_id,
                logical_invocation_id: logical_id,
                provider_model: provider_model(),
                assembler_version: "stage8-test-v1".to_owned(),
                context_policy_version: "stage8-test-v1".to_owned(),
                system_prompt_fingerprint: Sha256Digest::hash_bytes(b"system"),
                toolset_fingerprint: Sha256Digest::hash_bytes(b"tools"),
                eligibility_conversation_id: fixture.identity.conversation_id,
                active_work_ordinal: 1,
                highest_prior_terminal_work_ordinal: None,
                input_event_ids: Vec::new(),
                active_output_record_ids: Vec::new(),
                maximum_journal_offset: JournalOffset::try_new(2).unwrap(),
                canonical_byte_count: CanonicalByteCount::try_new(16).unwrap(),
                rendered_request_byte_count: CanonicalByteCount::try_new(16).unwrap(),
                estimated_input_tokens: 1,
                token_estimator_id: "test-estimator-v1".to_owned(),
                context_window_tokens: 1_000,
                reserved_output_tokens: 99,
                utilization_basis_points: 1_000,
                manifest_sha256: Sha256Digest::hash_bytes(b"rollback-manifest"),
                rendered_request_sha256: request_sha,
                rendered_request_artifact_id: Some(artifact_id),
                omitted_source_count: 0,
                transformed_source_count: 0,
                sources: Vec::new(),
                created_at: T1.parse().unwrap(),
            },
            invocation: PreparedModelInvocation {
                attempt: ModelAttemptReference::new(ModelAttemptReferenceInput {
                    logical_invocation_id: logical_id,
                    model_invocation_id: invocation_id,
                    work_id: fixture.work_id,
                    runtime_instance_id: fixture.runtime_id,
                    context_manifest_id: manifest_id,
                    agent_step_no: AgentStepNo::try_new(1).unwrap(),
                    attempt_no: AttemptNo::try_new(1).unwrap(),
                    provider_model: provider_model(),
                    retry_of: None,
                }),
                selection_reason: ModelSelectionReason::Explicit,
                required_capabilities: RequiredModelCapabilities {
                    text_input: true,
                    text_output: true,
                    custom_tool_calling: false,
                    streaming: true,
                    ordered_output_items: true,
                    structured_output: false,
                    reasoning_continuation: false,
                },
                provider_options: Vec::new(),
                request_sha256: request_sha,
                request_artifact_id: Some(artifact_id),
                retry_evidence: None,
                started_at: T1.parse().unwrap(),
            },
            artifacts: vec![PreparedArtifact {
                finalized,
                metadata: ArtifactReference::new(ArtifactReferenceInput {
                    artifact_id,
                    craxii_id: fixture.identity.craxii_id,
                    producing_work_id: Some(fixture.work_id),
                    producer: ArtifactProducer::Model(invocation_id),
                    storage_key: ArtifactStorageKey::from_digest(request_sha),
                    sha256: request_sha,
                    canonical_length: CanonicalByteCount::try_new(16).unwrap(),
                    observed_length: Some(CanonicalByteCount::try_new(16).unwrap()),
                    mime_type: ArtifactMimeType::try_new("application/json").unwrap(),
                    encoding: Some(ArtifactEncoding::try_new("utf-8").unwrap()),
                    logical_name: Some(ArtifactLogicalName::try_new("rollback-request").unwrap()),
                    retention: ArtifactRetention::CanonicalEvidence,
                    truncated: false,
                    compression: None,
                    created_at: T1.parse().unwrap(),
                }),
                event: EventIntent {
                    event_id: JournalEventId::generate(),
                    correlation_id: fixture.correlation_id,
                    causation_event_id: None,
                },
            }],
            work_next: waiting_model(&fixture, invocation_id, 3),
            invocation_event: EventIntent {
                event_id: JournalEventId::parse_canonical(&existing_event).unwrap(),
                correlation_id: fixture.correlation_id,
                causation_event_id: None,
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(JournalEventId::parse_canonical(&existing_event).unwrap()),
            },
        })
        .await;
    assert!(result.is_err());
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    for table in [
        "artifacts",
        "context_manifests",
        "context_manifest_sources",
        "model_invocations",
    ] {
        let count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table}"
        )))
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    let work = sqlx::query("SELECT state, state_version FROM work_items WHERE work_id = ?")
        .bind(fixture.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(work.get::<String, _>("state"), "running");
    assert_eq!(work.get::<i64, _>("state_version"), 2);
    drop(connection);
    assert_eq!(
        fixture.artifact_store.read_verified(&object).unwrap(),
        b"rollback-request"
    );
    let report = fixture
        .artifact_store
        .scan_orphans(&BTreeSet::new(), T4.parse().unwrap())
        .unwrap();
    assert_eq!(report.referenced_final_count, 0);
    assert_eq!(report.orphans.len(), 1);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let root = fixture._root.path().to_owned();
    fixture.guard.shutdown().await;
    let reopened = SqliteRuntimeGuard::start(&root, 1).await.unwrap();
    SqliteStateStore::new(reopened.runtime().clone())
        .verify_application_consistency()
        .await
        .unwrap();
    reopened.shutdown().await;
}

#[tokio::test]
async fn model_retry_is_contiguous_terminal_and_uses_distinct_semantic_artifact_metadata() {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;

    let (_, skipped_request) = retry_model_request(&fixture, &model, 3);
    assert!(
        fixture
            .store
            .begin_model_invocation(skipped_request)
            .await
            .is_err()
    );
    let (retry_id, retry_request) = retry_model_request(&fixture, &model, 2);
    fixture
        .store
        .begin_model_invocation(retry_request)
        .await
        .unwrap();

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let rows = sqlx::query(
        "SELECT model_invocation_id, attempt_no, retry_of_invocation_id, context_manifest_id, \
         request_artifact_id, retry_reason, retry_delay_ms, provider_retry_after_ms \
         FROM model_invocations ORDER BY attempt_no",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<i64, _>("attempt_no"), 1);
    assert_eq!(rows[1].get::<i64, _>("attempt_no"), 2);
    assert_eq!(
        rows[1].get::<Option<String>, _>("retry_of_invocation_id"),
        Some(model.invocation_id.to_string())
    );
    assert_eq!(
        rows[1].get::<String, _>("context_manifest_id"),
        model.manifest_id.to_string()
    );
    assert_ne!(
        rows[0].get::<Option<String>, _>("request_artifact_id"),
        rows[1].get::<Option<String>, _>("request_artifact_id")
    );
    assert_eq!(
        rows[1].get::<Option<String>, _>("retry_reason").as_deref(),
        Some("classified_transient_before_output")
    );
    assert_eq!(rows[1].get::<Option<i64>, _>("retry_delay_ms"), Some(0));
    assert_eq!(
        rows[1].get::<Option<i64>, _>("provider_retry_after_ms"),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM artifacts WHERE producer_kind = 'model_invocation' \
             AND producer_id IN (?, ?)",
        )
        .bind(model.invocation_id.to_string())
        .bind(retry_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT storage_key) FROM artifacts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn tool_request_dispatch_and_unknown_outcome_preserve_intent_before_action() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let tool_id = ToolExecutionId::generate();
    let execution_id = ExecutionId::generate();
    let requested_event = JournalEventId::generate();
    fixture
        .store
        .request_tool_execution(RequestToolExecutionRequest {
            expected_work: running_expectation(&fixture, 4),
            tool: PreparedToolExecution {
                lifecycle: ToolLifecycleReference::new(
                    tool_id,
                    execution_id,
                    fixture.work_id,
                    fixture.runtime_id,
                    model.invocation_id,
                    AgentStepNo::try_new(1).unwrap(),
                    ToolOrdinal::try_new(1).unwrap(),
                ),
                provider_tool_call_id: Some("call-1".to_owned()),
                tool_name: ToolName::try_new("run-shell").unwrap(),
                tool_version: ToolVersion::try_new("1.0.0").unwrap(),
                tool_schema_version: 1,
                arguments_json: "{\"command\":\"true\"}".to_owned(),
                arguments_sha256: Sha256Digest::hash_bytes(b"{\"command\":\"true\"}"),
                workstation_id: fixture.identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                workspace_id: fixture.identity.workspace_id,
                requested_cwd: LogicalPathReference::absolute("/workspace").unwrap(),
                requested_privilege: PrivilegeMode::User,
                timeout_ms: 1_000,
                output_policy: ToolOutputPolicy {
                    stdout_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    stderr_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    combined_inline_limit: CanonicalByteCount::try_new(50).unwrap(),
                    per_stream_inline_limit: CanonicalByteCount::try_new(25).unwrap(),
                },
                requested_at: T3.parse().unwrap(),
            },
            work_next: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id: fixture.work_id,
                state: WorkState::WaitingOnTool,
                projection_version: ProjectionVersion::try_new(5).unwrap(),
                runtime_owner: Some(fixture.runtime_id),
                current_attempt: CurrentWorkAttempt::Tool(tool_id),
                cancellation_reason: None,
                terminal_reason: None,
            })
            .unwrap(),
            tool_event: EventIntent {
                event_id: requested_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: None,
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(requested_event),
            },
        })
        .await
        .unwrap();
    let waiting = WorkExpectation {
        work_id: fixture.work_id,
        state: WorkState::WaitingOnTool,
        version: ProjectionVersion::try_new(5).unwrap(),
        runtime_owner: Some(fixture.runtime_id),
        current_attempt: CurrentWorkAttempt::Tool(tool_id),
        cancellation_reason: None,
    };
    let dispatch_event = JournalEventId::generate();
    let prepared_cwd = fixture_prepared_cwd(&fixture);
    fixture
        .store
        .commit_tool_dispatch_intent(CommitToolDispatchIntentRequest {
            expected_work: waiting,
            expected_tool: ToolExpectation {
                tool_execution_id: tool_id,
                state: ToolExecutionState::Requested,
            },
            dispatch: ToolDispatchIntent {
                authority: AuthorityDecisionSnapshot::new(
                    AuthorityDecision::Allow,
                    PrivilegeMode::User,
                    AuthorityReasonCode::try_new("allowed").unwrap(),
                ),
                dispatch_evidence_json: authority_evidence(
                    &fixture,
                    "run_shell",
                    "foreground_execute",
                    &prepared_cwd,
                ),
                effective_privilege: PrivilegeMode::User,
                prepared_cwd,
                timeout_ms: 1_000,
                output_policy: ToolOutputPolicy {
                    stdout_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    stderr_capture_limit: CanonicalByteCount::try_new(100).unwrap(),
                    combined_inline_limit: CanonicalByteCount::try_new(50).unwrap(),
                    per_stream_inline_limit: CanonicalByteCount::try_new(25).unwrap(),
                },
                dispatch_intent_at: T4.parse().unwrap(),
            },
            event: EventIntent {
                event_id: dispatch_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(requested_event),
            },
        })
        .await
        .unwrap();
    let unknown_event = JournalEventId::generate();
    fixture
        .store
        .finish_tool_execution(FinishToolExecutionRequest {
            expected_work: waiting,
            expected_tool: ToolExpectation {
                tool_execution_id: tool_id,
                state: ToolExecutionState::Dispatching,
            },
            outcome: ToolTerminalOutcome {
                state: ToolExecutionState::OutcomeUnknown,
                predispatch_authority: None,
                started_at: None,
                completed_at: "2026-08-28T01:02:08.000000Z".parse().unwrap(),
                exit_code: None,
                signal: None,
                timed_out: None,
                cancelled: None,
                cleanup_confirmed: Some(false),
                result: None,
                evidence_artifact_ids: Vec::new(),
                stdout_artifact_id: None,
                stderr_artifact_id: None,
                stdout_counts: None,
                stderr_counts: None,
                truncated: false,
                normalized_error: Some(NormalizedError::workstation(
                    Certainty::OutcomeUnknown,
                    None,
                )),
            },
            artifacts: Vec::new(),
            work_next: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id: fixture.work_id,
                state: WorkState::Interrupted,
                projection_version: ProjectionVersion::try_new(6).unwrap(),
                runtime_owner: None,
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: None,
                terminal_reason: Some(WorkTerminalReason::Interruption(
                    WorkInterruptionReason::ToolOutcomeUnknown,
                )),
            })
            .unwrap(),
            tool_event: EventIntent {
                event_id: unknown_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(dispatch_event),
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(unknown_event),
            },
        })
        .await
        .unwrap();
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let tool = sqlx::query(
        "SELECT state, authority_decision_json, dispatch_intent_at, cleanup_confirmed \
         FROM tool_executions WHERE tool_execution_id = ?",
    )
    .bind(tool_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(tool.get::<String, _>("state"), "outcome_unknown");
    assert!(
        tool.get::<Option<String>, _>("authority_decision_json")
            .is_some()
    );
    assert!(
        tool.get::<Option<String>, _>("dispatch_intent_at")
            .is_some()
    );
    assert_eq!(tool.get::<Option<i64>, _>("cleanup_confirmed"), Some(0));
    let work = sqlx::query(
        "SELECT state, current_tool_execution_id, terminal_reason_code FROM work_items WHERE work_id = ?",
    )
    .bind(fixture.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work.get::<String, _>("state"), "interrupted");
    assert!(
        work.get::<Option<String>, _>("current_tool_execution_id")
            .is_none()
    );
    assert_eq!(
        work.get::<Option<String>, _>("terminal_reason_code")
            .as_deref(),
        Some("tool_outcome_unknown")
    );
    let event_types = sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM journal_events WHERE work_id = ? ORDER BY journal_offset",
    )
    .bind(fixture.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert!(
        event_types
            .windows(2)
            .any(|pair| pair == ["tool.execution_requested", "work.waiting_on_tool"])
    );
    assert!(
        event_types
            .windows(2)
            .any(|pair| pair == ["tool.execution_outcome_unknown", "work.interrupted"])
    );
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn tool_predispatch_denial_and_interruption_are_definite_and_never_dispatch() {
    let fixture = fixture().await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;

    let (denied_tool, denied_request_event, denied_waiting) =
        request_tool(&fixture, &model, 1, 4).await;
    let denied_event = JournalEventId::generate();
    fixture
        .store
        .finish_tool_execution(FinishToolExecutionRequest {
            expected_work: denied_waiting,
            expected_tool: ToolExpectation {
                tool_execution_id: denied_tool,
                state: ToolExecutionState::Requested,
            },
            outcome: ToolTerminalOutcome {
                state: ToolExecutionState::Completed,
                predispatch_authority: Some(AuthorityDecisionSnapshot::new(
                    AuthorityDecision::Deny,
                    PrivilegeMode::User,
                    AuthorityReasonCode::try_new("policy_denied").unwrap(),
                )),
                started_at: None,
                completed_at: T4.parse().unwrap(),
                exit_code: None,
                signal: None,
                timed_out: None,
                cancelled: None,
                cleanup_confirmed: None,
                result: Some(ToolResultEvidence {
                    result_kind: ToolResultClass::AuthorityDenial,
                    summary: "authority denied".to_owned(),
                    fields: Vec::new(),
                }),
                evidence_artifact_ids: Vec::new(),
                stdout_artifact_id: None,
                stderr_artifact_id: None,
                stdout_counts: None,
                stderr_counts: None,
                truncated: false,
                normalized_error: None,
            },
            artifacts: Vec::new(),
            work_next: resumed(&fixture, 6),
            tool_event: EventIntent {
                event_id: denied_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(denied_request_event),
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(denied_event),
            },
        })
        .await
        .unwrap();

    let (interrupted_tool, interrupted_request_event, interrupted_waiting) =
        request_tool(&fixture, &model, 2, 6).await;
    let interrupted_event = JournalEventId::generate();
    fixture
        .store
        .finish_tool_execution(FinishToolExecutionRequest {
            expected_work: interrupted_waiting,
            expected_tool: ToolExpectation {
                tool_execution_id: interrupted_tool,
                state: ToolExecutionState::Requested,
            },
            outcome: ToolTerminalOutcome {
                state: ToolExecutionState::InterruptedBeforeDispatch,
                predispatch_authority: None,
                started_at: None,
                completed_at: "2026-08-28T01:02:08.000000Z".parse().unwrap(),
                exit_code: None,
                signal: None,
                timed_out: None,
                cancelled: None,
                cleanup_confirmed: None,
                result: None,
                evidence_artifact_ids: Vec::new(),
                stdout_artifact_id: None,
                stderr_artifact_id: None,
                stdout_counts: None,
                stderr_counts: None,
                truncated: false,
                normalized_error: Some(NormalizedError::workstation(Certainty::Definite, None)),
            },
            artifacts: Vec::new(),
            work_next: WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                work_id: fixture.work_id,
                state: WorkState::Interrupted,
                projection_version: ProjectionVersion::try_new(8).unwrap(),
                runtime_owner: None,
                current_attempt: CurrentWorkAttempt::None,
                cancellation_reason: None,
                terminal_reason: Some(WorkTerminalReason::Interruption(
                    WorkInterruptionReason::ToolInterruptedBeforeDispatch,
                )),
            })
            .unwrap(),
            tool_event: EventIntent {
                event_id: interrupted_event,
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(interrupted_request_event),
            },
            work_event: EventIntent {
                event_id: JournalEventId::generate(),
                correlation_id: fixture.correlation_id,
                causation_event_id: Some(interrupted_event),
            },
        })
        .await
        .unwrap();

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let denied = sqlx::query(
        "SELECT state, dispatch_intent_at, started_at, authority_decision_json \
         FROM tool_executions WHERE tool_execution_id = ?",
    )
    .bind(denied_tool.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(denied.get::<String, _>("state"), "completed");
    assert!(
        denied
            .get::<Option<String>, _>("dispatch_intent_at")
            .is_none()
    );
    assert!(denied.get::<Option<String>, _>("started_at").is_none());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &denied
                .get::<Option<String>, _>("authority_decision_json")
                .unwrap()
        )
        .unwrap()["decision"],
        "deny"
    );
    let interrupted = sqlx::query(
        "SELECT state, dispatch_intent_at, authority_decision_json FROM tool_executions \
         WHERE tool_execution_id = ?",
    )
    .bind(interrupted_tool.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        interrupted.get::<String, _>("state"),
        "interrupted_before_dispatch"
    );
    assert!(
        interrupted
            .get::<Option<String>, _>("dispatch_intent_at")
            .is_none()
    );
    assert!(
        interrupted
            .get::<Option<String>, _>("authority_decision_json")
            .is_none()
    );
    let event_types = sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM journal_events WHERE work_id = ? ORDER BY journal_offset",
    )
    .bind(fixture.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert!(
        event_types
            .windows(2)
            .any(|pair| pair == ["tool.execution_completed", "work.resumed"])
    );
    assert!(event_types.windows(2).any(|pair| pair
        == [
            "tool.execution_interrupted_before_dispatch",
            "work.interrupted"
        ]));
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn artifact_publish_crash_child() {
    let Some(root) = std::env::var_os("CRAXII_STAGE8_ARTIFACT_CRASH_ROOT") else {
        return;
    };
    let artifact_id: ArtifactId = std::env::var("CRAXII_STAGE8_ARTIFACT_CRASH_ID")
        .unwrap()
        .parse()
        .unwrap();
    let root = PathBuf::from(root);
    let guard = SqliteRuntimeGuard::start(&root, 1).await.unwrap();
    let state_store = SqliteStateStore::new(guard.runtime().clone());
    state_store
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
            created_at: T0.parse().unwrap(),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".to_owned(),
                os_release: "macos".to_owned(),
                default_shell: "/bin/zsh".to_owned(),
                workspace_logical_name: "primary".to_owned(),
                workspace_logical_root: "/workspace".to_owned(),
                workspace_resolved_root: "/workspace".to_owned(),
                execution_capabilities:
                    crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap();
    let artifact_store = LocalArtifactStore::initialize(&root.join("artifacts")).unwrap();
    let mut capture = artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id,
            hard_capture_limit: CanonicalByteCount::try_new(64).unwrap(),
        })
        .unwrap();
    capture.write_chunk(b"durable-before-sqlite").unwrap();
    capture.finalize().unwrap();
    std::process::exit(86);
}

#[tokio::test]
async fn durable_publish_before_database_commit_reopens_as_nonfatal_orphan_without_deletion() {
    let root = TestRoot::new();
    let artifact_id = ArtifactId::generate();
    let child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("adapters::sqlite::stage8_tests::artifact_publish_crash_child")
        .arg("--nocapture")
        .env("CRAXII_STAGE8_ARTIFACT_CRASH_ROOT", root.path())
        .env("CRAXII_STAGE8_ARTIFACT_CRASH_ID", artifact_id.to_string())
        .env("RUST_TEST_THREADS", "1")
        .status()
        .unwrap();
    assert_eq!(child.code(), Some(86));
    let reopened = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let reopened_state = SqliteStateStore::new(reopened.runtime().clone());
    reopened_state
        .verify_application_consistency()
        .await
        .unwrap();
    let reopened_artifacts =
        LocalArtifactStore::initialize(&root.path().join("artifacts")).unwrap();
    let report = reopened_artifacts
        .scan_orphans(&BTreeSet::new(), T4.parse().unwrap())
        .unwrap();
    assert_eq!(report.referenced_final_count, 0);
    assert_eq!(report.orphans.len(), 1);
    assert!(!report.orphans[0].eligible_for_maintenance());
    let digest = Sha256Digest::hash_bytes(b"durable-before-sqlite");
    let object = crate::ports::artifact_store::ArtifactObjectReference::from_persisted_metadata(
        ArtifactStorageKey::from_digest(digest),
        digest,
        CanonicalByteCount::try_new(21).unwrap(),
    );
    assert_eq!(
        reopened_artifacts.read_verified(&object).unwrap(),
        b"durable-before-sqlite"
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn populated_v2_migrates_through_v4_without_changing_stage7_identity_or_old_fingerprints() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let state_store = SqliteStateStore::new(guard.runtime().clone());
    let identity = state_store
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
            created_at: T0.parse().unwrap(),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".to_owned(),
                os_release: "macos".to_owned(),
                default_shell: "/bin/zsh".to_owned(),
                workspace_logical_name: "primary".to_owned(),
                workspace_logical_root: "/workspace".to_owned(),
                workspace_resolved_root: "/workspace".to_owned(),
                execution_capabilities:
                    crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    let mut connection = guard.runtime().acquire().await.unwrap();
    for table in [
        "context_manifest_sources",
        "tool_executions",
        "model_invocations",
        "context_manifests",
        "artifacts",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version >= 3")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    guard.shutdown().await;

    let migrated = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    assert_eq!(
        migrated.disposition(),
        super::schema::DatabaseDisposition::Current
    );
    let migrated_store = SqliteStateStore::new(migrated.runtime().clone());
    assert_eq!(
        migrated_store
            .load_bootstrap_snapshot()
            .await
            .unwrap()
            .identity,
        identity
    );
    assert_eq!(
        super::schema::v1_schema_fingerprint(),
        "f4636df22c635c90ac469f49f2ac3a9ccb38956f1670d26ab566140a137f5521"
    );
    assert_eq!(
        super::schema::v2_schema_fingerprint(),
        "391d9bfb54cf771de1815a3bf54ee4d7d16f1b877acf629cf783ca12dbd37d4d"
    );
    assert_eq!(
        super::schema::expected_schema_fingerprint(),
        "78eed488a202c15dac3215ea96ca860907d472c393639bdc94f90301007e4fb2"
    );
    migrated.shutdown().await;

    let reopened = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    assert_eq!(
        reopened.disposition(),
        super::schema::DatabaseDisposition::Current
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn natural_stage8_queries_use_the_frozen_named_indexes() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let probes = [
        (
            "ix_artifacts_storage_key",
            "SELECT artifact_id FROM artifacts WHERE storage_key = 'sha256/aa/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
        ),
        (
            "ix_artifacts_content",
            "SELECT artifact_id FROM artifacts WHERE sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' AND captured_byte_count = 1 AND backend = 'local'",
        ),
        (
            "ix_artifacts_producing_work",
            "SELECT artifact_id FROM artifacts WHERE producing_work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' ORDER BY created_at",
        ),
        (
            "ix_artifacts_producer_kind_id",
            "SELECT artifact_id FROM artifacts WHERE producer_kind = 'model_invocation' AND producer_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ux_context_manifests_logical_invocation",
            "SELECT context_manifest_id FROM context_manifests WHERE logical_invocation_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ix_context_manifests_work_created",
            "SELECT context_manifest_id FROM context_manifests WHERE work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' ORDER BY created_at",
        ),
        (
            "ix_context_manifest_sources_event",
            "SELECT context_manifest_id FROM context_manifest_sources WHERE event_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ix_context_manifest_sources_artifact",
            "SELECT context_manifest_id FROM context_manifest_sources WHERE artifact_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ux_model_invocations_logical_attempt",
            "SELECT model_invocation_id FROM model_invocations WHERE logical_invocation_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND attempt_no = 1",
        ),
        (
            "ux_model_invocations_work_step_attempt",
            "SELECT model_invocation_id FROM model_invocations WHERE work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND agent_step_no = 1 AND attempt_no = 1",
        ),
        (
            "ux_model_invocations_retry_of",
            "SELECT model_invocation_id FROM model_invocations WHERE retry_of_invocation_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ux_model_invocations_one_nonterminal_per_work",
            "SELECT model_invocation_id FROM model_invocations WHERE work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND state IN ('requesting', 'streaming')",
        ),
        (
            "ix_model_invocations_runtime_nonterminal",
            "SELECT model_invocation_id FROM model_invocations WHERE runtime_instance_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND state IN ('requesting', 'streaming') ORDER BY work_id",
        ),
        (
            "ix_model_invocations_context_attempt",
            "SELECT model_invocation_id FROM model_invocations WHERE context_manifest_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' ORDER BY attempt_no",
        ),
        (
            "ux_tool_executions_execution_id",
            "SELECT tool_execution_id FROM tool_executions WHERE execution_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d'",
        ),
        (
            "ux_tool_executions_work_step_ordinal",
            "SELECT tool_execution_id FROM tool_executions WHERE work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND agent_step_no = 1 AND tool_ordinal = 1",
        ),
        (
            "ux_tool_executions_source_ordinal",
            "SELECT tool_execution_id FROM tool_executions WHERE source_model_invocation_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND tool_ordinal = 1",
        ),
        (
            "ux_tool_executions_source_provider_call",
            "SELECT tool_execution_id FROM tool_executions WHERE source_model_invocation_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND provider_tool_call_id = 'call-1'",
        ),
        (
            "ux_tool_executions_one_nonterminal_per_work",
            "SELECT tool_execution_id FROM tool_executions WHERE work_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND state IN ('requested', 'dispatching')",
        ),
        (
            "ix_tool_executions_runtime_nonterminal",
            "SELECT tool_execution_id FROM tool_executions WHERE runtime_instance_id = '01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d' AND state IN ('requested', 'dispatching') ORDER BY work_id",
        ),
    ];
    for (index, query) in probes {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {query}")))
            .fetch_all(&mut *connection)
            .await
            .unwrap();
        let detail = rows
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(detail.contains(index), "{index}: {detail}");
    }
    drop(connection);
    guard.shutdown().await;
}

async fn create_stage10_recovery_runtime(fixture: &Fixture) -> RuntimeInstanceId {
    create_stage10_recovery_runtime_with_evidence(fixture)
        .await
        .0
}

async fn create_stage10_recovery_runtime_with_evidence(
    fixture: &Fixture,
) -> (RuntimeInstanceId, JournalEventId, CorrelationId) {
    let runtime_id = RuntimeInstanceId::generate();
    let started_event_id = JournalEventId::generate();
    let correlation_id = CorrelationId::generate();
    fixture
        .store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence: RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: fixture.identity.craxii_id,
                workstation_id: fixture.identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("stage10-recovery-test-boot").unwrap()),
                diagnostic_pid: Some(DiagnosticPid::try_new(84).unwrap()),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: GitRevision::try_new("stage10-recovery-test").unwrap(),
                schema_version: SchemaVersion::try_new(4).unwrap(),
                started_at: T5.parse().unwrap(),
            }),
            event_id: started_event_id,
            correlation_id,
        })
        .await
        .unwrap();
    (runtime_id, started_event_id, correlation_id)
}

async fn append_stage10_recovery_summary(
    fixture: &Fixture,
    runtime_id: RuntimeInstanceId,
    started_event_id: JournalEventId,
    correlation_id: CorrelationId,
    recovery: RecoveryReceipt,
) {
    fixture
        .store
        .append_recovery_summary(AppendRecoverySummaryRequest {
            summary: RuntimeRecoveryPerformedV1 {
                runtime_instance_id: runtime_id,
                stale_runtimes_observed: 1,
                stale_runtimes_closed: u64::from(recovery.stale_runtime_closed),
                retained_queued_work: fixture.store.count_retained_queued_work().await.unwrap(),
                interrupted_work: recovery.interrupted_work,
                model_attempts_provider_outcome_unknown: recovery
                    .model_attempts_provider_outcome_unknown,
                model_attempts_terminal_preserved: recovery.model_attempts_terminal_preserved,
                tool_attempts_interrupted_before_dispatch: recovery
                    .tool_attempts_interrupted_before_dispatch,
                tool_attempts_outcome_unknown: recovery.tool_attempts_outcome_unknown,
                tool_attempts_terminal_preserved: recovery.tool_attempts_terminal_preserved,
                drafts_abandoned: recovery.drafts_abandoned,
                orphan_artifacts_observed: 0,
                cleanup_checks_performed: recovery.cleanup_checks_performed,
                cleanup_unconfirmed: recovery.cleanup_unconfirmed,
                recovery_duration_ms: 0,
                binary_version: PackageVersion::try_new("0.0.1").unwrap(),
                schema_version: SchemaVersion::try_new(4).unwrap(),
                recovered_at: T5.parse().unwrap(),
            },
            event_id: JournalEventId::generate(),
            started_event_id,
            correlation_id,
        })
        .await
        .unwrap();
}

async fn corrupt_and_rehash_recovery_counter(fixture: &Fixture, field: &str) {
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let payload: String = sqlx::query_scalar(
        "SELECT payload_json FROM journal_events WHERE event_type = 'runtime.recovery_performed' \
         ORDER BY journal_offset DESC LIMIT 1",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    value[field] = (value[field].as_u64().unwrap() + 1).into();
    let reencoded = serde_json::to_string(&value).unwrap();
    let digest = Sha256Digest::hash_bytes(reencoded.as_bytes());
    sqlx::query(
        "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? \
         WHERE event_type = 'runtime.recovery_performed' AND journal_offset = \
         (SELECT MAX(journal_offset) FROM journal_events \
          WHERE event_type = 'runtime.recovery_performed')",
    )
    .bind(reencoded)
    .bind(digest.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
}

async fn request_shutdown_cancellation(fixture: &Fixture) {
    fixture
        .store
        .request_owned_work_cancellation(RequestOwnedCancellationRequest {
            runtime_id: fixture.runtime_id,
            requested_at: T4.parse().unwrap(),
        })
        .await
        .unwrap();
}

async fn recover_fixture(
    fixture: &Fixture,
    current_runtime_id: RuntimeInstanceId,
) -> RecoveryReceipt {
    fixture
        .store
        .recover_stale_runtime_ownership(RecoverStaleRuntimeRequest {
            stale_runtime_id: fixture.runtime_id,
            current_runtime_id,
            recovered_at: T5.parse().unwrap(),
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn stage10_model_requesting_streaming_and_cancel_requested_recovery_is_conservative() {
    for (streaming, cancellation_requested, draft_exposed) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let fixture = fixture().await;
        make_fixture_journal_consistent(&fixture).await;
        let model = if streaming {
            begin_and_stream_model(&fixture).await
        } else {
            begin_requesting_model(&fixture).await
        };
        if draft_exposed {
            let mut connection = fixture.store.runtime.acquire().await.unwrap();
            sqlx::query(
                "UPDATE model_invocations SET draft_exposed = 1 WHERE model_invocation_id = ?",
            )
            .bind(model.invocation_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
        }
        if cancellation_requested {
            request_shutdown_cancellation(&fixture).await;
            fixture
                .store
                .verify_application_consistency()
                .await
                .unwrap();
        }
        let current_runtime = create_stage10_recovery_runtime(&fixture).await;
        let recovery = recover_fixture(&fixture, current_runtime).await;
        assert_eq!(recovery.interrupted_work, 1);
        assert_eq!(recovery.model_attempts_provider_outcome_unknown, 1);
        assert_eq!(recovery.drafts_abandoned, u64::from(draft_exposed));
        assert_eq!(recovery.tool_attempts_outcome_unknown, 0);

        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        let attempt: (String, String) = sqlx::query_as(
            "SELECT state, completed_at FROM model_invocations WHERE model_invocation_id = ?",
        )
        .bind(model.invocation_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(attempt, ("provider_outcome_unknown".into(), T5.into()));
        let work: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT state, runtime_instance_id, current_model_invocation_id \
             FROM work_items WHERE work_id = ?",
        )
        .bind(fixture.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(work, ("interrupted".into(), None, None));
        let terminal_events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM journal_events WHERE work_id = ? AND event_type IN \
             ('model.invocation_interrupted','work.interrupted') ORDER BY journal_offset",
        )
        .bind(fixture.work_id.to_string())
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        assert_eq!(
            terminal_events,
            vec!["model.invocation_interrupted", "work.interrupted"]
        );
        drop(connection);
        fixture
            .store
            .verify_application_consistency()
            .await
            .unwrap();
        fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn stage10_tool_requested_dispatching_and_cancel_requested_recovery_is_conservative() {
    for (dispatching, cancellation_requested) in [(false, false), (true, false), (true, true)] {
        let fixture = fixture().await;
        make_fixture_journal_consistent(&fixture).await;
        let model = begin_and_stream_model(&fixture).await;
        complete_model(&fixture, &model).await;
        let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
        if dispatching {
            dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
        }
        if cancellation_requested {
            request_shutdown_cancellation(&fixture).await;
            fixture
                .store
                .verify_application_consistency()
                .await
                .unwrap();
        }
        let current_runtime = create_stage10_recovery_runtime(&fixture).await;
        let recovery = recover_fixture(&fixture, current_runtime).await;
        assert_eq!(recovery.interrupted_work, 1);
        assert_eq!(
            recovery.tool_attempts_interrupted_before_dispatch,
            u64::from(!dispatching)
        );
        assert_eq!(
            recovery.tool_attempts_outcome_unknown,
            u64::from(dispatching)
        );
        assert_eq!(recovery.cleanup_checks_performed, u64::from(dispatching));
        assert_eq!(recovery.cleanup_unconfirmed, u64::from(dispatching));

        let expected_state = if dispatching {
            "outcome_unknown"
        } else {
            "interrupted_before_dispatch"
        };
        let expected_event = if dispatching {
            "tool.execution_outcome_unknown"
        } else {
            "tool.execution_interrupted_before_dispatch"
        };
        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        let attempt: (String, Option<i64>) = sqlx::query_as(
            "SELECT state, cleanup_confirmed FROM tool_executions WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(attempt.0, expected_state);
        if dispatching {
            assert_eq!(attempt.1, Some(0));
        }
        let terminal_events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM journal_events WHERE work_id = ? AND event_type IN \
             ('tool.execution_interrupted_before_dispatch','tool.execution_outcome_unknown',\
              'work.interrupted') ORDER BY journal_offset",
        )
        .bind(fixture.work_id.to_string())
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        assert_eq!(terminal_events, vec![expected_event, "work.interrupted"]);
        drop(connection);
        fixture
            .store
            .verify_application_consistency()
            .await
            .unwrap();
        fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn stage10_completed_attempt_evidence_is_preserved_and_stale_work_is_not_resumed() {
    let fixture = fixture().await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let current_runtime = create_stage10_recovery_runtime(&fixture).await;
    let recovery = recover_fixture(&fixture, current_runtime).await;
    assert_eq!(recovery.interrupted_work, 1);
    assert_eq!(recovery.model_attempts_provider_outcome_unknown, 0);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let model_state: String =
        sqlx::query_scalar("SELECT state FROM model_invocations WHERE model_invocation_id = ?")
            .bind(model.invocation_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(model_state, "completed");
    let work_state: String = sqlx::query_scalar("SELECT state FROM work_items WHERE work_id = ?")
        .bind(fixture.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(work_state, "interrupted");
    let resumed_after_recovery: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type = 'work.resumed' \
         AND recorded_at = ?",
    )
    .bind(fixture.work_id.to_string())
    .bind(T5)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(resumed_after_recovery, 0);
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn recovery_summary_interrupted_and_model_counter_corruption_fail_closed() {
    for field in [
        "interrupted_work",
        "model_attempts_provider_outcome_unknown",
    ] {
        let fixture = fixture().await;
        make_fixture_journal_consistent(&fixture).await;
        begin_and_stream_model(&fixture).await;
        let (current_runtime, started_event_id, correlation_id) =
            create_stage10_recovery_runtime_with_evidence(&fixture).await;
        let recovery = recover_fixture(&fixture, current_runtime).await;
        append_stage10_recovery_summary(
            &fixture,
            current_runtime,
            started_event_id,
            correlation_id,
            recovery,
        )
        .await;
        fixture
            .store
            .verify_application_consistency()
            .await
            .unwrap();

        corrupt_and_rehash_recovery_counter(&fixture, field).await;
        assert!(
            fixture
                .store
                .verify_application_consistency()
                .await
                .is_err(),
            "corrupted exact recovery counter was accepted: {field}"
        );
        fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn recovery_summary_tool_counter_corruption_fails_closed() {
    for (dispatching, field) in [
        (false, "tool_attempts_interrupted_before_dispatch"),
        (true, "tool_attempts_outcome_unknown"),
    ] {
        let fixture = fixture().await;
        make_fixture_journal_consistent(&fixture).await;
        let model = begin_and_stream_model(&fixture).await;
        complete_model(&fixture, &model).await;
        let (tool_id, requested_event, waiting) = request_tool(&fixture, &model, 1, 4).await;
        if dispatching {
            dispatch_tool(&fixture, tool_id, requested_event, waiting).await;
        }
        let (current_runtime, started_event_id, correlation_id) =
            create_stage10_recovery_runtime_with_evidence(&fixture).await;
        let recovery = recover_fixture(&fixture, current_runtime).await;
        append_stage10_recovery_summary(
            &fixture,
            current_runtime,
            started_event_id,
            correlation_id,
            recovery,
        )
        .await;
        fixture
            .store
            .verify_application_consistency()
            .await
            .unwrap();

        corrupt_and_rehash_recovery_counter(&fixture, field).await;
        assert!(
            fixture
                .store
                .verify_application_consistency()
                .await
                .is_err(),
            "corrupted exact recovery counter was accepted: {field}"
        );
        fixture.guard.shutdown().await;
    }
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
#[ignore = "subprocess-only Stage 14 crash-window worker"]
async fn stage14_crash_window_child() {
    if std::env::var("CRAXII_STAGE14_CRASH_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let root = PathBuf::from(std::env::var_os("CRAXII_STAGE14_CRASH_ROOT").unwrap());
    let hook = std::env::var("CRAXII_STAGE14_CRASH_HOOK").unwrap();
    let fixture = fixture_at(TestRoot::at(root.clone())).await;
    make_fixture_journal_consistent(&fixture).await;
    let model = begin_and_stream_model(&fixture).await;
    complete_model(&fixture, &model).await;
    let workspace_root = root.join("workspace");
    let cwd = workspace_root.join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(workspace_root.join("small.txt"), b"small\n").unwrap();
    fs::write(workspace_root.join("large.txt"), vec![b'x'; 40_000]).unwrap();

    let snapshot = fixture.store.load_bootstrap_snapshot().await.unwrap();
    let call_workspace = WorkspaceIdentity::try_new(WorkspaceIdentityInput {
        workspace_id: fixture.identity.workspace_id,
        craxii_id: fixture.identity.craxii_id,
        workstation_id: fixture.identity.workstation_id,
        logical_name: "primary".to_owned(),
        logical_root: LogicalPathReference::absolute(workspace_root.to_str().unwrap()).unwrap(),
        created_at: T0.parse().unwrap(),
    })
    .unwrap();
    let clock = Arc::new(TestClock::new(
        T4.parse::<UtcTimestamp>().unwrap().to_offset_datetime(),
        Duration::from_secs(10),
    ));
    let artifact_store = Arc::new(LocalArtifactStore::initialize(&root.join("artifacts")).unwrap());
    let workstation_clock: Arc<dyn Clock> = clock.clone();
    let workstation_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
    let local_workstation = Arc::new(
        LocalWorkstation::new(
            &snapshot.workstation,
            &snapshot.workspace,
            LocalWorkstationOptions {
                default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
                configured_workspace_root: workspace_root.clone(),
                read_hard_limit: HARD_FILE_READ_MAX_BYTES,
                artifact_store: workstation_artifacts,
                administrative_enabled: false,
                delegated_cgroup_root: None,
                clock: workstation_clock,
            },
        )
        .unwrap(),
    );
    let policy = ToolSemanticPolicy {
        read_file_default_bytes: 1_048_576,
        read_file_max_bytes: HARD_FILE_READ_MAX_BYTES,
        run_shell_command_max_bytes: 65_536,
        run_shell_default_timeout_ms: 120_000,
        run_shell_max_timeout_ms: 900_000,
    };
    let registry = Arc::new(ToolRegistry::v0(policy).unwrap());
    let authority: Arc<dyn AuthorityEvaluator> = Arc::new(V0AuthorityEvaluator);
    let state_store: Arc<dyn ToolStateStore> = Arc::new(fixture.store.clone());
    let workstation: Arc<dyn Workstation> = local_workstation.clone();
    let preparation: Arc<dyn WorkstationPreparation> = local_workstation;
    let service_artifacts: Arc<dyn ArtifactStore> = artifact_store;
    let service_clock: Arc<dyn Clock> = clock.clone();
    let service = ToolExecutionService::new(
        registry,
        authority,
        state_store,
        workstation,
        preparation,
        service_artifacts,
        service_clock,
        ToolRuntimeLimits {
            read_file_default_bytes: policy.read_file_default_bytes,
            read_file_max_bytes: policy.read_file_max_bytes,
            run_shell_command_max_bytes: policy.run_shell_command_max_bytes,
            run_shell_default_timeout_ms: policy.run_shell_default_timeout_ms,
            run_shell_max_timeout_ms: policy.run_shell_max_timeout_ms,
            stdout_capture_bytes: 1_048_576,
            stderr_capture_bytes: 1_048_576,
            inline_model_result_bytes: 65_536,
            per_stream_projection_bytes: 32_768,
        },
    )
    .unwrap();

    let source_model_event_id = {
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        let id: String = sqlx::query_scalar(
            "SELECT event_id FROM journal_events WHERE work_id = ? \
             AND event_type = 'model.invocation_completed' ORDER BY journal_offset DESC LIMIT 1",
        )
        .bind(fixture.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        JournalEventId::parse_canonical(&id).unwrap()
    };
    let side_effect = root.join("machine-side-effect.marker");
    let terminal = root.join("terminal-observed.marker");
    let (tool_name, raw_arguments) = match hook.as_str() {
        "after_tool_requested_commit" | "after_tool_dispatch_intent_commit" => (
            "read_file",
            serde_json::to_vec(&serde_json::json!({"path":"small.txt"})).unwrap(),
        ),
        "after_tool_process_spawn" | "after_tool_process_exit_before_outcome_commit" => {
            let command = format!(
                "printf 'one\\n' >> {}; printf 'definite\\n' > {}",
                shell_quote_path(&side_effect),
                shell_quote_path(&terminal),
            );
            (
                "run_shell",
                serde_json::to_vec(&serde_json::json!({"command":command,"cwd":"cwd"})).unwrap(),
            )
        }
        "after_artifact_rename_before_db_commit" => (
            "read_file",
            serde_json::to_vec(&serde_json::json!({"path":"large.txt"})).unwrap(),
        ),
        _ => panic!("unknown Stage 14 crash hook"),
    };
    let invocation_now = clock.monotonic_now();
    let work_deadline = MonotonicInstant::from_elapsed(
        invocation_now
            .elapsed()
            .checked_add(Duration::from_secs(30))
            .unwrap(),
    );
    let call = ToolExecutionCall {
        craxii_id: fixture.identity.craxii_id,
        work: resumed(&fixture, 4),
        runtime_instance_id: fixture.runtime_id,
        source_model_invocation_id: model.invocation_id,
        source_model_event_id,
        agent_step_no: AgentStepNo::try_new(1).unwrap(),
        tool_ordinal: ToolOrdinal::try_new(1).unwrap(),
        provider_tool_call_id: Some("stage14-crash-call-1".to_owned()),
        tool_name: tool_name.to_owned(),
        raw_arguments,
        correlation_id: fixture.correlation_id,
        workstation_id: fixture.identity.workstation_id,
        workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
        workspace: call_workspace,
        work_deadline,
        shutdown_deadline: None,
        authority_constraints: Default::default(),
        cancellation: None,
    };

    // SAFETY: this disposable subprocess selects its production failpoint immediately before the
    // sole service call. No concurrent test in this process reads or mutates the environment.
    unsafe { std::env::set_var("CRAXII_TEST_ABORT_AT_FAILPOINT", &hook) };
    let result = service.execute_call(call).await;
    std::mem::forget(fixture);
    panic!("selected Stage 14 production crash failpoint did not abort: {result:?}");
}

#[cfg(feature = "test-failpoints")]
fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(feature = "test-failpoints")]
fn stage14_side_effect_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .ok()
        .map_or(0, |contents| contents.lines().count())
}

#[cfg(feature = "test-failpoints")]
async fn wait_for_stage14_shell_completion(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.is_file() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("spawned crash-window shell did not terminate");
}

#[cfg(feature = "test-failpoints")]
async fn verify_stage14_crash_window(hook: &str) {
    let root_path = std::env::temp_dir().join(format!(
        "craxii-stage14-crash-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "adapters::sqlite::stage8_tests::stage14_crash_window_child",
        ])
        .env("CRAXII_STAGE14_CRASH_CHILD", "1")
        .env("CRAXII_STAGE14_CRASH_ROOT", &root_path)
        .env("CRAXII_STAGE14_CRASH_HOOK", hook)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "crash child unexpectedly survived"
    );
    assert!(
        root_path.join("db/craxii.sqlite3").is_file(),
        "crash child did not create its database; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let side_effect = root_path.join("machine-side-effect.marker");
    let terminal_marker = root_path.join("terminal-observed.marker");
    if matches!(
        hook,
        "after_tool_process_spawn" | "after_tool_process_exit_before_outcome_commit"
    ) {
        wait_for_stage14_shell_completion(&terminal_marker).await;
    }
    let side_effect_count_before_recovery = stage14_side_effect_count(&side_effect);
    if hook == "after_tool_process_spawn" {
        assert!(side_effect_count_before_recovery <= 1);
    } else if hook == "after_tool_process_exit_before_outcome_commit" {
        assert_eq!(side_effect_count_before_recovery, 1);
    } else {
        assert_eq!(side_effect_count_before_recovery, 0);
    }

    let root = TestRoot::at(root_path);
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let artifact_store = LocalArtifactStore::initialize(&root.path().join("artifacts")).unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT t.tool_execution_id, t.runtime_instance_id, t.work_id, t.state, \
         w.craxii_id, w.conversation_id, w.workspace_id, w.correlation_id, \
         r.workstation_id FROM tool_executions t \
         JOIN work_items w ON w.work_id = t.work_id \
         JOIN runtime_instances r ON r.runtime_instance_id = t.runtime_instance_id",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let tool_id =
        ToolExecutionId::parse_canonical(&row.get::<String, _>("tool_execution_id")).unwrap();
    let old_runtime =
        RuntimeInstanceId::parse_canonical(&row.get::<String, _>("runtime_instance_id")).unwrap();
    let work_id = WorkId::parse_canonical(&row.get::<String, _>("work_id")).unwrap();
    let identity = V0IdentityReference {
        craxii_id: CraxiiId::parse_canonical(&row.get::<String, _>("craxii_id")).unwrap(),
        conversation_id: ConversationId::parse_canonical(&row.get::<String, _>("conversation_id"))
            .unwrap(),
        workstation_id: WorkstationId::parse_canonical(&row.get::<String, _>("workstation_id"))
            .unwrap(),
        workspace_id: WorkspaceId::parse_canonical(&row.get::<String, _>("workspace_id")).unwrap(),
    };
    let correlation_id =
        CorrelationId::parse_canonical(&row.get::<String, _>("correlation_id")).unwrap();
    let dispatched = hook != "after_tool_requested_commit";
    assert_eq!(
        row.get::<String, _>("state"),
        if dispatched {
            "dispatching"
        } else {
            "requested"
        },
        "crash child returned without reaching the selected hook; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let work_projection: (String, Option<String>) =
        sqlx::query_as("SELECT state, current_tool_execution_id FROM work_items WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(work_projection.0, "waiting_on_tool");
    let tool_id_text = tool_id.to_string();
    assert_eq!(work_projection.1.as_deref(), Some(tool_id_text.as_str()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tool_executions WHERE tool_execution_id = ? \
             AND completed_at IS NULL AND result_json IS NULL",
        )
        .bind(tool_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE work_id = ? \
             AND event_type = 'tool.execution_dispatching'",
        )
        .bind(work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        i64::from(dispatched)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type IN \
             ('tool.execution_interrupted_before_dispatch','tool.execution_outcome_unknown')",
        )
        .bind(work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        0
    );
    drop(connection);

    assert_eq!(
        terminal_marker.is_file(),
        matches!(
            hook,
            "after_tool_process_spawn" | "after_tool_process_exit_before_outcome_commit"
        )
    );

    let fixture = Fixture {
        _root: root,
        guard,
        store,
        artifact_store,
        identity,
        runtime_id: old_runtime,
        work_id,
        correlation_id,
    };
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let current_runtime = create_stage10_recovery_runtime(&fixture).await;
    let recovery = recover_fixture(&fixture, current_runtime).await;
    assert_eq!(recovery.interrupted_work, 1);
    assert_eq!(
        recovery.tool_attempts_interrupted_before_dispatch,
        u64::from(!dispatched)
    );
    assert_eq!(
        recovery.tool_attempts_outcome_unknown,
        u64::from(dispatched)
    );
    assert_eq!(
        stage14_side_effect_count(&side_effect),
        side_effect_count_before_recovery,
        "startup recovery redispatched the command"
    );
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tool_executions WHERE tool_execution_id = ?",
        )
        .bind(tool_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        if dispatched {
            "outcome_unknown"
        } else {
            "interrupted_before_dispatch"
        }
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type IN \
             ('tool.execution_interrupted_before_dispatch','tool.execution_outcome_unknown')",
        )
        .bind(work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        1
    );
    let recovered_work: (String, Option<String>) =
        sqlx::query_as("SELECT state, current_tool_execution_id FROM work_items WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(recovered_work, ("interrupted".to_owned(), None));
    drop(connection);
    if hook == "after_artifact_rename_before_db_commit" {
        let referenced = fixture.store.load_referenced_artifacts().await.unwrap();
        let keys = referenced
            .into_iter()
            .map(|reference| reference.storage_key().clone())
            .collect();
        let report = fixture
            .artifact_store
            .scan_orphans(&keys, T5.parse().unwrap())
            .unwrap();
        assert_eq!(report.orphans.len(), 1);
    }
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn crash_after_tool_requested_commit_recovers_without_redispatch() {
    verify_stage14_crash_window("after_tool_requested_commit").await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn crash_after_tool_dispatch_intent_commit_recovers_outcome_unknown() {
    verify_stage14_crash_window("after_tool_dispatch_intent_commit").await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn crash_after_tool_process_spawn_records_one_side_effect() {
    verify_stage14_crash_window("after_tool_process_spawn").await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn crash_after_tool_process_exit_preserves_definite_observation_marker() {
    verify_stage14_crash_window("after_tool_process_exit_before_outcome_commit").await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn crash_after_artifact_rename_recovers_and_reports_one_orphan() {
    verify_stage14_crash_window("after_artifact_rename_before_db_commit").await;
}
