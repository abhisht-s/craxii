use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sqlx::{ConnectOptions, Connection, Row};

use crate::domain::{
    ClientMessageId, ContentBlock, ConversationCreatedV1, ConversationId, ConversationWorkOrdinal,
    CorrelationId, CraxiiId, DeviceId, JournalActor, JournalEventId, JournalEventKind,
    JournalEventPayload, JournalStreamId, MessageCommittedV1, MessageContent, MessageId,
    MessageRole, ProjectionVersion, Sha256Digest, WorkId, WorkInputActor, WorkInputFactV1,
    WorkInputOrdinal, WorkInputRelationship, WorkKind, WorkQueuedV1, WorkspaceId,
    WorkstationGeneration, WorkstationId,
};
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, LoadOrBootstrapIdentityRequest, V0IdentityReference,
};

use super::codec::encode_message_content;
use super::journal::{
    JournalAppendIntent, allocate_stream_sequence, append_event, insert_work_input,
    load_stream_events, prepare_event,
};
use super::runtime::SqliteRuntimeGuard;
use super::schema::{
    JOURNAL_MIGRATION_DESCRIPTION, MIGRATOR, PRODUCT_INDEXES, PRODUCT_TABLES,
    expected_schema_fingerprint, v1_schema_fingerprint, v2_schema_fingerprint,
};
use super::state_store::{BootstrapTestHook, SqliteStateStore};
use super::transaction::WriteTransaction;

const AT: &str = "2026-08-28T01:02:03.456789Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage7-test-{}-{}",
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

fn observation() -> BootstrapObservation {
    BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(1).unwrap(),
        architecture: "test-architecture".into(),
        os_release: "test-os-release".into(),
        default_shell: "/bin/sh".into(),
        workspace_logical_name: "primary".into(),
        workspace_logical_root: "/workspace".into(),
        workspace_resolved_root: "/tmp/craxii-workspace".into(),
        execution_capabilities:
            crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
    }
}

fn request() -> LoadOrBootstrapIdentityRequest {
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
        created_at: AT.parse().unwrap(),
        observation: observation(),
    }
}

