use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Barrier;

use crate::application::authentication::DeviceAuthenticator;
use crate::application::command_service::{
    AcceptMessageCommand, CancelWorkCommand, CommandService, CommandServiceErrorKind,
};
use crate::application::device_provisioning::{
    DeviceAdministrationErrorKind, DeviceProvisioningService,
};
use crate::domain::{
    AuthenticatedDevice, BearerToken, CancellationCommandReceipt, ClientCommandId, ClientMessageId,
    ContentBlock, ConversationId, CorrelationId, CraxiiId, CurrentWorkAttempt, DeviceDisplayName,
    DeviceId, DiagnosticPid, GitRevision, IdempotencyKey, JournalActor, JournalCurrentAttempt,
    JournalEventId, JournalEventPayload, JournalStreamId, LinuxBootId, MessageContent,
    PackageVersion, ProjectionVersion, RuntimeInstanceId, RuntimeStartEvidence,
    RuntimeStartEvidenceInput, SchemaVersion, UtcTimestamp, WorkId, WorkLifecycleSnapshot,
    WorkLifecycleSnapshotInput, WorkState, WorkTransitionV1, WorkspaceId, WorkstationGeneration,
    WorkstationId,
};
use crate::ports::device_credentials::RevokeDeviceOutcome;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, CreateRuntimeRequest,
    LoadOrBootstrapIdentityRequest, RuntimeStateStore, StateStoreErrorKind, V0IdentityReference,
};

use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::projection::{WorkProjectionTimes, guarded_work_update};
use super::runtime::SqliteRuntimeGuard;
use super::stage9::Stage9TestHook;
use super::state_store::SqliteStateStore;
use super::transaction::WriteTransaction;

const T0: &str = "2026-08-28T01:02:03.000001Z";
const T1: &str = "2026-08-28T01:02:04.000001Z";
const T2: &str = "2026-08-28T01:02:05.000001Z";
const T3: &str = "2026-08-28T01:02:06.000001Z";
const SENTINEL: &str = "abababababababababababababababababababababababababababababababab";

trait FreshClientId {
    fn generate() -> Self;
}

impl FreshClientId for ClientMessageId {
    fn generate() -> Self {
        Self::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
    }
}

impl FreshClientId for ClientCommandId {
    fn generate() -> Self {
        Self::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
    }
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage9-test-{}-{}",
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

struct Fixture {
    root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: SqliteStateStore,
    identity: V0IdentityReference,
    device_id: DeviceId,
    token_text: String,
}

fn timestamp(value: &str) -> UtcTimestamp {
    value.parse().unwrap()
}

fn observation() -> BootstrapObservation {
    BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(1).unwrap(),
        architecture: "stage9-test-architecture".into(),
        os_release: "stage9-test-os".into(),
        default_shell: "/bin/sh".into(),
        workspace_logical_name: "primary".into(),
        workspace_logical_root: "/workspace".into(),
        workspace_resolved_root: "/tmp/craxii-stage9-workspace".into(),
    }
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
        created_at: timestamp(T0),
        observation: observation(),
    }
}

async fn fixture() -> Fixture {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let identity = store
        .load_or_bootstrap_v0_identity(bootstrap_request())
        .await
        .unwrap()
        .identity;
    let provisioned = DeviceProvisioningService::new(&store)
        .provision_fixture_token(
            DeviceDisplayName::try_new("Stage 9 device".into()).unwrap(),
            timestamp(T0),
            BearerToken::parse(SENTINEL.to_owned()).unwrap(),
        )
        .await
        .unwrap();
    let device_id = provisioned.summary.device_id;
    let mut token = Vec::new();
    provisioned.write_bearer_once(&mut token).unwrap();
    let token_text = String::from_utf8(token).unwrap();
    assert_eq!(token_text, format!("{SENTINEL}\n"));
    Fixture {
        root,
        guard,
        store,
        identity,
        device_id,
        token_text: token_text.trim_end().to_owned(),
    }
}

fn content(text: &str) -> MessageContent {
    MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap()
}

fn message_command(
    identity: V0IdentityReference,
    client_message_id: ClientMessageId,
    text: &str,
    accepted_at: UtcTimestamp,
) -> AcceptMessageCommand {
    AcceptMessageCommand {
        idempotency_key: IdempotencyKey::for_message(client_message_id),
        client_message_id,
        conversation_id: identity.conversation_id,
        content: content(text),
        accepted_at,
    }
}

