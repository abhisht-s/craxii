use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::{ConnectOptions, Connection, Row, SqlSafeStr};
use tokio::sync::Barrier;

use crate::domain::{
    ContentBlock, ConversationId, ConversationWorkOrdinal, CraxiiId, CurrentWorkAttempt, DeviceId,
    LogicalPathReference, MessageContent, ModelInvocationId, ProjectionVersion, RuntimeInstanceId,
    Sha256Digest, ToolExecutionId, UtcTimestamp, WorkCancellationReason, WorkCompletionReason,
    WorkId, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput, WorkState, WorkTerminalReason,
    WorkspaceCapabilityRef, WorkspaceId, WorkstationCapabilities, WorkstationCapabilitiesInput,
    WorkstationCapabilityFlags, WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits,
    WorkstationGeneration, WorkstationId, WorkstationKind,
};

use super::codec::{
    DecodedWorkstationRecord, decode_message_row, decode_workstation_row, encode_message_content,
    encode_workstation_capabilities,
};
use super::error::SqliteFailureKind;
use super::projection::{
    ConversationGuardConflict, ProjectionMutationError, WorkGuardConflict, WorkProjectionTimes,
    advance_conversation_ordinal, guarded_work_update,
};
use super::runtime::{SqliteRuntime, SqliteRuntimeGuard};
use super::schema::{MIGRATOR, PRODUCT_INDEXES, PRODUCT_TABLES, SQLX_CHECKSUM_LENGTH};
use super::transaction::WriteTransaction;

const NOW: &str = "2026-08-28T01:02:03.456789Z";
const LATER: &str = "2026-08-28T01:03:04.567890Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage6-test-{}-{}",
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

async fn database() -> (TestRoot, SqliteRuntimeGuard) {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    (root, guard)
}

#[derive(Clone)]
struct Fixture {
    craxii_id: String,
    workstation_id: String,
    workspace_id: String,
    conversation_id: String,
    runtime_id: String,
    device_id: String,
    token_hash: String,
}

fn workstation_capabilities(fixture: &Fixture) -> WorkstationCapabilities {
    WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
        workstation_id: WorkstationId::parse_canonical(&fixture.workstation_id).unwrap(),
        generation: WorkstationGeneration::try_new(1).unwrap(),
        cpu_architecture: "aarch64".to_owned(),
        os_release: "ubuntu".to_owned(),
        default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
        flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
            filesystem_read: true,
            foreground_execute: true,
            cancel_execution: true,
            inspect_execution: true,
            privilege_user: true,
            privilege_administrative: false,
            process_group_cleanup: true,
            cgroup_cleanup: true,
        }),
        limits: WorkstationCapabilityLimits::try_new(60_000, 1_048_576, 524_288).unwrap(),
        workspaces: vec![
            WorkspaceCapabilityRef::try_new(
                WorkspaceId::parse_canonical(&fixture.workspace_id).unwrap(),
                LogicalPathReference::absolute("/workspace").unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

async fn seed_topology(runtime: &SqliteRuntime) -> Fixture {
    let device_id = DeviceId::generate().to_string();
    let fixture = Fixture {
        craxii_id: CraxiiId::generate().to_string(),
        workstation_id: WorkstationId::generate().to_string(),
        workspace_id: WorkspaceId::generate().to_string(),
        conversation_id: ConversationId::generate().to_string(),
        runtime_id: RuntimeInstanceId::generate().to_string(),
        token_hash: Sha256Digest::hash_bytes(device_id.as_bytes()).to_string(),
        device_id,
    };
    let mut connection = runtime.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO craxii_principals \
         (craxii_id, display_name, owner_label, lifecycle_state, primary_conversation_id, \
          default_workspace_id, created_at, architecture_revision, schema_revision) \
         VALUES (?, 'Craxii', 'owner', 'active', NULL, NULL, ?, 'V0.0.01', 1)",
    )
    .bind(&fixture.craxii_id)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    let capabilities_json =
        encode_workstation_capabilities(&workstation_capabilities(&fixture)).unwrap();
    sqlx::query(
        "INSERT INTO workstations \
         (workstation_id, craxii_id, kind, generation, hosting_provider, provider_instance_id, \
          provider_image_id, provisioning_revision, architecture, os_release, capabilities_json, \
          created_at, last_seen_at) \
         VALUES (?, ?, 'local', 1, 'aws', 'i-safe', 'ami-safe', 'rev-safe', 'aarch64', \
                 'ubuntu', ?, ?, ?)",
    )
    .bind(&fixture.workstation_id)
    .bind(&fixture.craxii_id)
    .bind(capabilities_json)
    .bind(NOW)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workspaces \
         (workspace_id, craxii_id, workstation_id, logical_name, logical_root, \
          local_resolved_root, lifecycle_state, created_at) \
         VALUES (?, ?, ?, 'default', '/workspace', '/srv/workspace', 'active', ?)",
    )
    .bind(&fixture.workspace_id)
    .bind(&fixture.craxii_id)
    .bind(&fixture.workstation_id)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversations \
         (conversation_id, craxii_id, kind, lifecycle_state, next_work_ordinal, state_version, created_at) \
         VALUES (?, ?, 'primary', 'active', 1, 1, ?)",
    )
    .bind(&fixture.conversation_id)
    .bind(&fixture.craxii_id)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE craxii_principals SET primary_conversation_id = ?, default_workspace_id = ? \
         WHERE craxii_id = ?",
    )
    .bind(&fixture.conversation_id)
    .bind(&fixture.workspace_id)
    .bind(&fixture.craxii_id)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO runtime_instances \
         (runtime_instance_id, craxii_id, workstation_id, workstation_generation, linux_boot_id, \
          process_id, binary_version, git_revision, schema_version, state, started_at, \
          last_heartbeat_at, stopped_at, stop_reason) \
         VALUES (?, ?, ?, 1, 'boot-safe', 42, '0.0.1', 'revision', 1, 'running', ?, NULL, NULL, NULL)",
    )
    .bind(&fixture.runtime_id)
    .bind(&fixture.craxii_id)
    .bind(&fixture.workstation_id)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO client_devices (device_id, display_name, token_hash, created_at, last_seen_at, revoked_at) \
         VALUES (?, 'device', ?, ?, NULL, NULL)",
    )
    .bind(&fixture.device_id)
    .bind(&fixture.token_hash)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
    fixture
}

#[derive(Default)]
struct WorkstationRowOverrides<'a> {
    workstation_id: Option<&'a str>,
    kind: Option<&'a str>,
    generation: Option<i64>,
    architecture: Option<&'a str>,
    os_release: Option<&'a str>,
    capabilities_json: Option<&'a str>,
}