async fn count(runtime: &super::runtime::SqliteRuntime, table: &str) -> i64 {
    let mut connection = runtime.acquire().await.unwrap();
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

#[derive(Clone)]
struct RawJournalRow {
    event_id: String,
    craxii_id: String,
    stream_id: String,
    stream_seq: i64,
    event_type: String,
    event_version: i64,
    conversation_id: Option<String>,
    work_id: Option<String>,
    causation_event_id: Option<String>,
    correlation_id: String,
    actor_kind: String,
    actor_id: Option<String>,
    runtime_instance_id: Option<String>,
    payload_json: String,
    payload_sha256: String,
    recorded_at: String,
    occurred_at: Option<String>,
}

impl RawJournalRow {
    fn valid(craxii_id: CraxiiId) -> Self {
        let payload_json = "{}".to_owned();
        Self {
            event_id: JournalEventId::generate().to_string(),
            craxii_id: craxii_id.to_string(),
            stream_id: JournalStreamId::Work(WorkId::generate()).to_string(),
            stream_seq: 1,
            event_type: "test.event".to_owned(),
            event_version: 1,
            conversation_id: None,
            work_id: None,
            causation_event_id: None,
            correlation_id: CorrelationId::generate().to_string(),
            actor_kind: "craxii".to_owned(),
            actor_id: Some(craxii_id.to_string()),
            runtime_instance_id: None,
            payload_sha256: Sha256Digest::hash_bytes(payload_json.as_bytes()).to_string(),
            payload_json,
            recorded_at: AT.to_owned(),
            occurred_at: None,
        }
    }
}

async fn raw_insert(
    connection: &mut sqlx::SqliteConnection,
    row: &RawJournalRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO journal_events (event_id, craxii_id, stream_id, stream_seq, event_type, \
         event_version, conversation_id, work_id, causation_event_id, correlation_id, actor_kind, \
         actor_id, runtime_instance_id, payload_json, payload_sha256, recorded_at, occurred_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.event_id)
    .bind(&row.craxii_id)
    .bind(&row.stream_id)
    .bind(row.stream_seq)
    .bind(&row.event_type)
    .bind(row.event_version)
    .bind(&row.conversation_id)
    .bind(&row.work_id)
    .bind(&row.causation_event_id)
    .bind(&row.correlation_id)
    .bind(&row.actor_kind)
    .bind(&row.actor_id)
    .bind(&row.runtime_instance_id)
    .bind(&row.payload_json)
    .bind(&row.payload_sha256)
    .bind(&row.recorded_at)
    .bind(&row.occurred_at)
    .execute(connection)
    .await
    .map(|_| ())
}

fn conversation_event_intent(
    snapshot: &crate::ports::state_store::BootstrapSnapshot,
    cause: JournalEventId,
) -> JournalAppendIntent {
    let conversation = &snapshot.primary_conversation;
    JournalAppendIntent {
        event_id: JournalEventId::generate(),
        craxii_id: snapshot.identity.craxii_id,
        stream_id: JournalStreamId::Conversation(snapshot.identity.conversation_id),
        conversation_id: Some(snapshot.identity.conversation_id),
        work_id: None,
        causation_event_id: Some(cause),
        correlation_id: CorrelationId::generate(),
        actor: JournalActor::Craxii(snapshot.identity.craxii_id),
        runtime_instance_id: None,
        payload: JournalEventPayload::ConversationCreated(ConversationCreatedV1 {
            conversation_id: conversation.conversation_id(),
            craxii_id: conversation.craxii_id(),
            kind: conversation.kind(),
            lifecycle: conversation.lifecycle(),
            next_work_ordinal: conversation.next_work_ordinal(),
            state_version: conversation.projection_version(),
            created_at: conversation.created_at(),
        }),
        recorded_at: AT.parse().unwrap(),
        occurred_at: None,
    }
}

#[tokio::test]
async fn migration_three_manifest_schema_and_zero_product_rows_are_exact() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(PRODUCT_TABLES.len(), 17);
    assert_eq!(PRODUCT_INDEXES.len(), 40);
    for table in PRODUCT_TABLES {
        assert_eq!(count(guard.runtime(), table).await, 0, "{table}");
    }
    let migration = sqlx::query(
        "SELECT description, checksum, success FROM _sqlx_migrations WHERE version = 2",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        migration.get::<String, _>("description"),
        JOURNAL_MIGRATION_DESCRIPTION
    );
    assert_eq!(migration.get::<i64, _>("success"), 1);
    let embedded = MIGRATOR.iter().find(|item| item.version == 2).unwrap();
    assert_eq!(
        migration.get::<Vec<u8>, _>("checksum").as_slice(),
        embedded.checksum.as_ref()
    );
    assert_eq!(
        migration
            .get::<Vec<u8>, _>("checksum")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        "677379cfb19c61d45c6a61bdeb978539490adcee97f57e51cab8794e63038b70950d715a90e7e524397007a97f875ebf"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger'")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_schema WHERE type = 'view'")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        expected_schema_fingerprint(),
        "73ab94c2ec36ef1b09addc475aa6bcf806336612f58fd551fd4648c5a124f5a3"
    );
    assert_eq!(
        v1_schema_fingerprint(),
        "f4636df22c635c90ac469f49f2ac3a9ccb38956f1670d26ab566140a137f5521"
    );
    assert_eq!(
        v2_schema_fingerprint(),
        "391d9bfb54cf771de1815a3bf54ee4d7d16f1b877acf629cf783ca12dbd37d4d"
    );
    let partial_indexes = sqlx::query(
        "SELECT name, sql FROM sqlite_schema WHERE name IN \
         ('ix_journal_events_conversation_offset', 'ix_journal_events_work_offset') \
         ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(partial_indexes.len(), 2);
    assert!(
        partial_indexes[0]
            .get::<String, _>("sql")
            .contains("WHERE conversation_id IS NOT NULL")
    );
    assert!(
        partial_indexes[1]
            .get::<String, _>("sql")
            .contains("WHERE work_id IS NOT NULL")
    );
    let journal_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'journal_events'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(journal_sql.contains("INTEGER PRIMARY KEY AUTOINCREMENT"));
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn journal_scalar_json_foreign_key_and_uniqueness_constraints_fail_closed() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let bootstrap = store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let valid = RawJournalRow::valid(bootstrap.identity.craxii_id);
    raw_insert(&mut connection, &valid).await.unwrap();

    let invalid_rows = [
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.event_id = "not-a-uuid".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.craxii_id = CraxiiId::generate().to_string();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.stream_id = "Work:not-a-stream".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.stream_seq = 0;
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.event_type = "invalid".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.event_version = 0;
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.conversation_id = Some(ConversationId::generate().to_string());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.work_id = Some(WorkId::generate().to_string());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.causation_event_id = Some(JournalEventId::generate().to_string());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.correlation_id = "not-a-uuid".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.actor_kind = "system".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.actor_id = Some("not-a-uuid".to_owned());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.runtime_instance_id =
                Some(crate::domain::RuntimeInstanceId::generate().to_string());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.payload_json = "not-json".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.payload_json = "[]".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.payload_json = format!("{{\"value\":\"{}\"}}", "x".repeat(262_144));
            row.payload_sha256 = Sha256Digest::hash_bytes(row.payload_json.as_bytes()).to_string();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.payload_sha256 = "A".repeat(64);
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.recorded_at = "2026-08-28T01:02:03Z".to_owned();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.occurred_at = Some("2026-08-28T01:02:03Z".to_owned());
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.event_id = valid.event_id.clone();
            row
        },
        {
            let mut row = RawJournalRow::valid(bootstrap.identity.craxii_id);
            row.stream_id = valid.stream_id.clone();
            row.stream_seq = valid.stream_seq;
            row
        },
    ];
    for row in invalid_rows {
        assert!(raw_insert(&mut connection, &row).await.is_err());
    }
    let zero_offset = RawJournalRow::valid(bootstrap.identity.craxii_id);
    assert!(
        sqlx::query(
            "INSERT INTO journal_events (journal_offset, event_id, craxii_id, stream_id, \
             stream_seq, event_type, event_version, correlation_id, actor_kind, actor_id, \
             payload_json, payload_sha256, recorded_at) VALUES (0, ?, ?, ?, 1, 'test.event', 1, \
             ?, 'craxii', ?, '{}', ?, ?)",
        )
        .bind(&zero_offset.event_id)
        .bind(&zero_offset.craxii_id)
        .bind(&zero_offset.stream_id)
        .bind(&zero_offset.correlation_id)
        .bind(&zero_offset.actor_id)
        .bind(Sha256Digest::hash_bytes(b"{}").to_string())
        .bind(AT)
        .execute(&mut *connection)
        .await
        .is_err()
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn work_input_constraints_and_private_causal_validation_fail_closed() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let work_id = WorkId::generate();
    let correlation_id = CorrelationId::generate();
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO work_items (work_id, craxii_id, conversation_id, \
         conversation_work_ordinal, kind, state, state_version, priority, workspace_id, \
         runtime_instance_id, current_model_invocation_id, current_tool_execution_id, \
         correlation_id, created_at, queued_at, started_at, cancel_requested_at, \
         cancellation_reason_code, terminal_at, terminal_reason_code, terminal_detail_json) \
         VALUES (?, ?, ?, 1, 'conversational', 'queued', 1, 0, ?, NULL, NULL, NULL, ?, ?, ?, \
         NULL, NULL, NULL, NULL, NULL, NULL)",
    )
    .bind(work_id.to_string())
    .bind(snapshot.identity.craxii_id.to_string())
    .bind(snapshot.identity.conversation_id.to_string())
    .bind(snapshot.identity.workspace_id.to_string())
    .bind(correlation_id.to_string())
    .bind(AT)
    .bind(AT)
    .execute(&mut *connection)
    .await
    .unwrap();
    let mut message = RawJournalRow::valid(snapshot.identity.craxii_id);
    message.stream_id =
        JournalStreamId::Conversation(snapshot.identity.conversation_id).to_string();
    message.stream_seq = 2;
    message.event_type = JournalEventKind::MessageAccepted.as_str().to_owned();
    message.conversation_id = Some(snapshot.identity.conversation_id.to_string());
    message.correlation_id = correlation_id.to_string();
    raw_insert(&mut connection, &message).await.unwrap();
    sqlx::query(
        "INSERT INTO work_item_inputs (work_id, input_event_id, relationship, \
         ordinal_within_work, attached_at, attached_by_actor) \
         VALUES (?, ?, 'trigger', 1, ?, 'user')",
    )
    .bind(work_id.to_string())
    .bind(&message.event_id)
    .bind(AT)
    .execute(&mut *connection)
    .await
    .unwrap();

    assert!(
        sqlx::query("INSERT INTO work_item_inputs VALUES (?, ?, 'trigger', 1, ?, 'user')",)
            .bind(work_id.to_string())
            .bind(&message.event_id)
            .bind(AT)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    for (relationship, actor, ordinal) in [
        ("unknown", "user", 2_i64),
        ("supplemental", "unknown", 3),
        ("supplemental", "system", 0),
    ] {
        let mut event = RawJournalRow::valid(snapshot.identity.craxii_id);
        event.stream_seq = ordinal.max(2) + 20;
        raw_insert(&mut connection, &event).await.unwrap();
        assert!(
            sqlx::query("INSERT INTO work_item_inputs VALUES (?, ?, ?, ?, ?, ?)",)
                .bind(work_id.to_string())
                .bind(&event.event_id)
                .bind(relationship)
                .bind(ordinal)
                .bind(AT)
                .bind(actor)
                .execute(&mut *connection)
                .await
                .is_err()
        );
    }
    let mut second_message = RawJournalRow::valid(snapshot.identity.craxii_id);
    second_message.stream_id =
        JournalStreamId::Conversation(snapshot.identity.conversation_id).to_string();
    second_message.stream_seq = 3;
    second_message.event_type = JournalEventKind::MessageAccepted.as_str().to_owned();
    second_message.conversation_id = Some(snapshot.identity.conversation_id.to_string());
    second_message.correlation_id = correlation_id.to_string();
    raw_insert(&mut connection, &second_message).await.unwrap();
    assert!(
        sqlx::query("INSERT INTO work_item_inputs VALUES (?, ?, 'supplemental', 1, ?, 'system')",)
            .bind(work_id.to_string())
            .bind(&second_message.event_id)
            .bind(AT)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO work_item_inputs VALUES (?, ?, 'supplemental', 4, ?, 'system')",)
            .bind(WorkId::generate().to_string())
            .bind(&second_message.event_id)
            .bind(AT)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO work_item_inputs VALUES (?, ?, 'supplemental', 4, ?, 'system')",)
            .bind(work_id.to_string())
            .bind(JournalEventId::generate().to_string())
            .bind(AT)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    drop(connection);

    let unrelated = RawJournalRow::valid(snapshot.identity.craxii_id);
    let unrelated_event_id = JournalEventId::parse_canonical(&unrelated.event_id).unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    raw_insert(&mut connection, &unrelated).await.unwrap();
    drop(connection);
    let mut transaction = WriteTransaction::begin(guard.runtime(), "test_causal_input_validation")
        .await
        .unwrap();
    let input = WorkInputFactV1 {
        input_event_id: unrelated_event_id,
        relationship: WorkInputRelationship::Trigger,
        ordinal_within_work: WorkInputOrdinal::try_new(1).unwrap(),
        attached_at: AT.parse().unwrap(),
        actor: WorkInputActor::User,
    };
    assert!(
        insert_work_input(&mut transaction, work_id, &input)
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    guard.shutdown().await;
}

#[tokio::test]
async fn valid_version_one_database_migrates_to_version_three_and_reopens() {
    let root = TestRoot::new();
    let database_directory = root.path().join("db");
    fs::create_dir(&database_directory).unwrap();
    fs::set_permissions(&database_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let database = database_directory.join("craxii.sqlite3");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true)
        .foreign_keys(true);
    let mut connection = options.connect().await.unwrap();
    let version_one = sqlx::migrate::Migrator::with_migrations(vec![
        MIGRATOR
            .iter()
            .find(|item| item.version == 1)
            .unwrap()
            .clone(),
    ]);
    version_one.run(&mut connection).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        1
    );
    connection.close().await.unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();

    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    assert_eq!(count(guard.runtime(), "journal_events").await, 0);
    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        3
    );
    drop(connection);
    guard.shutdown().await;

    let reopened = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    reopened.shutdown().await;
}

#[tokio::test]
async fn stream_allocator_serializes_concurrent_appends_and_is_independent_per_stream() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let cause = JournalEventId::parse_canonical(
        &sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM journal_events WHERE event_type = 'craxii.initialized'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
    )
    .unwrap();
    drop(connection);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let runtime = guard.runtime().clone();
        let intent = conversation_event_intent(&snapshot, cause);
        tasks.push(tokio::spawn(async move {
            let mut transaction = WriteTransaction::begin(&runtime, "test_concurrent_append")
                .await
                .unwrap();
            let position = append_event(&mut transaction, prepare_event(intent).unwrap())
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            position
        }));
    }
    let mut positions = Vec::new();
    for task in tasks {
        positions.push(task.await.unwrap());
    }
    positions.sort_by_key(|position| position.offset);
    assert_eq!(
        positions
            .iter()
            .map(|position| position.stream_seq.get())
            .collect::<Vec<_>>(),
        (2..=9).collect::<Vec<_>>()
    );
    assert!(
        positions
            .windows(2)
            .all(|pair| pair[0].offset < pair[1].offset)
    );
    assert_eq!(
        load_stream_events(
            guard.runtime(),
            JournalStreamId::Conversation(snapshot.identity.conversation_id)
        )
        .await
        .unwrap()
        .len(),
        9
    );

    let first_stream = JournalStreamId::Work(WorkId::generate());
    let second_stream = JournalStreamId::Work(WorkId::generate());
    let runtime_a = guard.runtime().clone();
    let runtime_b = guard.runtime().clone();
    let first = tokio::spawn(async move {
        let mut transaction = WriteTransaction::begin(&runtime_a, "test_independent_stream_a")
            .await
            .unwrap();
        let one = allocate_stream_sequence(&mut transaction, first_stream)
            .await
            .unwrap();
        let two = allocate_stream_sequence(&mut transaction, first_stream)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        (one, two)
    });
    let second = tokio::spawn(async move {
        let mut transaction = WriteTransaction::begin(&runtime_b, "test_independent_stream_b")
            .await
            .unwrap();
        let one = allocate_stream_sequence(&mut transaction, second_stream)
            .await
            .unwrap();
        let two = allocate_stream_sequence(&mut transaction, second_stream)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        (one, two)
    });
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!((first.0.get(), first.1.get()), (1, 2));
    assert_eq!((second.0.get(), second.1.get()), (1, 2));

    let overflow_stream = JournalStreamId::Work(WorkId::generate());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("INSERT INTO stream_heads (stream_id, last_stream_seq) VALUES (?, ?)")
        .bind(overflow_stream.to_string())
        .bind(i64::MAX)
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    let mut transaction = WriteTransaction::begin(guard.runtime(), "test_stream_overflow")
        .await
        .unwrap();
    assert!(
        allocate_stream_sequence(&mut transaction, overflow_stream)
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    guard.shutdown().await;
}