fn cancel_command(
    client_command_id: ClientCommandId,
    work_id: WorkId,
    requested_at: UtcTimestamp,
) -> CancelWorkCommand {
    CancelWorkCommand {
        idempotency_key: IdempotencyKey::for_cancellation(client_command_id),
        client_command_id,
        work_id,
        requested_at,
    }
}

async fn counts(store: &SqliteStateStore) -> (i64, i64, i64, i64, i64) {
    let mut connection = store.runtime.acquire().await.unwrap();
    let mut values = Vec::new();
    for table in [
        "messages",
        "work_items",
        "work_item_inputs",
        "journal_events",
        "client_commands",
    ] {
        values.push(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        );
    }
    (values[0], values[1], values[2], values[3], values[4])
}

async fn accept(
    store: &SqliteStateStore,
    device_id: DeviceId,
    identity: V0IdentityReference,
    client_message_id: ClientMessageId,
    text: &str,
) -> crate::domain::CommandOutcome<crate::domain::MessageCommandReceipt> {
    CommandService::new(store)
        .accept_message(
            AuthenticatedDevice::new(device_id),
            message_command(identity, client_message_id, text, timestamp(T1)),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn provision_auth_touch_revoke_and_secret_storage_contract_is_exact() {
    let fixture = fixture().await;
    assert_eq!(counts(&fixture.store).await, (0, 0, 0, 2, 0));

    let authenticated = DeviceAuthenticator::new(&fixture.store)
        .authenticate(
            BearerToken::parse(fixture.token_text.clone()).unwrap(),
            timestamp(T2),
        )
        .await
        .unwrap();
    assert_eq!(authenticated.device_id(), fixture.device_id);
    let devices = DeviceProvisioningService::new(&fixture.store)
        .list()
        .await
        .unwrap();
    assert_eq!(devices.len(), 1);
    assert!(devices[0].is_active());
    assert_eq!(devices[0].last_seen_at, Some(timestamp(T2)));

    DeviceAuthenticator::new(&fixture.store)
        .authenticate(
            BearerToken::parse(fixture.token_text.clone()).unwrap(),
            timestamp(T1),
        )
        .await
        .unwrap();
    assert_eq!(
        DeviceProvisioningService::new(&fixture.store)
            .list()
            .await
            .unwrap()[0]
            .last_seen_at,
        Some(timestamp(T2))
    );

    let duplicate = DeviceProvisioningService::new(&fixture.store)
        .provision_fixture_token(
            DeviceDisplayName::try_new("Duplicate".into()).unwrap(),
            timestamp(T1),
            BearerToken::parse(fixture.token_text.clone()).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        duplicate.kind(),
        DeviceAdministrationErrorKind::CredentialConflict
    );

    let service = DeviceProvisioningService::new(&fixture.store);
    assert!(matches!(
        service
            .revoke(fixture.device_id, timestamp(T2))
            .await
            .unwrap(),
        RevokeDeviceOutcome::Revoked(_)
    ));
    let first_revoked_at = service.list().await.unwrap()[0].revoked_at;
    assert!(matches!(
        service
            .revoke(fixture.device_id, timestamp(T1))
            .await
            .unwrap(),
        RevokeDeviceOutcome::AlreadyRevoked(_)
    ));
    assert_eq!(
        service.list().await.unwrap()[0].revoked_at,
        first_revoked_at
    );
    let error = DeviceAuthenticator::new(&fixture.store)
        .authenticate(
            BearerToken::parse(fixture.token_text.clone()).unwrap(),
            timestamp(T2),
        )
        .await
        .unwrap_err();
    assert_eq!(format!("{error}"), "authentication_failed");
    assert_eq!(format!("{error:?}"), "authentication_failed");
    assert!(!format!("{error:?}").contains(SENTINEL));
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();

    fixture.guard.runtime().checkpoint_passive().await.unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let path = fixture
            .root
            .path()
            .join(format!("db/craxii.sqlite3{suffix}"));
        if path.exists() {
            let bytes = fs::read(path).unwrap();
            assert!(
                !bytes
                    .windows(SENTINEL.len())
                    .any(|window| window == SENTINEL.as_bytes())
            );
        }
    }
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn message_commit_replay_conflict_event_order_and_future_isolation_are_atomic() {
    let fixture = fixture().await;
    let first_id = ClientMessageId::generate();
    let first = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        first_id,
        "first é",
    )
    .await;
    assert!(!first.is_replay());
    let first_receipt = first.into_receipt();
    assert_eq!(first_receipt.work_ordinal.get(), 1);
    assert_eq!(counts(&fixture.store).await, (1, 1, 1, 4, 1));

    let replay = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        first_id,
        "first é",
    )
    .await;
    assert!(replay.is_replay());
    assert_eq!(replay.into_receipt(), first_receipt);
    assert_eq!(counts(&fixture.store).await, (1, 1, 1, 4, 1));

    let conflict = CommandService::new(&fixture.store)
        .accept_message(
            AuthenticatedDevice::new(fixture.device_id),
            message_command(fixture.identity, first_id, "changed", timestamp(T1)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict.kind(),
        CommandServiceErrorKind::IdempotencyConflict
    );
    let cross_kind_id = ClientCommandId::parse_canonical(&first_id.to_string()).unwrap();
    let cross_kind = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(cross_kind_id, first_receipt.work_id, timestamp(T2)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        cross_kind.kind(),
        CommandServiceErrorKind::IdempotencyConflict
    );

    let second_id = ClientMessageId::generate();
    let second = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        second_id,
        "future",
    )
    .await
    .into_receipt();
    assert_eq!(second.work_ordinal.get(), 2);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let rows = sqlx::query("SELECT work_id, input_event_id FROM work_item_inputs ORDER BY work_id")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let first_input: String =
        sqlx::query_scalar("SELECT input_event_id FROM work_item_inputs WHERE work_id = ?")
            .bind(first_receipt.work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    let second_input: String =
        sqlx::query_scalar("SELECT input_event_id FROM work_item_inputs WHERE work_id = ?")
            .bind(second.work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_ne!(first_input, second_input);
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn every_message_precommit_failure_boundary_rolls_back_all_durable_truth() {
    let fixture = fixture().await;
    for hook in [
        Stage9TestHook::AfterMessageInsert,
        Stage9TestHook::AfterMessageAccepted,
        Stage9TestHook::AfterWorkInsert,
        Stage9TestHook::AfterWorkInput,
        Stage9TestHook::AfterWorkQueued,
        Stage9TestHook::AfterConversationAdvance,
        Stage9TestHook::BeforeClientCommandInsert,
        Stage9TestHook::AfterClientCommandInsert,
    ] {
        fixture.store.set_stage9_test_hook(Some(hook));
        let error = CommandService::new(&fixture.store)
            .accept_message(
                AuthenticatedDevice::new(fixture.device_id),
                message_command(
                    fixture.identity,
                    ClientMessageId::generate(),
                    "must roll back",
                    timestamp(T1),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), CommandServiceErrorKind::StorageInconsistent);
        fixture.store.set_stage9_test_hook(None);
        assert_eq!(counts(&fixture.store).await, (0, 0, 0, 2, 0));
        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        let (ordinal, version): (i64, i64) = sqlx::query_as(
            "SELECT next_work_ordinal, state_version FROM conversations WHERE conversation_id = ?",
        )
        .bind(fixture.identity.conversation_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!((ordinal, version), (1, 1));
    }
    fixture.guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_messages_serialize_to_contiguous_ordinals_without_shared_inputs() {
    let fixture = fixture().await;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(Barrier::new(9));
    let mut tasks = Vec::new();
    for index in 0..8 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let identity = fixture.identity;
        let device_id = fixture.device_id;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            accept(
                &store,
                device_id,
                identity,
                ClientMessageId::generate(),
                &format!("message {index}"),
            )
            .await
            .into_receipt()
        }));
    }
    barrier.wait().await;
    let mut receipts = Vec::new();
    for task in tasks {
        receipts.push(task.await.unwrap());
    }
    let mut ordinals = receipts
        .iter()
        .map(|receipt| receipt.work_ordinal.get())
        .collect::<Vec<_>>();
    ordinals.sort_unstable();
    assert_eq!(ordinals, (1_i64..=8).collect::<Vec<_>>());
    assert_eq!(counts(&store).await, (8, 8, 8, 18, 8));
    let mut connection = store.runtime.acquire().await.unwrap();
    let distinct_inputs: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT input_event_id) FROM work_item_inputs")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    let (next, version): (i64, i64) =
        sqlx::query_as("SELECT next_work_ordinal, state_version FROM conversations")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(distinct_inputs, 8);
    assert_eq!((next, version), (9, 9));
    drop(connection);
    store.verify_application_consistency().await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_message_key_has_one_winner_and_exact_replay() {
    let fixture = fixture().await;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(Barrier::new(3));
    let client_message_id = ClientMessageId::generate();
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let identity = fixture.identity;
        let device = fixture.device_id;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            CommandService::new(&*store)
                .accept_message(
                    AuthenticatedDevice::new(device),
                    message_command(identity, client_message_id, "same", timestamp(T1)),
                )
                .await
                .unwrap()
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_ne!(first.is_replay(), second.is_replay());
    assert_eq!(first.receipt(), second.receipt());
    assert_eq!(counts(&store).await, (1, 1, 1, 4, 1));
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn message_key_body_mismatch_and_duplicate_identity_bypass_persist_nothing() {
    let fixture = fixture().await;
    let client_message_id = ClientMessageId::generate();
    let mismatched = CommandService::new(&fixture.store)
        .accept_message(
            AuthenticatedDevice::new(fixture.device_id),
            AcceptMessageCommand {
                idempotency_key: IdempotencyKey::for_message(ClientMessageId::generate()),
                client_message_id,
                conversation_id: fixture.identity.conversation_id,
                content: content("invalid"),
                accepted_at: timestamp(T1),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        mismatched.kind(),
        CommandServiceErrorKind::CommandValidationFailed
    );
    assert_eq!(counts(&fixture.store).await, (0, 0, 0, 2, 0));

    accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        client_message_id,
        "winner",
    )
    .await;
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    sqlx::query("DELETE FROM client_commands WHERE device_id = ? AND idempotency_key = ?")
        .bind(fixture.device_id.to_string())
        .bind(client_message_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    let bypass = CommandService::new(&fixture.store)
        .accept_message(
            AuthenticatedDevice::new(fixture.device_id),
            message_command(fixture.identity, client_message_id, "winner", timestamp(T1)),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        bypass.kind(),
        CommandServiceErrorKind::StorageFailure | CommandServiceErrorKind::StorageInconsistent
    ));
    assert_eq!(counts(&fixture.store).await, (1, 1, 1, 4, 0));
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn queued_cancellation_replay_terminal_noop_conflict_and_not_found_are_durable() {
    let fixture = fixture().await;
    let work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "cancel this",
    )
    .await
    .into_receipt()
    .work_id;
    let command_id = ClientCommandId::generate();
    let cancelled = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(command_id, work, timestamp(T2)),
        )
        .await
        .unwrap();
    assert!(!cancelled.is_replay());
    assert_eq!(
        cancelled.receipt().resulting_work_state,
        WorkState::Cancelled
    );
    assert_eq!(cancelled.receipt().http_status(), 200);
    assert!(!cancelled.receipt().cleanup.is_pending());
    let cancelled_receipt = cancelled.into_receipt();

    let replay = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(command_id, work, timestamp(T2)),
        )
        .await
        .unwrap();
    assert!(replay.is_replay());
    assert_eq!(replay.into_receipt(), cancelled_receipt);

    let no_op = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(ClientCommandId::generate(), work, timestamp(T2)),
        )
        .await
        .unwrap();
    assert_eq!(no_op.receipt().resulting_work_state, WorkState::Cancelled);
    assert_eq!(
        no_op.receipt().committed_cursor,
        cancelled_receipt.committed_cursor
    );
    let cancellation_events: i64 = {
        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancelled'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap()
    };
    assert_eq!(cancellation_events, 1);

    let other_work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "other",
    )
    .await
    .into_receipt()
    .work_id;
    let conflict = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(command_id, other_work, timestamp(T2)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict.kind(),
        CommandServiceErrorKind::IdempotencyConflict
    );
    let missing = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(
                ClientCommandId::generate(),
                WorkId::generate(),
                timestamp(T2),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.kind(), CommandServiceErrorKind::TargetNotFound);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