async fn decode_fixture_workstation(
    runtime: &SqliteRuntime,
    fixture: &Fixture,
    overrides: WorkstationRowOverrides<'_>,
) -> Result<DecodedWorkstationRecord, super::SqliteAdapterError> {
    let mut connection = runtime.acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT COALESCE(?, workstation_id) AS workstation_id, craxii_id, \
         COALESCE(?, kind) AS kind, COALESCE(?, generation) AS generation, hosting_provider, \
         provider_instance_id, provider_image_id, provisioning_revision, \
         COALESCE(?, architecture) AS architecture, COALESCE(?, os_release) AS os_release, \
         COALESCE(?, capabilities_json) AS capabilities_json, created_at, last_seen_at \
         FROM workstations WHERE workstation_id = ?",
    )
    .bind(overrides.workstation_id)
    .bind(overrides.kind)
    .bind(overrides.generation)
    .bind(overrides.architecture)
    .bind(overrides.os_release)
    .bind(overrides.capabilities_json)
    .bind(&fixture.workstation_id)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    decode_workstation_row(&row)
}

#[derive(Clone)]
struct WorkRow {
    work_id: String,
    ordinal: i64,
    state: &'static str,
    state_version: i64,
    priority: i64,
    runtime: Option<String>,
    model: Option<String>,
    tool: Option<String>,
    started: Option<&'static str>,
    cancel_at: Option<&'static str>,
    cancel_reason: Option<&'static str>,
    terminal_at: Option<&'static str>,
    terminal_reason: Option<&'static str>,
    detail: Option<&'static str>,
}

impl WorkRow {
    fn for_state(fixture: &Fixture, state: &'static str, ordinal: i64) -> Self {
        let mut row = Self {
            work_id: WorkId::generate().to_string(),
            ordinal,
            state,
            state_version: 1,
            priority: 0,
            runtime: None,
            model: None,
            tool: None,
            started: None,
            cancel_at: None,
            cancel_reason: None,
            terminal_at: None,
            terminal_reason: None,
            detail: None,
        };
        match state {
            "queued" => {}
            "running" => {
                row.runtime = Some(fixture.runtime_id.clone());
                row.started = Some(NOW);
            }
            "waiting_on_model" => {
                row.runtime = Some(fixture.runtime_id.clone());
                row.model = Some(ModelInvocationId::generate().to_string());
                row.started = Some(NOW);
            }
            "waiting_on_tool" => {
                row.runtime = Some(fixture.runtime_id.clone());
                row.tool = Some(ToolExecutionId::generate().to_string());
                row.started = Some(NOW);
            }
            "cancel_requested" => {
                row.runtime = Some(fixture.runtime_id.clone());
                row.started = Some(NOW);
                row.cancel_at = Some(LATER);
                row.cancel_reason = Some("user_request");
            }
            "completed" => {
                row.started = Some(NOW);
                row.terminal_at = Some(LATER);
                row.terminal_reason = Some("answered");
            }
            "failed" => {
                row.started = Some(NOW);
                row.terminal_at = Some(LATER);
                row.terminal_reason = Some("provider_exhausted");
            }
            "cancelled" => {
                row.terminal_at = Some(LATER);
                row.terminal_reason = Some("user_request");
            }
            "interrupted" => {
                row.started = Some(NOW);
                row.terminal_at = Some(LATER);
                row.terminal_reason = Some("runtime_ownership_lost");
            }
            _ => {}
        }
        row
    }
}

async fn insert_work(
    runtime: &SqliteRuntime,
    fixture: &Fixture,
    row: &WorkRow,
) -> Result<(), sqlx::Error> {
    let mut connection = runtime.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO work_items \
         (work_id, craxii_id, conversation_id, conversation_work_ordinal, kind, state, \
          state_version, priority, workspace_id, runtime_instance_id, \
          current_model_invocation_id, current_tool_execution_id, correlation_id, created_at, \
          queued_at, started_at, cancel_requested_at, cancellation_reason_code, terminal_at, \
          terminal_reason_code, terminal_detail_json) \
         VALUES (?, ?, ?, ?, 'conversational', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.work_id)
    .bind(&fixture.craxii_id)
    .bind(&fixture.conversation_id)
    .bind(row.ordinal)
    .bind(row.state)
    .bind(row.state_version)
    .bind(row.priority)
    .bind(&fixture.workspace_id)
    .bind(row.runtime.as_deref())
    .bind(row.model.as_deref())
    .bind(row.tool.as_deref())
    .bind(uuid::Uuid::now_v7().hyphenated().to_string())
    .bind(NOW)
    .bind(NOW)
    .bind(row.started)
    .bind(row.cancel_at)
    .bind(row.cancel_reason)
    .bind(row.terminal_at)
    .bind(row.terminal_reason)
    .bind(row.detail)
    .execute(&mut *connection)
    .await
    .map(|_| ())
}

async fn delete_work(runtime: &SqliteRuntime, work_id: &str) {
    let mut connection = runtime.acquire().await.unwrap();
    sqlx::query("DELETE FROM work_items WHERE work_id = ?")
        .bind(work_id)
        .execute(&mut *connection)
        .await
        .unwrap();
}