#[tokio::test]
async fn multi_event_transaction_assigns_insertion_order_and_rollback_restores_heads() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let cause = JournalEventId::parse_canonical(
        &sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM journal_events WHERE event_type = 'craxii.initialized'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
    )
    .unwrap();
    drop(connection);

    let mut transaction = WriteTransaction::begin(guard.runtime(), "test_multi_event_rollback")
        .await
        .unwrap();
    let first = append_event(
        &mut transaction,
        prepare_event(conversation_event_intent(&snapshot, cause)).unwrap(),
    )
    .await
    .unwrap();
    let second = append_event(
        &mut transaction,
        prepare_event(conversation_event_intent(&snapshot, cause)).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!((first.offset.get(), second.offset.get()), (3, 4));
    assert_eq!((first.stream_seq.get(), second.stream_seq.get()), (2, 3));
    transaction.rollback().await.unwrap();
    assert_eq!(count(guard.runtime(), "journal_events").await, 2);
    let mut connection = guard.runtime().acquire().await.unwrap();
    let head: i64 =
        sqlx::query_scalar("SELECT last_stream_seq FROM stream_heads WHERE stream_id = ?")
            .bind(JournalStreamId::Conversation(snapshot.identity.conversation_id).to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(head, 1);
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn first_bootstrap_and_reopen_are_exact_atomic_and_idempotent() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let first = store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(first.commit.events.unwrap().first.get(), 1);
    assert_eq!(first.commit.events.unwrap().last.get(), 2);
    let stable = first.identity;
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    assert_eq!(snapshot.identity, stable);
    assert_eq!(snapshot.journal_head.get(), 2);
    assert_eq!(snapshot.primary_conversation.next_work_ordinal().get(), 1);
    assert_eq!(snapshot.primary_conversation.projection_version().get(), 1);
    for (table, expected) in [
        ("craxii_principals", 1),
        ("workstations", 1),
        ("workspaces", 1),
        ("conversations", 1),
        ("runtime_instances", 0),
        ("client_devices", 0),
        ("work_items", 0),
        ("messages", 0),
        ("client_commands", 0),
        ("work_item_inputs", 0),
        ("journal_events", 2),
        ("stream_heads", 2),
    ] {
        assert_eq!(count(guard.runtime(), table).await, expected, "{table}");
    }
    let mut connection = guard.runtime().acquire().await.unwrap();
    let rows = sqlx::query(
        "SELECT event_type, stream_seq, correlation_id, causation_event_id \
         FROM journal_events ORDER BY journal_offset",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(rows[0].get::<String, _>("event_type"), "craxii.initialized");
    assert_eq!(
        rows[1].get::<String, _>("event_type"),
        "conversation.created"
    );
    assert_eq!(rows[0].get::<i64, _>("stream_seq"), 1);
    assert_eq!(rows[1].get::<i64, _>("stream_seq"), 1);
    assert_eq!(
        rows[0].get::<String, _>("correlation_id"),
        rows[1].get::<String, _>("correlation_id")
    );
    let first_event_id: String = sqlx::query_scalar(
        "SELECT event_id FROM journal_events WHERE event_type = 'craxii.initialized'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        rows[1].get::<Option<String>, _>("causation_event_id"),
        Some(first_event_id)
    );
    drop(connection);

    let second = store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    assert!(!second.created);
    assert_eq!(second.identity, stable);
    assert!(second.commit.events.is_none());
    assert_eq!(count(guard.runtime(), "journal_events").await, 2);
    assert_eq!(count(guard.runtime(), "stream_heads").await, 2);
    guard.shutdown().await;
}

#[tokio::test]
async fn stage13_refreshes_current_capabilities_root_and_last_seen_without_rewriting_initial_event()
{
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    let legacy_capabilities: String = sqlx::query_scalar(
        "SELECT json_set(capabilities_json, \
         '$.flags.filesystem_read', json('false'), \
         '$.limits.max_execution_timeout_ms', 900000, \
         '$.limits.max_stdout_bytes', 8388608, \
         '$.limits.max_stderr_bytes', 8388608) FROM workstations",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let initial_payload: String = sqlx::query_scalar(
        "SELECT json_set(payload_json, '$.capabilities_sha256', ?) \
         FROM journal_events WHERE event_type = 'craxii.initialized'",
    )
    .bind(Sha256Digest::hash_bytes(legacy_capabilities.as_bytes()).to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let initial_payload_sha256 = Sha256Digest::hash_bytes(initial_payload.as_bytes()).to_string();
    sqlx::query("UPDATE workstations SET capabilities_json = ?")
        .bind(&legacy_capabilities)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? \
         WHERE event_type = 'craxii.initialized'",
    )
    .bind(&initial_payload)
    .bind(&initial_payload_sha256)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    let mut refresh = request();
    refresh.created_at = "2026-08-29T02:03:04.567890Z".parse().unwrap();
    refresh.observation.workspace_resolved_root = "/tmp/craxii-workspace-relocated".into();
    refresh.observation.execution_capabilities =
        crate::ports::state_store::ExecutionCapabilityObservation {
            foreground_execute: true,
            privilege_administrative: false,
            process_group_cleanup: true,
            cgroup_cleanup: false,
        };
    let receipt = store.load_or_bootstrap_v0_identity(refresh).await.unwrap();
    assert!(!receipt.created);
    assert!(receipt.commit.events.is_none());
    assert_eq!(count(guard.runtime(), "journal_events").await, 2);

    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let flags = snapshot.workstation_capabilities.flags();
    assert!(flags.filesystem_read());
    assert!(flags.privilege_user());
    assert!(flags.foreground_execute());
    assert!(flags.cancel_execution());
    assert!(flags.inspect_execution());
    assert!(!flags.privilege_administrative());
    assert!(flags.process_group_cleanup());
    assert!(!flags.cgroup_cleanup());
    assert_eq!(
        snapshot
            .workstation_capabilities
            .limits()
            .max_execution_timeout_ms(),
        900_000
    );
    assert_eq!(
        snapshot
            .workstation_capabilities
            .limits()
            .max_stdout_bytes(),
        8_388_608
    );
    assert_eq!(
        snapshot
            .workstation_capabilities
            .limits()
            .max_stderr_bytes(),
        8_388_608
    );

    let mut connection = guard.runtime().acquire().await.unwrap();
    let persisted_payload: (String, String) = sqlx::query_as(
        "SELECT payload_json, payload_sha256 FROM journal_events \
         WHERE event_type = 'craxii.initialized'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(persisted_payload.0, initial_payload);
    assert_eq!(persisted_payload.1, initial_payload_sha256);
    let (last_seen_at, resolved_root): (String, String) = sqlx::query_as(
        "SELECT w.last_seen_at, s.local_resolved_root FROM workstations w CROSS JOIN workspaces s",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(last_seen_at, "2026-08-29T02:03:04.567890Z");
    assert_eq!(resolved_root, "/tmp/craxii-workspace-relocated");
    drop(connection);
    assert!(store.verify_application_consistency().await.is_ok());
    guard.shutdown().await;
}

#[tokio::test]
async fn root_identity_survives_runtime_restart_and_state_root_relocation() {
    let mut root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let stable = store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap()
        .identity;
    let mut connection = guard.runtime().acquire().await.unwrap();
    let payload: String = sqlx::query_scalar(
        "SELECT payload_json FROM journal_events WHERE event_type = 'craxii.initialized'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(!payload.contains(&std::process::id().to_string()));
    assert!(!payload.contains("hostname"));
    assert!(!payload.contains(root.path().to_str().unwrap()));
    drop(connection);
    guard.shutdown().await;

    let relocated = root.0.with_extension("relocated");
    fs::rename(&root.0, &relocated).unwrap();
    root.0 = relocated;
    let reopened = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let store = SqliteStateStore::new(reopened.runtime().clone());
    let receipt = store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    assert!(!receipt.created);
    assert_eq!(receipt.identity, stable);
    assert_eq!(count(reopened.runtime(), "journal_events").await, 2);
    reopened.shutdown().await;
}

#[tokio::test]
async fn bootstrap_test_hooks_roll_back_roots_events_and_heads() {
    for hook in [
        BootstrapTestHook::BeforeFirstInsert,
        BootstrapTestHook::AfterRootRows,
        BootstrapTestHook::AfterFirstEvent,
        BootstrapTestHook::AfterSecondEvent,
    ] {
        let root = TestRoot::new();
        let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
        let store = SqliteStateStore::new(guard.runtime().clone());
        store.set_bootstrap_test_hook(Some(hook));
        assert!(
            store
                .load_or_bootstrap_v0_identity(request())
                .await
                .is_err()
        );
        for table in PRODUCT_TABLES {
            assert_eq!(count(guard.runtime(), table).await, 0, "{hook:?} {table}");
        }
        guard.shutdown().await;
        let reopened = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
        for table in PRODUCT_TABLES {
            assert_eq!(
                count(reopened.runtime(), table).await,
                0,
                "{hook:?} {table}"
            );
        }
        reopened.shutdown().await;
    }
}

#[tokio::test]
async fn partial_bootstrap_and_observation_contradictions_fail_without_repair() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE workstations SET os_release = 'contradiction'")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(
        store
            .load_or_bootstrap_v0_identity(request())
            .await
            .is_err()
    );
    assert_eq!(count(guard.runtime(), "journal_events").await, 2);
    guard.shutdown().await;
}

#[tokio::test]
async fn persisted_message_work_and_input_projection_comparison_is_exact() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    store
        .load_or_bootstrap_v0_identity(request())
        .await
        .unwrap();
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    let device_id = DeviceId::generate();
    let client_message_id =
        ClientMessageId::parse_canonical("01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d").unwrap();
    let message_id = MessageId::generate();
    let message_event_id = JournalEventId::generate();
    let work_event_id = JournalEventId::generate();
    let work_id = WorkId::generate();
    let correlation_id = CorrelationId::generate();
    let content =
        MessageContent::try_new(vec![ContentBlock::text("future fixture").unwrap()]).unwrap();
    let (content_json, content_sha256) = encode_message_content(&content).unwrap();
    let message_payload = MessageCommittedV1 {
        message_id,
        craxii_id: snapshot.identity.craxii_id,
        conversation_id: snapshot.identity.conversation_id,
        role: MessageRole::User,
        content: content.clone(),
        content_sha256,
        produced_by_work_id: None,
        device_id: Some(device_id),
        client_message_id: Some(client_message_id),
        committed_at: AT.parse().unwrap(),
    };
    let input = WorkInputFactV1 {
        input_event_id: message_event_id,
        relationship: WorkInputRelationship::Trigger,
        ordinal_within_work: WorkInputOrdinal::try_new(1).unwrap(),
        attached_at: AT.parse().unwrap(),
        actor: WorkInputActor::User,
    };
    let work_payload = WorkQueuedV1 {
        work_id,
        craxii_id: snapshot.identity.craxii_id,
        conversation_id: snapshot.identity.conversation_id,
        conversation_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
        kind: WorkKind::Conversational,
        priority: 0,
        workspace_id: snapshot.identity.workspace_id,
        correlation_id,
        state_version: ProjectionVersion::try_new(1).unwrap(),
        created_at: AT.parse().unwrap(),
        queued_at: AT.parse().unwrap(),
        trigger: input.clone(),
    };
    let mut connection = guard.runtime().acquire().await.unwrap();
    let conversation_created_id = JournalEventId::parse_canonical(
        &sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM journal_events WHERE event_type = 'conversation.created'",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
    )
    .unwrap();
    sqlx::query(
        "INSERT INTO client_devices \
         (device_id, display_name, token_hash, created_at, last_seen_at, revoked_at) \
         VALUES (?, 'fixture', ?, ?, NULL, NULL)",
    )
    .bind(device_id.to_string())
    .bind(Sha256Digest::hash_bytes(b"fixture-token").to_string())
    .bind(AT)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query("UPDATE sqlite_sequence SET seq = 10 WHERE name = 'journal_events'")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);

    let mut transaction = WriteTransaction::begin(guard.runtime(), "test_future_projection")
        .await
        .unwrap();
    let message_position = append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: message_event_id,
            craxii_id: snapshot.identity.craxii_id,
            stream_id: JournalStreamId::Conversation(snapshot.identity.conversation_id),
            conversation_id: Some(snapshot.identity.conversation_id),
            work_id: None,
            causation_event_id: Some(conversation_created_id),
            correlation_id,
            actor: JournalActor::User(Some(device_id)),
            runtime_instance_id: None,
            payload: JournalEventPayload::MessageAccepted(message_payload),
            recorded_at: AT.parse().unwrap(),
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
    .bind(snapshot.identity.craxii_id.to_string())
    .bind(snapshot.identity.conversation_id.to_string())
    .bind(content_json)
    .bind(content_sha256.to_string())
    .bind(device_id.to_string())
    .bind(client_message_id.to_string())
    .bind(AT)
    .execute(transaction.connection())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_items (work_id, craxii_id, conversation_id, \
         conversation_work_ordinal, kind, state, state_version, priority, workspace_id, \
         runtime_instance_id, current_model_invocation_id, current_tool_execution_id, \
         correlation_id, created_at, queued_at, started_at, cancel_requested_at, \
         cancellation_reason_code, terminal_at, terminal_reason_code, terminal_detail_json) \
         VALUES (?, ?, ?, 1, 'conversational', 'queued', 1, 0, ?, NULL, NULL, NULL, ?, ?, ?, \
         NULL, NULL, NULL, NULL, NULL, NULL)",
    )
    .bind(work_id.to_string())
    .bind(snapshot.identity.craxii_id.to_string())
    .bind(snapshot.identity.conversation_id.to_string())
    .bind(snapshot.identity.workspace_id.to_string())
    .bind(correlation_id.to_string())
    .bind(AT)
    .bind(AT)
    .execute(transaction.connection())
    .await
    .unwrap();
    let work_position = append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: work_event_id,
            craxii_id: snapshot.identity.craxii_id,
            stream_id: JournalStreamId::Work(work_id),
            conversation_id: Some(snapshot.identity.conversation_id),
            work_id: Some(work_id),
            causation_event_id: Some(message_event_id),
            correlation_id,
            actor: JournalActor::Craxii(snapshot.identity.craxii_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::WorkQueued(work_payload),
            recorded_at: AT.parse().unwrap(),
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    insert_work_input(&mut transaction, work_id, &input)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE conversations SET next_work_ordinal = 2, state_version = 2 \
         WHERE conversation_id = ? AND next_work_ordinal = 1 AND state_version = 1",
    )
    .bind(snapshot.identity.conversation_id.to_string())
    .execute(transaction.connection())
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(message_position.offset.get(), 11);
    assert_eq!(work_position.offset.get(), 12);
    assert_eq!(message_position.stream_seq.get(), 2);
    assert_eq!(work_position.stream_seq.get(), 1);
    assert_eq!(
        store
            .verify_application_consistency()
            .await
            .unwrap()
            .journal_head
            .unwrap()
            .get(),
        12
    );

    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE conversations SET state_version = 1")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE conversations SET state_version = 2")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE work_items SET state_version = 2")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE work_items SET state_version = 1")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET content_sha256 = ?")
        .bind("0".repeat(64))
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE messages SET content_sha256 = ?")
        .bind(content_sha256.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE stream_heads SET last_stream_seq = 2 WHERE stream_id LIKE 'work:%'")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE stream_heads SET last_stream_seq = 1 WHERE stream_id LIKE 'work:%'")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE journal_events SET event_version = 2 WHERE event_id = ?")
        .bind(work_event_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("UPDATE journal_events SET event_version = 1 WHERE event_id = ?")
        .bind(work_event_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    let original_payload: (String, String) = sqlx::query_as(
        "SELECT payload_json, payload_sha256 FROM journal_events WHERE event_id = ?",
    )
    .bind(work_event_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE journal_events SET payload_json = '{}', payload_sha256 = ? WHERE event_id = ?",
    )
    .bind(Sha256Digest::hash_bytes(b"{}").to_string())
    .bind(work_event_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? WHERE event_id = ?",
    )
    .bind(original_payload.0)
    .bind(original_payload.1)
    .bind(work_event_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM journal_events WHERE event_id = ?")
        .bind(message_event_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.verify_application_consistency().await.is_err());
    assert_eq!(count(guard.runtime(), "journal_events").await, 3);
    guard.shutdown().await;
}

#[derive(Clone, Copy, Debug)]
enum BootstrapCorruption {
    MissingPrincipal,
    MissingWorkstation,
    MissingWorkspace,
    MissingConversation,
    MissingBootstrapEvent,
    WrongEventType,
    WrongStream,
    WrongStreamSequence,
    WrongCorrelation,
    WrongCausation,
    MissingHead,
    WrongHead,
    ExtraHead,
    NullRootLinks,
    ExtraPrincipal,
    ExtraWorkstation,
    ExtraWorkspace,
    ExtraPrimaryConversation,
    SchemaRevision,
    InitializationPayload,
}

#[tokio::test]
async fn partial_bootstrap_matrix_fails_closed_and_never_repairs() {
    for corruption in [
        BootstrapCorruption::MissingPrincipal,
        BootstrapCorruption::MissingWorkstation,
        BootstrapCorruption::MissingWorkspace,
        BootstrapCorruption::MissingConversation,
        BootstrapCorruption::MissingBootstrapEvent,
        BootstrapCorruption::WrongEventType,
        BootstrapCorruption::WrongStream,
        BootstrapCorruption::WrongStreamSequence,
        BootstrapCorruption::WrongCorrelation,
        BootstrapCorruption::WrongCausation,
        BootstrapCorruption::MissingHead,
        BootstrapCorruption::WrongHead,
        BootstrapCorruption::ExtraHead,
        BootstrapCorruption::NullRootLinks,
        BootstrapCorruption::ExtraPrincipal,
        BootstrapCorruption::ExtraWorkstation,
        BootstrapCorruption::ExtraWorkspace,
        BootstrapCorruption::ExtraPrimaryConversation,
        BootstrapCorruption::SchemaRevision,
        BootstrapCorruption::InitializationPayload,
    ] {
        let root = TestRoot::new();
        let guard = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap();
        let store = SqliteStateStore::new(guard.runtime().clone());
        store
            .load_or_bootstrap_v0_identity(request())
            .await
            .unwrap();
        let mut connection = guard.runtime().acquire().await.unwrap();
        match corruption {
            BootstrapCorruption::MissingPrincipal
            | BootstrapCorruption::MissingWorkstation
            | BootstrapCorruption::MissingWorkspace
            | BootstrapCorruption::MissingConversation => {
                let table = match corruption {
                    BootstrapCorruption::MissingPrincipal => "craxii_principals",
                    BootstrapCorruption::MissingWorkstation => "workstations",
                    BootstrapCorruption::MissingWorkspace => "workspaces",
                    BootstrapCorruption::MissingConversation => "conversations",
                    _ => unreachable!(),
                };
                sqlx::query("PRAGMA foreign_keys = OFF")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table}")))
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
            }
            BootstrapCorruption::MissingBootstrapEvent => {
                sqlx::query("PRAGMA foreign_keys = OFF")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                sqlx::query("DELETE FROM journal_events WHERE event_type = 'conversation.created'")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
            }
            BootstrapCorruption::WrongEventType => {
                sqlx::query(
                    "UPDATE journal_events SET event_type = 'conversation.invalid' \
                     WHERE event_type = 'conversation.created'",
                )
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::WrongStream => {
                sqlx::query(
                    "UPDATE journal_events SET stream_id = ? \
                     WHERE event_type = 'conversation.created'",
                )
                .bind(JournalStreamId::Conversation(ConversationId::generate()).to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::WrongStreamSequence => {
                sqlx::query(
                    "UPDATE journal_events SET stream_seq = 2 \
                     WHERE event_type = 'conversation.created'",
                )
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::WrongCorrelation => {
                sqlx::query(
                    "UPDATE journal_events SET correlation_id = ? \
                     WHERE event_type = 'conversation.created'",
                )
                .bind(CorrelationId::generate().to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::WrongCausation => {
                sqlx::query(
                    "UPDATE journal_events SET causation_event_id = NULL \
                     WHERE event_type = 'conversation.created'",
                )
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::MissingHead => {
                sqlx::query("DELETE FROM stream_heads WHERE stream_id LIKE 'conversation:%'")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
            }
            BootstrapCorruption::WrongHead => {
                sqlx::query(
                    "UPDATE stream_heads SET last_stream_seq = 2 \
                     WHERE stream_id LIKE 'conversation:%'",
                )
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::ExtraHead => {
                sqlx::query("INSERT INTO stream_heads VALUES (?, 1)")
                    .bind(JournalStreamId::Work(WorkId::generate()).to_string())
                    .execute(&mut *connection)
                    .await
                    .unwrap();
            }
            BootstrapCorruption::NullRootLinks => {
                sqlx::query(
                    "UPDATE craxii_principals SET primary_conversation_id = NULL, \
                     default_workspace_id = NULL",
                )
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::ExtraPrincipal => {
                sqlx::query(
                    "INSERT INTO craxii_principals \
                     (craxii_id, display_name, owner_label, lifecycle_state, \
                      primary_conversation_id, default_workspace_id, created_at, \
                      architecture_revision, schema_revision) \
                     VALUES (?, 'Craxii', 'local-owner', 'active', NULL, NULL, ?, 'V0.0.01', 2)",
                )
                .bind(CraxiiId::generate().to_string())
                .bind(AT)
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::ExtraWorkstation => {
                sqlx::query(
                    "INSERT INTO workstations SELECT ?, craxii_id, kind, generation, \
                     hosting_provider, provider_instance_id, provider_image_id, \
                     provisioning_revision, architecture, os_release, capabilities_json, \
                     created_at, last_seen_at FROM workstations",
                )
                .bind(WorkstationId::generate().to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::ExtraWorkspace => {
                sqlx::query(
                    "INSERT INTO workspaces SELECT ?, craxii_id, workstation_id, 'extra', \
                     '/extra', '/extra', lifecycle_state, created_at FROM workspaces",
                )
                .bind(WorkspaceId::generate().to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::ExtraPrimaryConversation => {
                let extra_craxii = CraxiiId::generate();
                sqlx::query(
                    "INSERT INTO craxii_principals \
                     (craxii_id, display_name, owner_label, lifecycle_state, \
                      primary_conversation_id, default_workspace_id, created_at, \
                      architecture_revision, schema_revision) \
                     VALUES (?, 'Craxii', 'local-owner', 'active', NULL, NULL, ?, 'V0.0.01', 2)",
                )
                .bind(extra_craxii.to_string())
                .bind(AT)
                .execute(&mut *connection)
                .await
                .unwrap();
                sqlx::query(
                    "INSERT INTO conversations VALUES (?, ?, 'primary', 'active', 1, 1, ?)",
                )
                .bind(ConversationId::generate().to_string())
                .bind(extra_craxii.to_string())
                .bind(AT)
                .execute(&mut *connection)
                .await
                .unwrap();
            }
            BootstrapCorruption::SchemaRevision => {
                sqlx::query("UPDATE craxii_principals SET schema_revision = 1")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
            }
            BootstrapCorruption::InitializationPayload => {
                let changed: String = sqlx::query_scalar(
                    "SELECT json_set(payload_json, '$.display_name', 'Other') \
                     FROM journal_events WHERE event_type = 'craxii.initialized'",
                )
                .fetch_one(&mut *connection)
                .await
                .unwrap();
                let digest = Sha256Digest::hash_bytes(changed.as_bytes());
                sqlx::query(
                    "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? \
                     WHERE event_type = 'craxii.initialized'",
                )
                .bind(changed)
                .bind(digest.to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            }
        }
        drop(connection);
        let event_count_before = count(guard.runtime(), "journal_events").await;
        assert!(
            store.verify_application_consistency().await.is_err(),
            "{corruption:?}"
        );
        assert_eq!(
            count(guard.runtime(), "journal_events").await,
            event_count_before,
            "{corruption:?}"
        );
        guard.shutdown().await;
    }
}

#[test]
fn exact_registry_has_two_stage_seven_emitters_and_twenty_eight_total_events() {
    fn requires_bootstrap_capability<T: BootstrapStateStore>() {}
    requires_bootstrap_capability::<SqliteStateStore>();
    assert_eq!(JournalEventKind::ALL.len(), 28);
    assert_eq!(
        JournalEventKind::ALL
            .iter()
            .filter(|kind| kind.emitted_in_stage_7())
            .count(),
        2
    );
}