async fn transition_to_running(fixture: &Fixture, work_id: WorkId) -> RuntimeInstanceId {
    let runtime_id = RuntimeInstanceId::generate();
    fixture
        .store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence: RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: fixture.identity.craxii_id,
                workstation_id: fixture.identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("stage9-test-boot").unwrap()),
                diagnostic_pid: Some(DiagnosticPid::try_new(42).unwrap()),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: GitRevision::try_new("stage9-test").unwrap(),
                schema_version: SchemaVersion::try_new(3).unwrap(),
                started_at: timestamp(T1),
            }),
            event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
        })
        .await
        .unwrap();

    let current = WorkLifecycleSnapshot::initial(work_id);
    let next = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id,
        state: WorkState::Running,
        projection_version: ProjectionVersion::try_new(2).unwrap(),
        runtime_owner: Some(runtime_id),
        current_attempt: CurrentWorkAttempt::None,
        cancellation_reason: None,
        terminal_reason: None,
    })
    .unwrap();
    let mut transaction =
        WriteTransaction::begin(fixture.guard.runtime(), "stage9_running_fixture")
            .await
            .unwrap();
    guarded_work_update(
        &mut transaction,
        &current,
        &next,
        WorkProjectionTimes {
            started_at: Some(timestamp(T1)),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .unwrap();
    let queued_id: String = sqlx::query_scalar(
        "SELECT event_id FROM journal_events WHERE stream_id = ? ORDER BY stream_seq DESC LIMIT 1",
    )
    .bind(JournalStreamId::Work(work_id).to_string())
    .fetch_one(transaction.connection())
    .await
    .unwrap();
    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: JournalEventId::generate(),
            craxii_id: fixture.identity.craxii_id,
            stream_id: JournalStreamId::Work(work_id),
            conversation_id: Some(fixture.identity.conversation_id),
            work_id: Some(work_id),
            causation_event_id: Some(queued_id.parse().unwrap()),
            correlation_id: CorrelationId::for_work(work_id),
            actor: JournalActor::Runtime(runtime_id),
            runtime_instance_id: Some(runtime_id),
            payload: JournalEventPayload::WorkStarted(WorkTransitionV1 {
                work_id,
                from_state: WorkState::Queued,
                to_state: WorkState::Running,
                expected_state_version: ProjectionVersion::try_new(1).unwrap(),
                expected_runtime_owner: None,
                expected_current_attempt: JournalCurrentAttempt::None,
                expected_cancellation_reason: None,
                state_version: ProjectionVersion::try_new(2).unwrap(),
                runtime_owner: Some(runtime_id),
                current_attempt: JournalCurrentAttempt::None,
                cancellation_reason: None,
                terminal_reason: None,
                transitioned_at: timestamp(T1),
            }),
            recorded_at: timestamp(T1),
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    runtime_id
}

#[derive(Clone, Copy)]
enum CancellationTransitionKind {
    Queued,
    Active,
}

struct PersistedCancellation {
    fixture: Fixture,
    work_id: WorkId,
    command_id: ClientCommandId,
    receipt: CancellationCommandReceipt,
}

async fn persisted_cancellation(kind: CancellationTransitionKind) -> PersistedCancellation {
    let fixture = fixture().await;
    let work_id = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "startup cancellation evidence",
    )
    .await
    .into_receipt()
    .work_id;
    if matches!(kind, CancellationTransitionKind::Active) {
        transition_to_running(&fixture, work_id).await;
    }
    let command_id = ClientCommandId::generate();
    let receipt = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(command_id, work_id, timestamp(T2)),
        )
        .await
        .unwrap()
        .into_receipt();
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    PersistedCancellation {
        fixture,
        work_id,
        command_id,
        receipt,
    }
}