#[tokio::test]
async fn migration_metadata_table_policy_and_empty_inventory_are_exact() {
    let (_root, guard) = database().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    let migrations = sqlx::query(
        "SELECT version, description, success, length(checksum) AS checksum_length, checksum, \
         execution_time FROM _sqlx_migrations",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(migrations.len(), 3);
    for (migration, embedded) in migrations.iter().zip(MIGRATOR.iter()) {
        assert_eq!(migration.get::<i64, _>("version"), embedded.version);
        assert_eq!(
            migration.get::<String, _>("description"),
            embedded.description
        );
        assert_eq!(migration.get::<i64, _>("success"), 1);
        assert_eq!(migration.get::<i64, _>("checksum_length"), 48);
        assert!(migration.get::<i64, _>("execution_time") >= 0);
        let checksum = migration.get::<Vec<u8>, _>("checksum");
        assert_eq!(checksum.len(), SQLX_CHECKSUM_LENGTH);
        assert_eq!(checksum.as_slice(), embedded.checksum.as_ref());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        0
    );

    let table_list = sqlx::query("PRAGMA table_list")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    for table in PRODUCT_TABLES {
        let row = table_list
            .iter()
            .find(|row| row.get::<String, _>("name") == *table)
            .unwrap();
        assert_eq!(row.get::<i64, _>("strict"), 1, "{table}");
        assert_eq!(
            row.get::<i64, _>("wr"),
            i64::from(*table != "journal_events"),
            "{table}"
        );
        let count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table}"
        )))
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
        let foreign_keys = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA foreign_key_list('{table}')"
        )))
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        for foreign_key in foreign_keys {
            assert_eq!(foreign_key.get::<String, _>("on_update"), "RESTRICT");
            assert_eq!(foreign_key.get::<String, _>("on_delete"), "RESTRICT");
            if *table == "work_items" {
                let from = foreign_key.get::<String, _>("from");
                assert_ne!(from, "current_model_invocation_id");
                assert_ne!(from, "current_tool_execution_id");
            }
        }
    }
    assert!(
        sqlx::query("PRAGMA quick_check")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .iter()
            .all(|row| row.get::<String, _>(0) == "ok")
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .is_empty()
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn stage5_metadata_only_database_migrates_forward_to_current_version_three() {
    let (root, guard) = database().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    for table in [
        "context_manifest_sources",
        "context_manifests",
        "tool_executions",
        "model_invocations",
        "artifacts",
        "work_item_inputs",
        "stream_heads",
        "journal_events",
        "messages",
        "client_commands",
        "work_items",
        "client_devices",
        "runtime_instances",
        "conversations",
        "workspaces",
        "workstations",
        "craxii_principals",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM _sqlx_migrations")
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
    let mut connection = migrated.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        3
    );
    drop(connection);
    migrated.shutdown().await;
}

#[tokio::test]
async fn isolated_failed_sqlx_migration_rolls_back_without_fake_production_history() {
    let root = TestRoot::new();
    let path = root.path().join("rollback.sqlite3");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true);
    let mut connection = options.connect().await.unwrap();
    let migration = sqlx::migrate::Migration::new(
        1,
        "isolated rollback probe".into(),
        sqlx::migrate::MigrationType::Simple,
        sqlx::AssertSqlSafe(
            "CREATE TABLE rollback_probe (value INTEGER NOT NULL CHECK (value > 0)); \
             INSERT INTO rollback_probe VALUES (0);",
        )
        .into_sql_str(),
        false,
    );
    let migrator = sqlx::migrate::Migrator::with_migrations(vec![migration]);
    assert!(migrator.run(&mut connection).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'rollback_probe'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        0
    );
    connection.close().await.unwrap();
}

#[tokio::test]
async fn workstation_row_decoder_reconstructs_domain_and_rejects_every_contradiction() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;

    let decoded = decode_fixture_workstation(
        guard.runtime(),
        &fixture,
        WorkstationRowOverrides::default(),
    )
    .await
    .unwrap();
    let identity = decoded.identity();
    assert_eq!(
        identity.workstation_id().to_string(),
        fixture.workstation_id
    );
    assert_eq!(identity.craxii_id().to_string(), fixture.craxii_id);
    assert_eq!(identity.kind(), WorkstationKind::Local);
    assert_eq!(identity.generation().get(), 1);
    assert_eq!(identity.hosting_provider().as_str(), "aws");
    assert_eq!(identity.provider_instance_id(), Some("i-safe"));
    assert_eq!(identity.image_id(), Some("ami-safe"));
    assert_eq!(identity.provisioning_revision(), Some("rev-safe"));
    assert_eq!(identity.cpu_architecture(), "aarch64");
    assert_eq!(identity.os_release(), "ubuntu");
    assert_eq!(identity.created_at().to_string(), NOW);
    assert_eq!(decoded.last_seen_at().to_string(), NOW);
    assert_eq!(decoded.capabilities(), &workstation_capabilities(&fixture));

    for overrides in [
        WorkstationRowOverrides {
            generation: Some(2),
            ..WorkstationRowOverrides::default()
        },
        WorkstationRowOverrides {
            generation: Some(0),
            ..WorkstationRowOverrides::default()
        },
        WorkstationRowOverrides {
            generation: Some(-1),
            ..WorkstationRowOverrides::default()
        },
        WorkstationRowOverrides {
            kind: Some("remote"),
            ..WorkstationRowOverrides::default()
        },
        WorkstationRowOverrides {
            architecture: Some("x86_64"),
            ..WorkstationRowOverrides::default()
        },
        WorkstationRowOverrides {
            os_release: Some("Ubuntu 24.04 LTS"),
            ..WorkstationRowOverrides::default()
        },
    ] {
        let error = decode_fixture_workstation(guard.runtime(), &fixture, overrides)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), SqliteFailureKind::InconsistentSchema);
    }

    let mut other_workstation = fixture.clone();
    other_workstation.workstation_id = WorkstationId::generate().to_string();
    let mismatched_id_json =
        encode_workstation_capabilities(&workstation_capabilities(&other_workstation)).unwrap();
    let error = decode_fixture_workstation(
        guard.runtime(),
        &fixture,
        WorkstationRowOverrides {
            workstation_id: Some(&other_workstation.workstation_id),
            ..WorkstationRowOverrides::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), SqliteFailureKind::InconsistentSchema);

    for capabilities_json in [&mismatched_id_json, "not-json", "{}"] {
        let error = decode_fixture_workstation(
            guard.runtime(),
            &fixture,
            WorkstationRowOverrides {
                capabilities_json: Some(capabilities_json),
                ..WorkstationRowOverrides::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), SqliteFailureKind::InconsistentSchema);
    }

    guard.shutdown().await;
}