async fn work_event_value(
    fixture: &Fixture,
    work_id: WorkId,
    event_type: &str,
    column: &str,
) -> String {
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
        "SELECT {column} FROM journal_events WHERE event_type = ? AND (work_id = ? OR correlation_id = ?) ORDER BY journal_offset ASC LIMIT 1"
    )))
    .bind(event_type)
    .bind(work_id.to_string())
    .bind(CorrelationId::for_work(work_id).to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap()
}

async fn corrupt_cancellation_cursor(persisted: &PersistedCancellation, earlier_event_type: &str) {
    let earlier_offset = work_event_value(
        &persisted.fixture,
        persisted.work_id,
        earlier_event_type,
        "CAST(journal_offset AS TEXT)",
    )
    .await
    .parse::<i64>()
    .unwrap();
    assert!(earlier_offset < persisted.receipt.committed_cursor.get());
    let mut connection = persisted.fixture.store.runtime.acquire().await.unwrap();
    sqlx::query(
        "UPDATE client_commands SET committed_cursor = ?, \
         response_json = json_set(response_json, '$.committed_cursor', ?) \
         WHERE device_id = ? AND idempotency_key = ? AND command_type = 'cancel'",
    )
    .bind(earlier_offset)
    .bind(earlier_offset)
    .bind(persisted.fixture.device_id.to_string())
    .bind(persisted.command_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
}

async fn corrupt_cancellation_event(
    persisted: &PersistedCancellation,
    assignment: &str,
    value: &str,
) {
    let mut connection = persisted.fixture.store.runtime.acquire().await.unwrap();
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE journal_events SET {assignment} = ? WHERE journal_offset = ?"
    )))
    .bind(value)
    .bind(persisted.receipt.committed_cursor.get())
    .execute(&mut *connection)
    .await
    .unwrap();
    assert_eq!(result.rows_affected(), 1);
}