#[tokio::test]
async fn scalar_json_literal_foreign_key_and_uniqueness_constraints_fire() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    for invalid_id in [
        "not-a-uuid".to_owned(),
        fixture.device_id.to_uppercase(),
        "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        format!("-{}", &fixture.device_id[1..]),
    ] {
        assert!(
            sqlx::query("INSERT INTO client_devices VALUES (?, 'bad', ?, ?, NULL, NULL)")
                .bind(invalid_id)
                .bind("b".repeat(64))
                .bind(NOW)
                .execute(&mut *connection)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("INSERT INTO client_devices VALUES (?, 'bad', ?, ?, NULL, NULL)")
            .bind(DeviceId::generate().to_string())
            .bind("A".repeat(64))
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO client_devices VALUES (?, 'bad', ?, ?, NULL, NULL)")
            .bind(DeviceId::generate().to_string())
            .bind("b".repeat(64))
            .bind("2026-02-30T01:02:03Z")
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE workstations SET capabilities_json = 'not-json' WHERE workstation_id = ?"
        )
        .bind(&fixture.workstation_id)
        .execute(&mut *connection)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("UPDATE workstations SET kind = 'remote' WHERE workstation_id = ?")
            .bind(&fixture.workstation_id)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE craxii_principals SET primary_conversation_id = NULL WHERE craxii_id = ?"
        )
        .bind(&fixture.craxii_id)
        .execute(&mut *connection)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("UPDATE craxii_principals SET primary_conversation_id = ?, default_workspace_id = ? WHERE craxii_id = ?")
            .bind(ConversationId::generate().to_string())
            .bind(&fixture.workspace_id)
            .bind(&fixture.craxii_id)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO conversations VALUES (?, ?, 'primary', 'active', 1, 1, ?)")
            .bind(ConversationId::generate().to_string())
            .bind(&fixture.craxii_id)
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO workspaces VALUES (?, ?, ?, 'default', '/other', '/srv/other', 'active', ?)")
            .bind(WorkspaceId::generate().to_string())
            .bind(&fixture.craxii_id)
            .bind(&fixture.workstation_id)
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO client_devices VALUES (?, 'duplicate token', ?, ?, NULL, NULL)")
            .bind(DeviceId::generate().to_string())
            .bind(&fixture.token_hash)
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn runtime_state_and_stop_reason_shapes_are_exact() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    let base = "INSERT INTO runtime_instances \
        (runtime_instance_id, craxii_id, workstation_id, workstation_generation, linux_boot_id, \
         process_id, binary_version, git_revision, schema_version, state, started_at, \
         last_heartbeat_at, stopped_at, stop_reason) VALUES (?, ?, ?, 1, 'boot', 43, '0.0.1', \
         'revision', 1, ?, ?, NULL, ?, ?)";
    for (state, stopped_at, stop_reason) in [
        ("running", None, None),
        ("stopping", None, None),
        ("stopping", None, Some("graceful_shutdown")),
        ("stopped", Some(LATER), Some("graceful_shutdown")),
        ("stopped", Some(LATER), Some("startup_failure")),
    ] {
        sqlx::query(base)
            .bind(RuntimeInstanceId::generate().to_string())
            .bind(&fixture.craxii_id)
            .bind(&fixture.workstation_id)
            .bind(state)
            .bind(NOW)
            .bind(stopped_at)
            .bind(stop_reason)
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    for (state, stopped_at, stop_reason) in [
        ("unknown", None, None),
        ("running", Some(LATER), Some("graceful_shutdown")),
        ("stopping", None, Some("startup_failure")),
        ("stopping", Some(LATER), Some("graceful_shutdown")),
        ("stopped", None, Some("graceful_shutdown")),
        ("stopped", Some(LATER), None),
        ("stopped", Some(LATER), Some("unknown")),
    ] {
        assert!(
            sqlx::query(base)
                .bind(RuntimeInstanceId::generate().to_string())
                .bind(&fixture.craxii_id)
                .bind(&fixture.workstation_id)
                .bind(state)
                .bind(NOW)
                .bind(stopped_at)
                .bind(stop_reason)
                .execute(&mut *connection)
                .await
                .is_err()
        );
    }
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn every_work_state_shape_accepts_legal_rows_and_rejects_independent_corruption() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let states = [
        "queued",
        "running",
        "waiting_on_model",
        "waiting_on_tool",
        "cancel_requested",
        "completed",
        "failed",
        "cancelled",
        "interrupted",
    ];
    for (index, state) in states.into_iter().enumerate() {
        let row = WorkRow::for_state(&fixture, state, i64::try_from(index + 1).unwrap());
        insert_work(guard.runtime(), &fixture, &row).await.unwrap();
        delete_work(guard.runtime(), &row.work_id).await;

        let mut invalid = Vec::new();
        let mut candidate = row.clone();
        candidate.work_id = WorkId::generate().to_string();
        candidate.state_version = 0;
        invalid.push(candidate);
        let mut candidate = row.clone();
        candidate.work_id = WorkId::generate().to_string();
        candidate.ordinal = 0;
        invalid.push(candidate);
        let mut candidate = row.clone();
        candidate.work_id = WorkId::generate().to_string();
        candidate.priority = 1;
        invalid.push(candidate);

        match state {
            "queued" => {
                for mutation in 0..8 {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    match mutation {
                        0 => candidate.runtime = Some(fixture.runtime_id.clone()),
                        1 => candidate.model = Some(ModelInvocationId::generate().to_string()),
                        2 => candidate.tool = Some(ToolExecutionId::generate().to_string()),
                        3 => candidate.started = Some(NOW),
                        4 => candidate.cancel_at = Some(LATER),
                        5 => candidate.cancel_reason = Some("user_request"),
                        6 => candidate.terminal_at = Some(LATER),
                        _ => candidate.terminal_reason = Some("answered"),
                    }
                    invalid.push(candidate);
                }
            }
            "running" | "waiting_on_model" | "waiting_on_tool" => {
                let mut candidate = row.clone();
                candidate.work_id = WorkId::generate().to_string();
                candidate.runtime = None;
                invalid.push(candidate);
                let mut candidate = row.clone();
                candidate.work_id = WorkId::generate().to_string();
                candidate.started = None;
                invalid.push(candidate);
                let mut candidate = row.clone();
                candidate.work_id = WorkId::generate().to_string();
                candidate.cancel_at = Some(LATER);
                invalid.push(candidate);
                let mut candidate = row.clone();
                candidate.work_id = WorkId::generate().to_string();
                candidate.terminal_at = Some(LATER);
                candidate.terminal_reason = Some("answered");
                invalid.push(candidate);
                if state == "running" || state == "waiting_on_tool" {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    candidate.model = Some(ModelInvocationId::generate().to_string());
                    invalid.push(candidate);
                }
                if state == "running" || state == "waiting_on_model" {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    candidate.tool = Some(ToolExecutionId::generate().to_string());
                    invalid.push(candidate);
                }
                if state == "waiting_on_model" {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    candidate.model = None;
                    invalid.push(candidate);
                }
                if state == "waiting_on_tool" {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    candidate.tool = None;
                    invalid.push(candidate);
                }
            }
            "cancel_requested" => {
                for mutation in 0..6 {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    match mutation {
                        0 => candidate.runtime = None,
                        1 => candidate.started = None,
                        2 => candidate.cancel_at = None,
                        3 => candidate.cancel_reason = None,
                        4 => candidate.terminal_at = Some(LATER),
                        _ => {
                            candidate.model = Some(ModelInvocationId::generate().to_string());
                            candidate.tool = Some(ToolExecutionId::generate().to_string());
                        }
                    }
                    invalid.push(candidate);
                }
            }
            "completed" | "failed" | "cancelled" | "interrupted" => {
                for mutation in 0..7 {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    match mutation {
                        0 => candidate.runtime = Some(fixture.runtime_id.clone()),
                        1 => candidate.model = Some(ModelInvocationId::generate().to_string()),
                        2 => candidate.tool = Some(ToolExecutionId::generate().to_string()),
                        3 => candidate.terminal_at = None,
                        4 => candidate.cancel_at = Some(LATER),
                        5 => candidate.cancel_reason = Some("user_request"),
                        _ => candidate.detail = Some("{}"),
                    }
                    invalid.push(candidate);
                }
                if state != "cancelled" {
                    let mut candidate = row.clone();
                    candidate.work_id = WorkId::generate().to_string();
                    candidate.started = None;
                    invalid.push(candidate);
                }
                let mut candidate = row.clone();
                candidate.work_id = WorkId::generate().to_string();
                candidate.terminal_reason = Some(match state {
                    "completed" => "provider_exhausted",
                    "failed" => "answered",
                    "cancelled" => "cleanup_unconfirmed",
                    _ => "user_request",
                });
                invalid.push(candidate);
            }
            _ => unreachable!(),
        }

        for (candidate_index, candidate) in invalid.into_iter().enumerate() {
            assert!(
                insert_work(guard.runtime(), &fixture, &candidate)
                    .await
                    .is_err(),
                "state {state} accepted invalid mutation {candidate_index}"
            );
        }
    }

    for (reason, detail) in [
        ("definite_normalized_error", r#"{"safe":"detail"}"#),
        ("invalid_model_output", r#"{"version":1}"#),
        ("lifecycle_limit", r#"{"version":1}"#),
    ] {
        let mut row = WorkRow::for_state(&fixture, "failed", 100);
        row.terminal_reason = Some(reason);
        row.detail = Some(detail);
        insert_work(guard.runtime(), &fixture, &row).await.unwrap();
        delete_work(guard.runtime(), &row.work_id).await;
    }
    let mut direct = WorkRow::for_state(&fixture, "cancelled", 101);
    direct.started = None;
    insert_work(guard.runtime(), &fixture, &direct)
        .await
        .unwrap();
    delete_work(guard.runtime(), &direct.work_id).await;
    guard.shutdown().await;
}

#[tokio::test]
async fn messages_enforce_provenance_identity_and_content_hash_codec() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let work = WorkRow::for_state(&fixture, "queued", 1);
    insert_work(guard.runtime(), &fixture, &work).await.unwrap();
    let content =
        MessageContent::try_new(vec![ContentBlock::text("hello\n世界").unwrap()]).unwrap();
    let (content_json, digest) = encode_message_content(&content).unwrap();
    let client_message = uuid::Uuid::now_v7().hyphenated().to_string();
    let user_message = uuid::Uuid::now_v7().hyphenated().to_string();
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query("INSERT INTO messages VALUES (?, ?, ?, 'user', ?, ?, NULL, ?, ?, ?)")
        .bind(&user_message)
        .bind(&fixture.craxii_id)
        .bind(&fixture.conversation_id)
        .bind(&content_json)
        .bind(digest.to_string())
        .bind(&fixture.device_id)
        .bind(&client_message)
        .bind(NOW)
        .execute(&mut *connection)
        .await
        .unwrap();
    let row = sqlx::query("SELECT * FROM messages WHERE message_id = ?")
        .bind(&user_message)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let decoded = decode_message_row(&row).unwrap();
    assert_eq!(decoded.content(), &content);
    assert_eq!(decoded.content_sha256(), digest);

    for (role, produced, device, client) in [
        ("user", None, None, None),
        ("assistant", None, None, None),
        ("system", Some(work.work_id.as_str()), None, None),
        (
            "assistant",
            Some(work.work_id.as_str()),
            Some(fixture.device_id.as_str()),
            Some(client_message.as_str()),
        ),
    ] {
        assert!(
            sqlx::query("INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(uuid::Uuid::now_v7().hyphenated().to_string())
                .bind(&fixture.craxii_id)
                .bind(&fixture.conversation_id)
                .bind(role)
                .bind(&content_json)
                .bind(digest.to_string())
                .bind(produced)
                .bind(device)
                .bind(client)
                .bind(NOW)
                .execute(&mut *connection)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("INSERT INTO messages VALUES (?, ?, ?, 'user', ?, ?, NULL, ?, ?, ?)")
            .bind(uuid::Uuid::now_v7().hyphenated().to_string())
            .bind(&fixture.craxii_id)
            .bind(&fixture.conversation_id)
            .bind(&content_json)
            .bind(digest.to_string())
            .bind(&fixture.device_id)
            .bind(&client_message)
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    sqlx::query("INSERT INTO messages VALUES (?, ?, ?, 'assistant', ?, ?, ?, NULL, NULL, ?)")
        .bind(uuid::Uuid::now_v7().hyphenated().to_string())
        .bind(&fixture.craxii_id)
        .bind(&fixture.conversation_id)
        .bind(&content_json)
        .bind(digest.to_string())
        .bind(&work.work_id)
        .bind(NOW)
        .execute(&mut *connection)
        .await
        .unwrap();
    assert!(
        sqlx::query("INSERT INTO messages VALUES (?, ?, ?, 'assistant', ?, ?, ?, NULL, NULL, ?)")
            .bind(uuid::Uuid::now_v7().hyphenated().to_string())
            .bind(&fixture.craxii_id)
            .bind(&fixture.conversation_id)
            .bind(&content_json)
            .bind(digest.to_string())
            .bind(&work.work_id)
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO messages VALUES (?, ?, ?, 'user', ?, ?, NULL, ?, ?, ?)")
            .bind(uuid::Uuid::now_v7().hyphenated().to_string())
            .bind(&fixture.craxii_id)
            .bind(&fixture.conversation_id)
            .bind(&content_json)
            .bind(digest.to_string())
            .bind(DeviceId::generate().to_string())
            .bind(uuid::Uuid::now_v7().hyphenated().to_string())
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn command_constraints_and_duplicate_command_race_are_database_enforced() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    for (command_type, hash, status, response, cursor) in [
        ("unknown", "b".repeat(64), 200_i64, "{}", 1_i64),
        ("message", "B".repeat(64), 200, "{}", 1),
        ("message", "b".repeat(64), 99, "{}", 1),
        ("message", "b".repeat(64), 600, "{}", 1),
        ("message", "b".repeat(64), 200, "not-json", 1),
        ("message", "b".repeat(64), 200, "{}", 0),
    ] {
        assert!(
            sqlx::query("INSERT INTO client_commands VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&fixture.device_id)
                .bind(uuid::Uuid::now_v7().hyphenated().to_string())
                .bind(command_type)
                .bind(hash)
                .bind(status)
                .bind(response)
                .bind(cursor)
                .bind(NOW)
                .execute(&mut *connection)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("INSERT INTO client_commands VALUES (?, ?, 'message', ?, 200, '{}', 1, ?)")
            .bind(&fixture.device_id)
            .bind("550e8400-e29b-41d4-a716-446655440000")
            .bind("b".repeat(64))
            .bind(NOW)
            .execute(&mut *connection)
            .await
            .is_err()
    );
    drop(connection);

    let key = uuid::Uuid::now_v7().hyphenated().to_string();
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let runtime = guard.runtime().clone();
        let barrier = barrier.clone();
        let device = fixture.device_id.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            let mut connection = runtime.acquire().await.unwrap();
            barrier.wait().await;
            sqlx::query("INSERT INTO client_commands VALUES (?, ?, 'message', ?, 202, '{}', 1, ?)")
                .bind(device)
                .bind(key)
                .bind("c".repeat(64))
                .bind(NOW)
                .execute(&mut *connection)
                .await
                .is_ok()
        }));
    }
    let outcomes = futures_join(tasks).await;
    assert_eq!(outcomes.iter().filter(|outcome| **outcome).count(), 1);
    guard.shutdown().await;
}

async fn futures_join(tasks: Vec<tokio::task::JoinHandle<bool>>) -> Vec<bool> {
    let mut outcomes = Vec::with_capacity(tasks.len());
    for task in tasks {
        outcomes.push(task.await.unwrap());
    }
    outcomes
}

#[tokio::test]
async fn ordinal_and_one_active_partial_unique_races_have_one_winner() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    for active in [false, true] {
        let barrier = Arc::new(Barrier::new(2));
        let mut tasks = Vec::new();
        for ordinal in [1_i64, if active { 2 } else { 1 }] {
            let runtime = guard.runtime().clone();
            let barrier = barrier.clone();
            let fixture = fixture.clone();
            tasks.push(tokio::spawn(async move {
                let row = WorkRow::for_state(
                    &fixture,
                    if active { "running" } else { "queued" },
                    ordinal,
                );
                barrier.wait().await;
                insert_work(&runtime, &fixture, &row).await.is_ok()
            }));
        }
        let outcomes = futures_join(tasks).await;
        assert_eq!(outcomes.iter().filter(|outcome| **outcome).count(), 1);
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("DELETE FROM work_items")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    guard.shutdown().await;
}

#[tokio::test]
async fn current_model_and_tool_identities_are_globally_unique_while_present() {
    let (_root, guard) = database().await;
    let first = seed_topology(guard.runtime()).await;
    let second = seed_topology(guard.runtime()).await;
    let model = ModelInvocationId::generate().to_string();
    let mut first_model = WorkRow::for_state(&first, "waiting_on_model", 1);
    first_model.model = Some(model.clone());
    insert_work(guard.runtime(), &first, &first_model)
        .await
        .unwrap();
    let mut second_model = WorkRow::for_state(&second, "waiting_on_model", 1);
    second_model.model = Some(model);
    assert!(
        insert_work(guard.runtime(), &second, &second_model)
            .await
            .is_err()
    );

    let tool = ToolExecutionId::generate().to_string();
    let mut first_tool = WorkRow::for_state(&first, "waiting_on_tool", 2);
    first_tool.tool = Some(tool.clone());
    assert!(
        insert_work(guard.runtime(), &first, &first_tool)
            .await
            .is_err()
    );
    delete_work(guard.runtime(), &first_model.work_id).await;
    insert_work(guard.runtime(), &first, &first_tool)
        .await
        .unwrap();
    let mut second_tool = WorkRow::for_state(&second, "waiting_on_tool", 2);
    second_tool.tool = Some(tool);
    assert!(
        insert_work(guard.runtime(), &second, &second_tool)
            .await
            .is_err()
    );
    guard.shutdown().await;
}

fn timestamp(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse_canonical(value).unwrap()
}

fn snapshot(
    work_id: WorkId,
    state: WorkState,
    version: i64,
    owner: Option<RuntimeInstanceId>,
    attempt: CurrentWorkAttempt,
    cancellation: Option<WorkCancellationReason>,
    terminal: Option<WorkTerminalReason>,
) -> WorkLifecycleSnapshot {
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id,
        state,
        projection_version: ProjectionVersion::try_new(version).unwrap(),
        runtime_owner: owner,
        current_attempt: attempt,
        cancellation_reason: cancellation,
        terminal_reason: terminal,
    })
    .unwrap()
}

#[tokio::test]
async fn guarded_conversation_and_work_updates_classify_every_stale_dimension() {
    let (_root, guard) = database().await;
    let fixture = seed_topology(guard.runtime()).await;
    let work_id = WorkId::generate();
    let mut row = WorkRow::for_state(&fixture, "queued", 1);
    row.work_id = work_id.to_string();
    insert_work(guard.runtime(), &fixture, &row).await.unwrap();
    let conversation_id = ConversationId::parse_canonical(&fixture.conversation_id).unwrap();
    let owner = RuntimeInstanceId::parse_canonical(&fixture.runtime_id).unwrap();

    let mut transaction = WriteTransaction::begin(guard.runtime(), "stage6_guard_test")
        .await
        .unwrap();
    let (version, ordinal) = advance_conversation_ordinal(
        &mut transaction,
        conversation_id,
        ProjectionVersion::try_new(1).unwrap(),
        ConversationWorkOrdinal::try_new(1).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(version.get(), 2);
    assert_eq!(ordinal.get(), 2);
    let queued = WorkLifecycleSnapshot::initial(work_id);
    let running = snapshot(
        work_id,
        WorkState::Running,
        2,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    guarded_work_update(
        &mut transaction,
        &queued,
        &running,
        WorkProjectionTimes {
            started_at: Some(timestamp(NOW)),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    let mut transaction = WriteTransaction::begin(guard.runtime(), "stale_conversation")
        .await
        .unwrap();
    assert!(matches!(
        advance_conversation_ordinal(
            &mut transaction,
            conversation_id,
            ProjectionVersion::try_new(1).unwrap(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
        )
        .await
        .unwrap_err(),
        ProjectionMutationError::Conflict(ConversationGuardConflict::StaleVersion)
    ));
    transaction.rollback().await.unwrap();

    let mut transaction = WriteTransaction::begin(guard.runtime(), "stale_ordinal")
        .await
        .unwrap();
    assert!(matches!(
        advance_conversation_ordinal(
            &mut transaction,
            conversation_id,
            ProjectionVersion::try_new(2).unwrap(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
        )
        .await
        .unwrap_err(),
        ProjectionMutationError::Conflict(ConversationGuardConflict::StaleOrdinal)
    ));
    transaction.rollback().await.unwrap();

    let missing_conversation = ConversationId::generate();
    let mut transaction = WriteTransaction::begin(guard.runtime(), "missing_conversation")
        .await
        .unwrap();
    assert!(matches!(
        advance_conversation_ordinal(
            &mut transaction,
            missing_conversation,
            ProjectionVersion::try_new(1).unwrap(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
        )
        .await
        .unwrap_err(),
        ProjectionMutationError::Conflict(ConversationGuardConflict::Missing)
    ));
    transaction.rollback().await.unwrap();

    let model_a = ModelInvocationId::generate();
    let model_b = ModelInvocationId::generate();
    let waiting_model = snapshot(
        work_id,
        WorkState::WaitingOnModel,
        3,
        Some(owner),
        CurrentWorkAttempt::Model(model_a),
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &queued,
        &running,
        WorkGuardConflict::StaleState,
    )
    .await;
    let missing_work_id = WorkId::generate();
    let missing_queued = WorkLifecycleSnapshot::initial(missing_work_id);
    let missing_running = snapshot(
        missing_work_id,
        WorkState::Running,
        2,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &missing_queued,
        &missing_running,
        WorkGuardConflict::Missing,
    )
    .await;
    let stale_version = snapshot(
        work_id,
        WorkState::Running,
        1,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &stale_version,
        &snapshot(
            work_id,
            WorkState::WaitingOnModel,
            2,
            Some(owner),
            CurrentWorkAttempt::Model(model_a),
            None,
            None,
        ),
        WorkGuardConflict::StaleVersion,
    )
    .await;
    let wrong_owner = snapshot(
        work_id,
        WorkState::Running,
        2,
        Some(RuntimeInstanceId::generate()),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &wrong_owner,
        &waiting_model,
        WorkGuardConflict::StaleOwner,
    )
    .await;

    apply_work_update(guard.runtime(), &running, &waiting_model).await;
    let wrong_model = snapshot(
        work_id,
        WorkState::WaitingOnModel,
        3,
        Some(owner),
        CurrentWorkAttempt::Model(model_b),
        None,
        None,
    );
    let resumed = snapshot(
        work_id,
        WorkState::Running,
        4,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &wrong_model,
        &resumed,
        WorkGuardConflict::WrongCurrentModel,
    )
    .await;
    apply_work_update(guard.runtime(), &waiting_model, &resumed).await;

    let tool_a = ToolExecutionId::generate();
    let waiting_tool = snapshot(
        work_id,
        WorkState::WaitingOnTool,
        5,
        Some(owner),
        CurrentWorkAttempt::Tool(tool_a),
        None,
        None,
    );
    apply_work_update(guard.runtime(), &resumed, &waiting_tool).await;
    let wrong_tool = snapshot(
        work_id,
        WorkState::WaitingOnTool,
        5,
        Some(owner),
        CurrentWorkAttempt::Tool(ToolExecutionId::generate()),
        None,
        None,
    );
    let running_six = snapshot(
        work_id,
        WorkState::Running,
        6,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    assert_work_conflict(
        guard.runtime(),
        &wrong_tool,
        &running_six,
        WorkGuardConflict::WrongCurrentTool,
    )
    .await;

    let terminal = snapshot(
        work_id,
        WorkState::Completed,
        6,
        None,
        CurrentWorkAttempt::None,
        None,
        Some(WorkTerminalReason::Completion(
            WorkCompletionReason::Answered,
        )),
    );
    let resurrection = snapshot(
        work_id,
        WorkState::Running,
        7,
        Some(owner),
        CurrentWorkAttempt::None,
        None,
        None,
    );
    let mut transaction = WriteTransaction::begin(guard.runtime(), "terminal_resurrection")
        .await
        .unwrap();
    assert!(matches!(
        guarded_work_update(
            &mut transaction,
            &terminal,
            &resurrection,
            WorkProjectionTimes {
                started_at: Some(timestamp(NOW)),
                cancel_requested_at: None,
                terminal_at: None,
            },
        )
        .await
        .unwrap_err(),
        ProjectionMutationError::Invariant
    ));
    transaction.rollback().await.unwrap();
    guard.shutdown().await;
}

async fn apply_work_update(
    runtime: &SqliteRuntime,
    expected: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
) {
    let mut transaction = WriteTransaction::begin(runtime, "apply_work_update")
        .await
        .unwrap();
    guarded_work_update(
        &mut transaction,
        expected,
        next,
        WorkProjectionTimes {
            started_at: Some(timestamp(NOW)),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

async fn assert_work_conflict(
    runtime: &SqliteRuntime,
    expected: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    conflict: WorkGuardConflict,
) {
    let mut transaction = WriteTransaction::begin(runtime, "assert_work_conflict")
        .await
        .unwrap();
    let error = guarded_work_update(
        &mut transaction,
        expected,
        next,
        WorkProjectionTimes {
            started_at: Some(timestamp(NOW)),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ProjectionMutationError::Conflict(actual) if actual == conflict));
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn every_named_performance_index_supports_its_frozen_query_shape() {
    let (_root, guard) = database().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    let indexes = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_schema WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        indexes,
        PRODUCT_INDEXES
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    );

    let probes = [
        (
            "ux_conversations_craxii_kind",
            "SELECT conversation_id FROM conversations INDEXED BY ux_conversations_craxii_kind WHERE craxii_id = ? AND kind = 'primary'",
        ),
        (
            "ix_workstations_craxii_id",
            "SELECT workstation_id FROM workstations INDEXED BY ix_workstations_craxii_id WHERE craxii_id = ?",
        ),
        (
            "ux_workspaces_workstation_logical_name",
            "SELECT workspace_id FROM workspaces INDEXED BY ux_workspaces_workstation_logical_name WHERE workstation_id = ? AND logical_name = ?",
        ),
        (
            "ix_workspaces_craxii_id",
            "SELECT workspace_id FROM workspaces INDEXED BY ix_workspaces_craxii_id WHERE craxii_id = ?",
        ),
        (
            "ix_runtime_instances_craxii_state",
            "SELECT runtime_instance_id FROM runtime_instances INDEXED BY ix_runtime_instances_craxii_state WHERE craxii_id = ? AND state = 'running'",
        ),
        (
            "ux_client_devices_token_hash",
            "SELECT device_id FROM client_devices INDEXED BY ux_client_devices_token_hash WHERE token_hash = ?",
        ),
        (
            "ux_work_items_conversation_ordinal",
            "SELECT work_id FROM work_items INDEXED BY ux_work_items_conversation_ordinal WHERE conversation_id = ? AND conversation_work_ordinal = ?",
        ),
        (
            "ux_work_items_one_active_per_conversation",
            "SELECT work_id FROM work_items INDEXED BY ux_work_items_one_active_per_conversation WHERE conversation_id = ? AND state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested')",
        ),
        (
            "ix_work_items_queued_fifo",
            "SELECT work_id FROM work_items INDEXED BY ix_work_items_queued_fifo WHERE conversation_id = ? AND state = 'queued' ORDER BY conversation_work_ordinal, work_id LIMIT 1",
        ),
        (
            "ix_work_items_nonterminal_by_runtime",
            "SELECT work_id FROM work_items INDEXED BY ix_work_items_nonterminal_by_runtime WHERE runtime_instance_id = ? AND state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested')",
        ),
        (
            "ux_work_items_current_model_invocation",
            "SELECT work_id FROM work_items INDEXED BY ux_work_items_current_model_invocation WHERE current_model_invocation_id = ?",
        ),
        (
            "ux_work_items_current_tool_execution",
            "SELECT work_id FROM work_items INDEXED BY ux_work_items_current_tool_execution WHERE current_tool_execution_id = ?",
        ),
        (
            "ix_messages_conversation",
            "SELECT message_id FROM messages INDEXED BY ix_messages_conversation WHERE conversation_id = ?",
        ),
        (
            "ux_messages_client_identity",
            "SELECT message_id FROM messages INDEXED BY ux_messages_client_identity WHERE client_device_id = ? AND client_message_id = ?",
        ),
        (
            "ux_messages_produced_by_work",
            "SELECT message_id FROM messages INDEXED BY ux_messages_produced_by_work WHERE produced_by_work_id = ?",
        ),
    ];
    for (index, query) in probes {
        let statement = format!("EXPLAIN QUERY PLAN {query}");
        let rows = sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind("01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d")
            .bind("logical")
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

async fn mutate_then_reject(statements: &[&str]) -> SqliteFailureKind {
    let (root, guard) = database().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe((*statement).to_owned()))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    drop(connection);
    guard.shutdown().await;
    SqliteRuntimeGuard::start(root.path(), 1)
        .await
        .unwrap_err()
        .kind()
}

#[tokio::test]
async fn current_schema_drift_mutations_all_fail_closed() {
    let cases: &[&[&str]] = &[
        &["DROP TABLE messages"],
        &["CREATE TABLE unexpected_product_object (id INTEGER PRIMARY KEY) STRICT, WITHOUT ROWID"],
        &["DROP INDEX ix_messages_conversation"],
        &[
            "DROP INDEX ux_work_items_one_active_per_conversation",
            "CREATE UNIQUE INDEX ux_work_items_one_active_per_conversation ON work_items (conversation_id) WHERE state IN ('running','waiting_on_model')",
        ],
        &["CREATE TRIGGER unexpected_trigger AFTER INSERT ON client_devices BEGIN SELECT 1; END"],
        &["CREATE VIEW unexpected_view AS SELECT device_id FROM client_devices"],
        &["PRAGMA user_version = 1"],
        &["UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1"],
        &["UPDATE _sqlx_migrations SET description = 'wrong' WHERE version = 1"],
        &["UPDATE _sqlx_migrations SET success = 0 WHERE version = 1"],
        &["UPDATE _sqlx_migrations SET checksum = 'malformed' WHERE version = 1"],
        &["UPDATE _sqlx_migrations SET installed_on = X'00' WHERE version = 1"],
        &["UPDATE _sqlx_migrations SET execution_time = -1 WHERE version = 1"],
        &["DELETE FROM _sqlx_migrations WHERE version = 1"],
    ];
    for statements in cases {
        assert_eq!(
            mutate_then_reject(statements).await,
            SqliteFailureKind::InconsistentSchema,
            "{statements:?}"
        );
    }

    for replacement in [
        "CREATE TABLE client_commands (device_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, command_type TEXT NOT NULL, request_hash TEXT NOT NULL, response_http_status INTEGER NOT NULL DEFAULT 200, response_json TEXT NOT NULL, committed_cursor INTEGER NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (device_id, idempotency_key), FOREIGN KEY (device_id) REFERENCES client_devices(device_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID",
        "CREATE TABLE client_commands (device_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, command_type TEXT NOT NULL, request_hash TEXT NOT NULL, response_http_status INTEGER NOT NULL, response_json TEXT NOT NULL, committed_cursor INTEGER NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (device_id, idempotency_key), FOREIGN KEY (device_id) REFERENCES client_devices(device_id) ON UPDATE RESTRICT ON DELETE CASCADE) STRICT, WITHOUT ROWID",
        "CREATE TABLE client_commands (device_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, command_type TEXT NOT NULL, request_hash TEXT NOT NULL, response_http_status INTEGER NOT NULL, response_json TEXT NOT NULL, committed_cursor INTEGER NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (device_id, idempotency_key), FOREIGN KEY (device_id) REFERENCES client_devices(device_id) ON UPDATE RESTRICT ON DELETE RESTRICT) WITHOUT ROWID",
        "CREATE TABLE client_commands (device_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, command_type TEXT NOT NULL, request_hash TEXT NOT NULL, response_http_status INTEGER NOT NULL, response_json TEXT NOT NULL, committed_cursor INTEGER NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (device_id, idempotency_key), FOREIGN KEY (device_id) REFERENCES client_devices(device_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT",
    ] {
        let (root, guard) = database().await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("DROP TABLE client_commands")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query(sqlx::AssertSqlSafe(replacement.to_owned()))
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        guard.shutdown().await;
        assert_eq!(
            SqliteRuntimeGuard::start(root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );
    }
}

#[tokio::test]
async fn valid_contiguous_newer_metadata_is_newer_schema_not_drift() {
    let (root, guard) = database().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations \
         (version, description, success, checksum, execution_time) \
         VALUES (4, 'future migration', 1, zeroblob(48), 0)",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    guard.shutdown().await;
    assert_eq!(
        SqliteRuntimeGuard::start(root.path(), 1)
            .await
            .unwrap_err()
            .kind(),
        SqliteFailureKind::NewerSchema
    );
}