async fn assert_consistency_fails_closed_and_redacted(
    persisted: &PersistedCancellation,
    corrupted_value: &str,
) {
    let error = persisted
        .fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), StateStoreErrorKind::InternalInvariant);
    for surface in [error.to_string(), format!("{error:?}")] {
        assert_eq!(surface, "state store internal invariant failure");
        for forbidden in [
            SENTINEL,
            corrupted_value,
            "journal_events",
            "client_commands",
            "SELECT",
            persisted.fixture.root.path().to_str().unwrap(),
        ] {
            assert!(!surface.contains(forbidden));
        }
    }
}

#[tokio::test]
async fn queued_and_active_transition_receipts_match_exact_cancellation_events() {
    for kind in [
        CancellationTransitionKind::Queued,
        CancellationTransitionKind::Active,
    ] {
        let persisted = persisted_cancellation(kind).await;
        persisted.fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn queued_cancellation_rejects_message_and_work_cursor_downgrades() {
    for earlier_event_type in ["message.accepted", "work.queued"] {
        let persisted = persisted_cancellation(CancellationTransitionKind::Queued).await;
        corrupt_cancellation_cursor(&persisted, earlier_event_type).await;
        assert_consistency_fails_closed_and_redacted(&persisted, earlier_event_type).await;
        persisted.fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn queued_cancellation_rejects_non_immediate_work_stream_causation() {
    let persisted = persisted_cancellation(CancellationTransitionKind::Queued).await;
    let message_event_id = work_event_value(
        &persisted.fixture,
        persisted.work_id,
        "message.accepted",
        "event_id",
    )
    .await;
    corrupt_cancellation_event(&persisted, "causation_event_id", &message_event_id).await;
    assert_consistency_fails_closed_and_redacted(&persisted, &message_event_id).await;
    persisted.fixture.guard.shutdown().await;
}

#[tokio::test]
async fn queued_cancellation_rejects_nonexistent_and_wrong_existing_device_actors() {
    for wrong_existing in [false, true] {
        let persisted = persisted_cancellation(CancellationTransitionKind::Queued).await;
        let actor_id = if wrong_existing {
            DeviceProvisioningService::new(&persisted.fixture.store)
                .provision_fixture_token(
                    DeviceDisplayName::try_new("Unrelated device".into()).unwrap(),
                    timestamp(T0),
                    BearerToken::parse("cd".repeat(32)).unwrap(),
                )
                .await
                .unwrap()
                .summary
                .device_id
        } else {
            DeviceId::generate()
        };
        corrupt_cancellation_event(&persisted, "actor_id", &actor_id.to_string()).await;
        assert_consistency_fails_closed_and_redacted(&persisted, &actor_id.to_string()).await;
        persisted.fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn active_cancellation_rejects_cursor_causation_and_actor_corruption() {
    for corruption in ["cursor", "causation", "actor"] {
        let persisted = persisted_cancellation(CancellationTransitionKind::Active).await;
        let corrupted_value = match corruption {
            "cursor" => {
                corrupt_cancellation_cursor(&persisted, "work.queued").await;
                "work.queued".to_owned()
            }
            "causation" => {
                let message_event_id = work_event_value(
                    &persisted.fixture,
                    persisted.work_id,
                    "message.accepted",
                    "event_id",
                )
                .await;
                corrupt_cancellation_event(&persisted, "causation_event_id", &message_event_id)
                    .await;
                message_event_id
            }
            "actor" => {
                let actor_id = DeviceId::generate().to_string();
                corrupt_cancellation_event(&persisted, "actor_id", &actor_id).await;
                actor_id
            }
            _ => unreachable!(),
        };
        assert_consistency_fails_closed_and_redacted(&persisted, &corrupted_value).await;
        persisted.fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_noop_receipts_keep_the_v3_high_water_compatibility_path() {
    for kind in [
        CancellationTransitionKind::Active,
        CancellationTransitionKind::Queued,
    ] {
        let persisted = persisted_cancellation(kind).await;
        accept(
            &persisted.fixture.store,
            persisted.fixture.device_id,
            persisted.fixture.identity,
            ClientMessageId::generate(),
            "advance unrelated journal head",
        )
        .await;
        let no_op = CommandService::new(&persisted.fixture.store)
            .cancel_work(
                AuthenticatedDevice::new(persisted.fixture.device_id),
                cancel_command(
                    ClientCommandId::generate(),
                    persisted.work_id,
                    timestamp(T3),
                ),
            )
            .await
            .unwrap()
            .into_receipt();
        assert_eq!(
            no_op.resulting_work_state,
            persisted.receipt.resulting_work_state
        );
        assert!(no_op.committed_cursor > persisted.receipt.committed_cursor);
        persisted
            .fixture
            .store
            .verify_application_consistency()
            .await
            .unwrap();
        persisted.fixture.guard.shutdown().await;
    }
}

#[tokio::test]
async fn active_cancellation_requests_cleanup_once_and_preserves_runtime_ownership() {
    let fixture = fixture().await;
    let work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "active",
    )
    .await
    .into_receipt()
    .work_id;
    let runtime_id = transition_to_running(&fixture, work).await;
    let first_id = ClientCommandId::generate();
    let first = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(first_id, work, timestamp(T2)),
        )
        .await
        .unwrap()
        .into_receipt();
    assert_eq!(first.resulting_work_state, WorkState::CancelRequested);
    assert_eq!(first.http_status(), 202);
    assert!(first.cleanup.is_pending());
    let replay = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(first_id, work, timestamp(T2)),
        )
        .await
        .unwrap();
    assert!(replay.is_replay());
    assert_eq!(replay.into_receipt(), first);
    let second = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            cancel_command(ClientCommandId::generate(), work, timestamp(T2)),
        )
        .await
        .unwrap()
        .into_receipt();
    assert_eq!(second.resulting_work_state, WorkState::CancelRequested);
    assert_eq!(second.committed_cursor, first.committed_cursor);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let (state, owner, reason, events): (String, Option<String>, Option<String>, i64) =
        sqlx::query_as(
            "SELECT state, runtime_instance_id, cancellation_reason_code, \
             (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancel_requested' \
              AND work_id = ?) FROM work_items WHERE work_id = ?",
        )
        .bind(work.to_string())
        .bind(work.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(state, "cancel_requested");
    assert_eq!(owner, Some(runtime_id.to_string()));
    assert_eq!(reason.as_deref(), Some("user_request"));
    assert_eq!(events, 1);
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cancellations_have_one_transition_and_stable_noop_receipts() {
    let fixture = fixture().await;
    let work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "race",
    )
    .await
    .into_receipt()
    .work_id;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let device = fixture.device_id;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            CommandService::new(&*store)
                .cancel_work(
                    AuthenticatedDevice::new(device),
                    cancel_command(ClientCommandId::generate(), work, timestamp(T2)),
                )
                .await
                .unwrap()
                .into_receipt()
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_eq!(first.resulting_work_state, WorkState::Cancelled);
    assert_eq!(second.resulting_work_state, WorkState::Cancelled);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancelled' AND work_id = ?",
    )
    .bind(work.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(event_count, 1);
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_cancellation_key_replays_and_changed_target_conflicts() {
    let fixture = fixture().await;
    let first_work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "same-key",
    )
    .await
    .into_receipt()
    .work_id;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(Barrier::new(3));
    let command_id = ClientCommandId::generate();
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let device = fixture.device_id;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            CommandService::new(&*store)
                .cancel_work(
                    AuthenticatedDevice::new(device),
                    cancel_command(command_id, first_work, timestamp(T2)),
                )
                .await
                .unwrap()
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_ne!(first.is_replay(), second.is_replay());
    assert_eq!(first.receipt(), second.receipt());

    let second_work = accept(
        &store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "changed-target-a",
    )
    .await
    .into_receipt()
    .work_id;
    let third_work = accept(
        &store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "changed-target-b",
    )
    .await
    .into_receipt()
    .work_id;
    let changed_key = ClientCommandId::generate();
    let barrier = Arc::new(Barrier::new(3));
    let mut changed_tasks = Vec::new();
    for work in [second_work, third_work] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let device = fixture.device_id;
        changed_tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            CommandService::new(&*store)
                .cancel_work(
                    AuthenticatedDevice::new(device),
                    cancel_command(changed_key, work, timestamp(T2)),
                )
                .await
        }));
    }
    barrier.wait().await;
    let outcomes = [
        changed_tasks.remove(0).await.unwrap(),
        changed_tasks.remove(0).await.unwrap(),
    ];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| {
                outcome.as_ref().is_err_and(|error| {
                    error.kind() == CommandServiceErrorKind::IdempotencyConflict
                })
            })
            .count(),
        1
    );
    fixture.guard.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_active_cancellation_keys_emit_one_request_event() {
    let fixture = fixture().await;
    let work = accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "active-race",
    )
    .await
    .into_receipt()
    .work_id;
    transition_to_running(&fixture, work).await;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let device = fixture.device_id;
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            CommandService::new(&*store)
                .cancel_work(
                    AuthenticatedDevice::new(device),
                    cancel_command(ClientCommandId::generate(), work, timestamp(T2)),
                )
                .await
                .unwrap()
                .into_receipt()
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_eq!(first.resulting_work_state, WorkState::CancelRequested);
    assert_eq!(second.resulting_work_state, WorkState::CancelRequested);
    assert_eq!(first.committed_cursor, second.committed_cursor);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancel_requested' \
         AND work_id = ?",
    )
    .bind(work.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(event_count, 1);
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn postcommit_message_and_cancellation_responses_replay_after_reopen() {
    let fixture = fixture().await;
    let root_path = fixture.root.path().to_path_buf();
    let identity = fixture.identity;
    let device_id = fixture.device_id;
    let token_text = fixture.token_text.clone();
    let message_id = ClientMessageId::generate();
    let message_receipt = accept(
        &fixture.store,
        device_id,
        identity,
        message_id,
        "lost response",
    )
    .await
    .into_receipt();
    fixture.guard.shutdown().await;

    let guard = SqliteRuntimeGuard::start(&root_path, 4).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let authenticated = DeviceAuthenticator::new(&store)
        .authenticate(
            BearerToken::parse(token_text.clone()).unwrap(),
            timestamp(T2),
        )
        .await
        .unwrap();
    let replay = CommandService::new(&store)
        .accept_message(
            authenticated,
            message_command(identity, message_id, "lost response", timestamp(T1)),
        )
        .await
        .unwrap();
    assert!(replay.is_replay());
    assert_eq!(replay.into_receipt(), message_receipt);
    let cancellation_id = ClientCommandId::generate();
    let cancellation_receipt = CommandService::new(&store)
        .cancel_work(
            authenticated,
            cancel_command(cancellation_id, message_receipt.work_id, timestamp(T2)),
        )
        .await
        .unwrap()
        .into_receipt();
    guard.shutdown().await;

    let guard = SqliteRuntimeGuard::start(&root_path, 4).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let cancellation_replay = CommandService::new(&store)
        .cancel_work(
            AuthenticatedDevice::new(device_id),
            cancel_command(cancellation_id, message_receipt.work_id, timestamp(T2)),
        )
        .await
        .unwrap();
    assert!(cancellation_replay.is_replay());
    assert_eq!(cancellation_replay.into_receipt(), cancellation_receipt);
    assert_eq!(counts(&store).await, (1, 1, 1, 5, 2));
    store.verify_application_consistency().await.unwrap();
    guard.shutdown().await;
    drop(fixture.root);
}

#[tokio::test]
async fn malformed_persisted_command_receipt_fails_closed_at_startup_consistency() {
    let fixture = fixture().await;
    accept(
        &fixture.store,
        fixture.device_id,
        fixture.identity,
        ClientMessageId::generate(),
        "corrupt later",
    )
    .await;
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    sqlx::query(
        "UPDATE client_commands SET response_json = json_set(response_json, '$.version', 2)",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert!(
        fixture
            .store
            .verify_application_consistency()
            .await
            .is_err()
    );
    fixture.guard.shutdown().await;
}
