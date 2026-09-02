use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::Row;

use crate::adapters::artifacts::LocalArtifactStore;
use crate::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
use crate::application::agent_loop::{AgentLoop, AgentLoopLimits, AgentLoopRuntimeContext};
use crate::application::authority::{
    AuthorityEvaluator, V0AuthorityConstraints, V0AuthorityEvaluator,
};
use crate::application::command_service::{
    AcceptMessageCommand, CancelWorkCommand, CommandService,
};
use crate::application::context_assembler::{
    ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot,
};
use crate::application::device_provisioning::DeviceProvisioningService;
use crate::application::model_gateway::{ModelGateway, ModelGatewayLimits, NoopDraftSink};
use crate::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
use crate::application::runtime::{HeartbeatTask, RuntimeControlError, ShutdownController};
use crate::application::runtime::{RuntimeBootstrapReceipt, bootstrap_runtime};
use crate::application::scheduler::{
    SchedulerReadiness, SchedulerStart, WorkCancellation, WorkRunner, WorkRunnerFuture,
    WorkRunnerStartError, start_scheduler,
};
use crate::application::tool_execution_service::{ToolExecutionService, ToolRuntimeLimits};
use crate::application::tool_registry::{ToolRegistry, ToolSemanticPolicy};
use crate::bootstrap::health::{FatalReasonCode, Health, HealthState};
use crate::domain::*;
use crate::ports::artifact_store::ArtifactStore;
use crate::ports::clock::{Clock, TestClock};
use crate::ports::model_provider::{
    ConservativeTokenEstimate, FullJitterSource, ModelProvider, ModelProviderFuture,
    ModelProviderInvocation, ModelProviderStream, ProviderError, ProviderErrorKind,
    ProviderOutcomeCertainty, TokenEstimateUnit, TokenEstimator,
};
use crate::ports::state_store::*;
use crate::ports::workstation::{HARD_FILE_READ_MAX_BYTES, Workstation};
use crate::ports::workstation_preparation::WorkstationPreparation;

use super::{SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2026-08-28T02:00:00.000000Z";
const T1: &str = "2026-08-28T02:00:01.000000Z";
const T2: &str = "2026-08-28T02:00:02.000000Z";
const T3: &str = "2026-08-28T02:00:03.000000Z";
const T4: &str = "2026-08-28T02:00:04.000000Z";
const TOKEN: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-stage10-test-{}-{}",
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
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: SqliteStateStore,
    identity: V0IdentityReference,
    device_id: DeviceId,
}

fn at(value: &str) -> UtcTimestamp {
    value.parse().unwrap()
}

fn observation() -> BootstrapObservation {
    BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(1).unwrap(),
        architecture: "stage10-test-architecture".into(),
        os_release: "stage10-test-os".into(),
        default_shell: "/bin/sh".into(),
        workspace_logical_name: "primary".into(),
        workspace_logical_root: "/workspace".into(),
        workspace_resolved_root: "/tmp/craxii-stage10-workspace".into(),
        execution_capabilities:
            crate::ports::state_store::ExecutionCapabilityObservation::unavailable(),
    }
}

async fn fixture() -> Fixture {
    fixture_with_observation(|_| observation()).await
}

async fn fixture_with_observation(
    build_observation: impl FnOnce(&TestRoot) -> BootstrapObservation,
) -> Fixture {
    let root = TestRoot::new();
    let bootstrap_observation = build_observation(&root);
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let identity = store
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
            created_at: at(T0),
            observation: bootstrap_observation,
        })
        .await
        .unwrap()
        .identity;
    let provisioned = DeviceProvisioningService::new(&store)
        .provision_fixture_token(
            DeviceDisplayName::try_new("Stage 10 device".into()).unwrap(),
            at(T0),
            BearerToken::parse(TOKEN.to_owned()).unwrap(),
        )
        .await
        .unwrap();
    Fixture {
        _root: root,
        guard,
        store,
        identity,
        device_id: provisioned.summary.device_id,
    }
}

fn runtime_evidence(
    identity: V0IdentityReference,
    runtime_instance_id: RuntimeInstanceId,
    started_at: UtcTimestamp,
) -> RuntimeStartEvidence {
    RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
        runtime_instance_id,
        craxii_id: identity.craxii_id,
        workstation_id: identity.workstation_id,
        workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
        linux_boot_id: Some(LinuxBootId::try_new("stage10-test-boot").unwrap()),
        diagnostic_pid: Some(DiagnosticPid::try_new(101).unwrap()),
        package_version: PackageVersion::try_new("0.0.1").unwrap(),
        git_revision: GitRevision::try_new("stage10-test").unwrap(),
        schema_version: SchemaVersion::try_new(4).unwrap(),
        started_at,
    })
}

async fn start_runtime(
    fixture: &Fixture,
    runtime_instance_id: RuntimeInstanceId,
    started_at: UtcTimestamp,
) -> RuntimeBootstrapReceipt {
    let clock = TestClock::new(started_at.to_offset_datetime(), Duration::from_millis(0));
    bootstrap_runtime(
        &fixture.store,
        runtime_evidence(fixture.identity, runtime_instance_id, started_at),
        0,
        &clock,
    )
    .await
    .unwrap()
}

async fn accept(fixture: &Fixture, text: &str, at: UtcTimestamp) -> MessageCommandReceipt {
    let client_message_id =
        ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap();
    CommandService::new(&fixture.store)
        .accept_message(
            AuthenticatedDevice::new(fixture.device_id),
            AcceptMessageCommand {
                idempotency_key: IdempotencyKey::for_message(client_message_id),
                client_message_id,
                conversation_id: fixture.identity.conversation_id,
                content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
                accepted_at: at,
            },
        )
        .await
        .unwrap()
        .into_receipt()
}

async fn cancel(
    fixture: &Fixture,
    work_id: WorkId,
    requested_at: UtcTimestamp,
) -> CancellationCommandReceipt {
    let client_command_id =
        ClientCommandId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap();
    CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            CancelWorkCommand {
                idempotency_key: IdempotencyKey::for_cancellation(client_command_id),
                client_command_id,
                work_id,
                requested_at,
            },
        )
        .await
        .unwrap()
        .into_receipt()
}

async fn claim(
    fixture: &Fixture,
    runtime_id: RuntimeInstanceId,
    claimed_at: UtcTimestamp,
) -> Option<ClaimedWork> {
    fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            conversation_id: fixture.identity.conversation_id,
            runtime_id,
            claimed_at,
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn runtime_start_recovery_heartbeat_stopping_and_stop_are_exact() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let receipt = start_runtime(&fixture, runtime_id, at(T1)).await;
    assert_eq!(receipt.runtime_instance_id, runtime_id);
    assert_eq!(receipt.recovery.stale_runtimes_observed, 0);
    assert_eq!(receipt.recovery.stale_runtimes_closed, 0);
    assert_eq!(receipt.recovery.interrupted_work, 0);
    let repeated_recovery = fixture
        .store
        .append_recovery_summary(AppendRecoverySummaryRequest {
            summary: receipt.recovery.clone(),
            event_id: JournalEventId::generate(),
            started_event_id: receipt.started_event_id,
            correlation_id: receipt.correlation_id,
        })
        .await
        .unwrap();
    assert!(repeated_recovery.events.is_none());

    let unchanged = fixture
        .store
        .heartbeat_runtime(HeartbeatRuntimeRequest {
            runtime_instance_id: runtime_id,
            observed_at: at(T0),
        })
        .await
        .unwrap();
    assert!(!unchanged.advanced);
    assert_eq!(unchanged.persisted_at, at(T1));
    let advanced = fixture
        .store
        .heartbeat_runtime(HeartbeatRuntimeRequest {
            runtime_instance_id: runtime_id,
            observed_at: at(T2),
        })
        .await
        .unwrap();
    assert!(advanced.advanced);
    assert_eq!(advanced.persisted_at, at(T2));

    let stopping = RuntimeStoppingV1 {
        runtime_instance_id: runtime_id,
        shutdown_requested_at: at(T3),
        shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
        grace_deadline: at(T4),
        active_work_count: 0,
        active_task_count: 0,
    };
    let first = fixture
        .store
        .begin_runtime_stopping(BeginRuntimeStoppingRequest {
            event: stopping.clone(),
            event_id: JournalEventId::generate(),
            correlation_id: receipt.correlation_id,
        })
        .await
        .unwrap();
    assert!(first.began);
    let repeated = fixture
        .store
        .begin_runtime_stopping(BeginRuntimeStoppingRequest {
            event: stopping,
            event_id: JournalEventId::generate(),
            correlation_id: receipt.correlation_id,
        })
        .await
        .unwrap();
    assert!(!repeated.began);
    assert!(repeated.commit.events.is_none());
    fixture
        .store
        .finish_runtime_graceful(FinishRuntimeRequest {
            runtime_instance_id: runtime_id,
            stopped_at: at(T4),
        })
        .await
        .unwrap();

    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let row: (String, String, String, i64, i64, i64) = sqlx::query_as(
        "SELECT state, stopped_at, stop_reason, \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'runtime.started'), \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'runtime.recovery_performed'), \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'runtime.stopping') \
         FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(runtime_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "stopped".into(),
            T4.into(),
            "graceful_shutdown".into(),
            1,
            1,
            1
        )
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
async fn runtime_started_row_and_rehashed_event_detail_corruption_fail_startup_consistency() {
    for corrupt_event in [false, true] {
        let fixture = fixture().await;
        let runtime_id = RuntimeInstanceId::generate();
        start_runtime(&fixture, runtime_id, at(T1)).await;
        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        let corrupted_evidence = "0.0.2";
        if corrupt_event {
            let payload: String = sqlx::query_scalar(
                "SELECT payload_json FROM journal_events WHERE event_type = 'runtime.started'",
            )
            .fetch_one(&mut *connection)
            .await
            .unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
            value["binary_version"] = corrupted_evidence.into();
            let reencoded = serde_json::to_string(&value).unwrap();
            let digest = Sha256Digest::hash_bytes(reencoded.as_bytes());
            sqlx::query(
                "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? \
                 WHERE event_type = 'runtime.started'",
            )
            .bind(reencoded)
            .bind(digest.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
        } else {
            sqlx::query(
                "UPDATE runtime_instances SET binary_version = ? WHERE runtime_instance_id = ?",
            )
            .bind(corrupted_evidence)
            .bind(runtime_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
        }
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
}

#[tokio::test]
async fn fifo_active_exclusion_terminal_skip_and_both_claim_cancel_orderings_are_durable() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    start_runtime(&fixture, runtime_id, at(T1)).await;
    let first = accept(&fixture, "first", at(T1)).await;
    let second = accept(&fixture, "second", at(T1)).await;
    let third = accept(&fixture, "third", at(T1)).await;

    let claimed_first = claim(&fixture, runtime_id, at(T2)).await.unwrap();
    assert_eq!(claimed_first.work.work_id(), first.work_id);
    assert_eq!(claimed_first.work.priority(), 0);
    assert!(claim(&fixture, runtime_id, at(T2)).await.is_none());
    fixture
        .store
        .interrupt_abnormal_runner(InterruptOwnedWorkRequest {
            work_id: first.work_id,
            runtime_id,
            interrupted_at: at(T3),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap();
    let claimed_second = claim(&fixture, runtime_id, at(T3)).await.unwrap();
    assert_eq!(claimed_second.work.work_id(), second.work_id);

    let active_cancel = cancel(&fixture, second.work_id, at(T3)).await;
    assert_eq!(
        active_cancel.resulting_work_state,
        WorkState::CancelRequested
    );
    assert_eq!(
        fixture
            .store
            .list_current_runtime_cancel_requested(runtime_id)
            .await
            .unwrap()[0]
            .work_id,
        second.work_id
    );
    fixture
        .store
        .finish_cancellation(FinishCancellationRequest {
            work_id: second.work_id,
            runtime_id,
            confirmed_at: at(T4),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap();

    let cancellation_wins = cancel(&fixture, third.work_id, at(T3)).await;
    assert_eq!(cancellation_wins.resulting_work_state, WorkState::Cancelled);
    assert!(claim(&fixture, runtime_id, at(T4)).await.is_none());

    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let states: Vec<(String, String)> =
        sqlx::query_as("SELECT work_id, state FROM work_items ORDER BY conversation_work_ordinal")
            .fetch_all(&mut *connection)
            .await
            .unwrap();
    assert_eq!(
        states,
        vec![
            (first.work_id.to_string(), "interrupted".into()),
            (second.work_id.to_string(), "cancelled".into()),
            (third.work_id.to_string(), "cancelled".into()),
        ]
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
async fn stale_running_work_is_interrupted_once_and_never_automatically_retried() {
    let fixture = fixture().await;
    let stale_runtime = RuntimeInstanceId::generate();
    start_runtime(&fixture, stale_runtime, at(T1)).await;
    let queued = accept(&fixture, "crash after claim", at(T1)).await;
    claim(&fixture, stale_runtime, at(T2)).await.unwrap();

    let current_runtime = RuntimeInstanceId::generate();
    let recovery = start_runtime(&fixture, current_runtime, at(T3)).await;
    assert_eq!(recovery.recovery.stale_runtimes_observed, 1);
    assert_eq!(recovery.recovery.stale_runtimes_closed, 1);
    assert_eq!(recovery.recovery.interrupted_work, 1);
    assert!(claim(&fixture, current_runtime, at(T4)).await.is_none());

    let repeated = fixture
        .store
        .recover_stale_runtime_ownership(RecoverStaleRuntimeRequest {
            stale_runtime_id: stale_runtime,
            current_runtime_id: current_runtime,
            recovered_at: at(T4),
        })
        .await
        .unwrap();
    assert!(!repeated.stale_runtime_closed);
    assert_eq!(repeated.interrupted_work, 0);
    assert!(repeated.commit.events.is_none());

    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let work: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT state, runtime_instance_id, \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.interrupted' AND work_id = ?) \
         FROM work_items WHERE work_id = ?",
    )
    .bind(queued.work_id.to_string())
    .bind(queued.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work, ("interrupted".into(), None, 1));
    let old: (String, String) = sqlx::query_as(
        "SELECT state, stop_reason FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(stale_runtime.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(old, ("stopped".into(), "startup_failure".into()));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn claim_and_recovery_queries_use_the_frozen_v3_indexes() {
    let fixture = fixture().await;
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let claim_plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT * FROM work_items w WHERE w.conversation_id = ? \
         AND w.state = 'queued' AND NOT EXISTS (SELECT 1 FROM work_items active \
         WHERE active.conversation_id = w.conversation_id AND active.state IN \
         ('running','waiting_on_model','waiting_on_tool','cancel_requested')) \
         ORDER BY w.conversation_work_ordinal ASC, w.work_id ASC LIMIT 1",
    )
    .bind(fixture.identity.conversation_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    let claim_detail = claim_plan
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        claim_detail.contains("ix_work_items_queued_fifo"),
        "{claim_detail}"
    );

    for (sql, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT work_id FROM work_items WHERE runtime_instance_id = ? \
             AND state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
             ORDER BY conversation_id, conversation_work_ordinal",
            "ix_work_items_nonterminal_by_runtime",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT model_invocation_id FROM model_invocations \
             WHERE runtime_instance_id = ? AND state IN ('requesting','streaming') ORDER BY work_id",
            "ix_model_invocations_runtime_nonterminal",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT tool_execution_id FROM tool_executions \
             WHERE runtime_instance_id = ? AND state IN ('requested','dispatching') ORDER BY work_id",
            "ix_tool_executions_runtime_nonterminal",
        ),
    ] {
        let detail = sqlx::query(sql)
            .bind(RuntimeInstanceId::generate().to_string())
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(detail.contains(index), "{detail}");
    }
    drop(connection);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn every_process_start_uses_a_fresh_runtime_identity_and_zero_action_summary() {
    let fixture = fixture().await;
    let first = RuntimeInstanceId::generate();
    start_runtime(&fixture, first, at(T1)).await;
    fixture
        .store
        .mark_runtime_startup_failure(FinishRuntimeRequest {
            runtime_instance_id: first,
            stopped_at: at(T2),
        })
        .await
        .unwrap();
    let second = RuntimeInstanceId::generate();
    let receipt = start_runtime(&fixture, second, at(T3)).await;
    assert_ne!(first, second);
    assert_eq!(receipt.recovery.stale_runtimes_observed, 0);
    assert_eq!(receipt.recovery.retained_queued_work, 0);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let summaries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE event_type = 'runtime.recovery_performed'",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(summaries, 2);
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn zero_action_recovery_summary_rejects_rehashed_nonzero_exact_counter() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let receipt = start_runtime(&fixture, runtime_id, at(T1)).await;
    assert_eq!(receipt.recovery.interrupted_work, 0);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();

    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let payload: String = sqlx::query_scalar(
        "SELECT payload_json FROM journal_events WHERE stream_id = ? \
         AND event_type = 'runtime.recovery_performed'",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    value["interrupted_work"] = 1.into();
    let reencoded = serde_json::to_string(&value).unwrap();
    let digest = Sha256Digest::hash_bytes(reencoded.as_bytes());
    sqlx::query(
        "UPDATE journal_events SET payload_json = ?, payload_sha256 = ? \
         WHERE stream_id = ? AND event_type = 'runtime.recovery_performed'",
    )
    .bind(reencoded)
    .bind(digest.to_string())
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
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

#[tokio::test]
async fn atomic_claim_and_cancellation_transactions_serialize_under_contention() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    start_runtime(&fixture, runtime_id, at(T1)).await;
    let queued = accept(&fixture, "race", at(T1)).await;
    let store = Arc::new(fixture.store.clone());
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let claim_store = Arc::clone(&store);
    let claim_barrier = Arc::clone(&barrier);
    let conversation_id = fixture.identity.conversation_id;
    let claim_task = tokio::spawn(async move {
        claim_barrier.wait().await;
        claim_store
            .claim_next_work(ClaimNextWorkRequest {
                conversation_id,
                runtime_id,
                claimed_at: at(T2),
                event_id: JournalEventId::generate(),
            })
            .await
            .unwrap()
    });
    let cancel_store = Arc::clone(&store);
    let cancel_barrier = Arc::clone(&barrier);
    let device_id = fixture.device_id;
    let cancel_task = tokio::spawn(async move {
        let command_id =
            ClientCommandId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string())
                .unwrap();
        cancel_barrier.wait().await;
        CommandService::new(&*cancel_store)
            .cancel_work(
                AuthenticatedDevice::new(device_id),
                CancelWorkCommand {
                    idempotency_key: IdempotencyKey::for_cancellation(command_id),
                    client_command_id: command_id,
                    work_id: queued.work_id,
                    requested_at: at(T2),
                },
            )
            .await
            .unwrap()
            .into_receipt()
    });
    barrier.wait().await;
    let claimed = claim_task.await.unwrap();
    let cancellation = cancel_task.await.unwrap();
    match claimed {
        Some(_) => assert_eq!(
            cancellation.resulting_work_state,
            WorkState::CancelRequested
        ),
        None => assert_eq!(cancellation.resulting_work_state, WorkState::Cancelled),
    }
    let mut connection = store.runtime.acquire().await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM work_items WHERE work_id = ?")
        .bind(queued.work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert!(matches!(state.as_str(), "cancel_requested" | "cancelled"));
    drop(connection);
    fixture.guard.shutdown().await;
}

struct StubbornRunner {
    started: Arc<tokio::sync::Notify>,
    aborted: Arc<AtomicBool>,
}

struct FaultInjectingStore {
    inner: SqliteStateStore,
    fail_heartbeat: AtomicBool,
    fail_claim: AtomicBool,
    heartbeat_failures: AtomicUsize,
}

impl FaultInjectingStore {
    fn new(inner: SqliteStateStore) -> Self {
        Self {
            inner,
            fail_heartbeat: AtomicBool::new(false),
            fail_claim: AtomicBool::new(false),
            heartbeat_failures: AtomicUsize::new(0),
        }
    }

    fn injected_failure<T>() -> StateStoreFuture<'static, T> {
        Box::pin(async { Err(StateStoreError::new(StateStoreErrorKind::InternalInvariant)) })
    }
}

impl SchedulerStateStore for FaultInjectingStore {
    fn claim_next_work(
        &self,
        request: ClaimNextWorkRequest,
    ) -> StateStoreFuture<'_, Option<ClaimedWork>> {
        if self.fail_claim.load(Ordering::SeqCst) {
            return Self::injected_failure();
        }
        self.inner.claim_next_work(request)
    }

    fn list_current_runtime_cancel_requested(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> StateStoreFuture<'_, Vec<CancelRequestedWork>> {
        self.inner.list_current_runtime_cancel_requested(runtime_id)
    }

    fn finish_cancellation(
        &self,
        request: FinishCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.inner.finish_cancellation(request)
    }

    fn interrupt_abnormal_runner(
        &self,
        request: InterruptOwnedWorkRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.inner.interrupt_abnormal_runner(request)
    }

    fn request_owned_work_cancellation(
        &self,
        request: RequestOwnedCancellationRequest,
    ) -> StateStoreFuture<'_, Vec<WorkId>> {
        self.inner.request_owned_work_cancellation(request)
    }
}

impl RuntimeStateStore for FaultInjectingStore {
    fn create_runtime_and_started_event(
        &self,
        request: CreateRuntimeRequest,
    ) -> StateStoreFuture<'_, CreateRuntimeReceipt> {
        self.inner.create_runtime_and_started_event(request)
    }

    fn heartbeat_runtime(
        &self,
        request: HeartbeatRuntimeRequest,
    ) -> StateStoreFuture<'_, HeartbeatRuntimeReceipt> {
        if self.fail_heartbeat.load(Ordering::SeqCst) {
            self.heartbeat_failures.fetch_add(1, Ordering::SeqCst);
            return Self::injected_failure();
        }
        self.inner.heartbeat_runtime(request)
    }

    fn begin_runtime_stopping(
        &self,
        request: BeginRuntimeStoppingRequest,
    ) -> StateStoreFuture<'_, BeginRuntimeStoppingReceipt> {
        self.inner.begin_runtime_stopping(request)
    }

    fn finish_runtime_graceful(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.inner.finish_runtime_graceful(request)
    }

    fn mark_runtime_startup_failure(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.inner.mark_runtime_startup_failure(request)
    }

    fn enumerate_stale_runtimes(
        &self,
        request: EnumerateStaleRuntimesRequest,
    ) -> StateStoreFuture<'_, Vec<RuntimeInstanceId>> {
        self.inner.enumerate_stale_runtimes(request)
    }

    fn append_recovery_summary(
        &self,
        request: AppendRecoverySummaryRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.inner.append_recovery_summary(request)
    }
}

impl RecoveryStateStore for FaultInjectingStore {
    fn count_retained_queued_work(&self) -> StateStoreFuture<'_, u64> {
        self.inner.count_retained_queued_work()
    }

    fn recover_stale_runtime_ownership(
        &self,
        request: RecoverStaleRuntimeRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt> {
        self.inner.recover_stale_runtime_ownership(request)
    }

    fn classify_unresolved_shutdown_work(
        &self,
        request: ClassifyShutdownWorkRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt> {
        self.inner.classify_unresolved_shutdown_work(request)
    }
}

struct StubbornRunnerDrop(Arc<AtomicBool>);

impl Drop for StubbornRunnerDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl WorkRunner for StubbornRunner {
    fn start(
        &self,
        _: ClaimedWork,
        _: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let started = Arc::clone(&self.started);
        let aborted = Arc::clone(&self.aborted);
        Ok(Box::pin(async move {
            let _drop_observation = StubbornRunnerDrop(aborted);
            started.notify_one();
            std::future::pending().await
        }))
    }
}

#[tokio::test]
async fn shutdown_latches_deadline_stops_claims_and_classifies_before_timeout_abort() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let bootstrap = start_runtime(&fixture, runtime_id, at(T1)).await;
    let queued = accept(&fixture, "stubborn runner", at(T1)).await;
    let retained = accept(&fixture, "must remain queued", at(T1)).await;
    let store = Arc::new(fixture.store.clone());
    let clock = Arc::new(TestClock::new(
        at(T3).to_offset_datetime(),
        Duration::from_secs(3),
    ));
    let health = Health::new();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let heartbeat = HeartbeatTask::start(
        Arc::clone(&store),
        Arc::clone(&clock),
        health.clone(),
        runtime_id,
        fatal.clone(),
    );
    let started = Arc::new(tokio::sync::Notify::new());
    let aborted = Arc::new(AtomicBool::new(false));
    let scheduler = start_scheduler(
        Arc::clone(&store),
        Arc::new(StubbornRunner {
            started: Arc::clone(&started),
            aborted: Arc::clone(&aborted),
        }),
        Arc::clone(&clock),
        health.clone(),
        fatal,
        SchedulerStart {
            runtime_instance_id: runtime_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: crate::application::scheduler::SchedulerReadiness::ReadyAfterInitialScan,
        },
    )
    .unwrap();
    started.notified().await;
    let registry = scheduler.registry();
    assert_eq!(registry.snapshot()[0].work_id, queued.work_id);
    let controller = ShutdownController::new(
        Arc::clone(&store),
        Arc::clone(&clock),
        health.clone(),
        runtime_id,
        bootstrap.correlation_id,
        1,
        heartbeat,
    );
    controller.install_scheduler(scheduler).await.unwrap();
    store.verify_application_consistency().await.unwrap();

    let first = controller.request().await.unwrap();
    let repeated = controller.request().await.unwrap();
    assert!(first.began);
    assert!(!repeated.began);
    assert_eq!(first.shutdown_requested_at, repeated.shutdown_requested_at);
    assert_eq!(first.grace_deadline, repeated.grace_deadline);
    assert_eq!(health.snapshot().state(), HealthState::Draining);
    {
        let mut connection = store.runtime.acquire().await.unwrap();
        let rows = sqlx::query("SELECT * FROM journal_events ORDER BY journal_offset")
            .fetch_all(&mut *connection)
            .await
            .unwrap();
        let events = rows
            .iter()
            .map(super::journal::decode_event_row)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let projected = crate::application::projector::project(&events).unwrap();
        super::stage8::verify_stage8_consistency(&mut connection, &projected, &events)
            .await
            .unwrap();
        super::stage9::verify_stage9_consistency(&mut connection, &events)
            .await
            .unwrap();
        super::stage10::verify_stage10_consistency(&mut connection, &projected, &events)
            .await
            .unwrap();
    }
    store.verify_application_consistency().await.unwrap();
    controller.finish().await.unwrap();
    assert!(aborted.load(Ordering::SeqCst));
    assert!(registry.snapshot().is_empty());

    let mut connection = store.runtime.acquire().await.unwrap();
    let runtime: (String, String, i64) = sqlx::query_as(
        "SELECT state, stop_reason, (SELECT COUNT(*) FROM journal_events \
         WHERE event_type = 'runtime.stopping' AND stream_id = ?) \
         FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .bind(runtime_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(runtime, ("stopped".into(), "graceful_shutdown".into(), 1));
    let work: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT state, runtime_instance_id, (SELECT COUNT(*) FROM journal_events \
         WHERE event_type = 'work.cancelled' AND work_id = ?) \
         FROM work_items WHERE work_id = ?",
    )
    .bind(queued.work_id.to_string())
    .bind(queued.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work, ("cancelled".into(), None, 1));
    let retained_state: String =
        sqlx::query_scalar("SELECT state FROM work_items WHERE work_id = ?")
            .bind(retained.work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
    assert_eq!(retained_state, "queued");
    let ordered_offsets: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.started'), \
         (SELECT MAX(journal_offset) FROM journal_events WHERE event_type = 'work.started'), \
         (SELECT journal_offset FROM journal_events WHERE event_type = 'runtime.stopping' \
          AND stream_id = ?)",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(ordered_offsets.0, 1);
    assert!(ordered_offsets.1 < ordered_offsets.2);
    drop(connection);
    store.verify_application_consistency().await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn fatal_health_remains_terminal_while_controlled_shutdown_completes() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let bootstrap = start_runtime(&fixture, runtime_id, at(T1)).await;
    let store = Arc::new(fixture.store.clone());
    let clock = Arc::new(TestClock::new(at(T3).to_offset_datetime(), Duration::ZERO));
    let health = Health::new();
    health.mark_fatal(FatalReasonCode::Internal).unwrap();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let heartbeat = HeartbeatTask::start(
        Arc::clone(&store),
        Arc::clone(&clock),
        health.clone(),
        runtime_id,
        fatal.clone(),
    );
    let scheduler = start_scheduler(
        Arc::clone(&store),
        Arc::new(StubbornRunner {
            started: Arc::new(tokio::sync::Notify::new()),
            aborted: Arc::new(AtomicBool::new(false)),
        }),
        Arc::clone(&clock),
        health.clone(),
        fatal,
        SchedulerStart {
            runtime_instance_id: runtime_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: crate::application::scheduler::SchedulerReadiness::RemainLiveUnready,
        },
    )
    .unwrap();
    let controller = ShutdownController::new(
        Arc::clone(&store),
        clock,
        health.clone(),
        runtime_id,
        bootstrap.correlation_id,
        10,
        heartbeat,
    );
    controller.install_scheduler(scheduler).await.unwrap();

    controller.request().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    controller.finish().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Fatal);

    let mut connection = store.runtime.acquire().await.unwrap();
    let evidence: (String, String, i64) = sqlx::query_as(
        "SELECT state, stop_reason, (SELECT COUNT(*) FROM journal_events \
         WHERE stream_id = ? AND event_type = 'runtime.stopping') \
         FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .bind(runtime_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(evidence, ("stopped".into(), "graceful_shutdown".into(), 1));
    drop(connection);
    store.verify_application_consistency().await.unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn heartbeat_fatal_error_still_runs_shutdown_and_preserves_original_failure() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let bootstrap = start_runtime(&fixture, runtime_id, at(T1)).await;
    let queued = accept(&fixture, "heartbeat fatal runner", at(T1)).await;
    let store = Arc::new(FaultInjectingStore::new(fixture.store.clone()));
    let clock = Arc::new(TestClock::new(at(T3).to_offset_datetime(), Duration::ZERO));
    let health = Health::new();
    let (fatal, mut observed_fatal) = tokio::sync::watch::channel(false);
    let started = Arc::new(tokio::sync::Notify::new());
    let aborted = Arc::new(AtomicBool::new(false));
    let scheduler = start_scheduler(
        Arc::clone(&store),
        Arc::new(StubbornRunner {
            started: Arc::clone(&started),
            aborted: Arc::clone(&aborted),
        }),
        Arc::clone(&clock),
        health.clone(),
        fatal.clone(),
        SchedulerStart {
            runtime_instance_id: runtime_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: crate::application::scheduler::SchedulerReadiness::ReadyAfterInitialScan,
        },
    )
    .unwrap();
    started.notified().await;
    let registry = scheduler.registry();
    store.fail_heartbeat.store(true, Ordering::SeqCst);
    let heartbeat = HeartbeatTask::start_with_cadence(
        Arc::clone(&store),
        Arc::clone(&clock),
        health.clone(),
        runtime_id,
        fatal,
        Duration::from_millis(1),
    );
    let controller = ShutdownController::new(
        Arc::clone(&store),
        clock,
        health.clone(),
        runtime_id,
        bootstrap.correlation_id,
        1,
        heartbeat,
    );
    controller.install_scheduler(scheduler).await.unwrap();

    observed_fatal.changed().await.unwrap();
    assert!(*observed_fatal.borrow());
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    controller.request().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    assert_eq!(
        controller.finish().await,
        Err(RuntimeControlError::StateStore)
    );

    assert_eq!(store.heartbeat_failures.load(Ordering::SeqCst), 1);
    assert!(aborted.load(Ordering::SeqCst));
    assert!(registry.snapshot().is_empty());
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let evidence: (String, String, String, i64) = sqlx::query_as(
        "SELECT r.state, r.stop_reason, w.state, \
         (SELECT COUNT(*) FROM journal_events WHERE stream_id = ? \
          AND event_type = 'runtime.stopping') \
         FROM runtime_instances r JOIN work_items w ON w.work_id = ? \
         WHERE r.runtime_instance_id = ?",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .bind(queued.work_id.to_string())
    .bind(runtime_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        evidence,
        (
            "stopped".into(),
            "graceful_shutdown".into(),
            "cancelled".into(),
            1
        )
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
async fn scheduler_fatal_error_retains_ownership_until_controlled_shutdown() {
    let fixture = fixture().await;
    let runtime_id = RuntimeInstanceId::generate();
    let bootstrap = start_runtime(&fixture, runtime_id, at(T1)).await;
    let store = Arc::new(FaultInjectingStore::new(fixture.store.clone()));
    store.fail_claim.store(true, Ordering::SeqCst);
    let clock = Arc::new(TestClock::new(at(T3).to_offset_datetime(), Duration::ZERO));
    let health = Health::new();
    let (fatal, mut observed_fatal) = tokio::sync::watch::channel(false);
    let heartbeat = HeartbeatTask::start(
        Arc::clone(&store),
        Arc::clone(&clock),
        health.clone(),
        runtime_id,
        fatal.clone(),
    );
    let scheduler = start_scheduler(
        Arc::clone(&store),
        Arc::new(StubbornRunner {
            started: Arc::new(tokio::sync::Notify::new()),
            aborted: Arc::new(AtomicBool::new(false)),
        }),
        Arc::clone(&clock),
        health.clone(),
        fatal,
        SchedulerStart {
            runtime_instance_id: runtime_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: crate::application::scheduler::SchedulerReadiness::RemainLiveUnready,
        },
    )
    .unwrap();
    let controller = ShutdownController::new(
        Arc::clone(&store),
        clock,
        health.clone(),
        runtime_id,
        bootstrap.correlation_id,
        10,
        heartbeat,
    );
    controller.install_scheduler(scheduler).await.unwrap();

    observed_fatal.changed().await.unwrap();
    assert_eq!(health.snapshot().state(), HealthState::Fatal);
    controller.request().await.unwrap();
    assert_eq!(
        controller.finish().await,
        Err(RuntimeControlError::TaskJoin)
    );
    assert_eq!(health.snapshot().state(), HealthState::Fatal);

    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let evidence: (String, String, i64) = sqlx::query_as(
        "SELECT state, stop_reason, (SELECT COUNT(*) FROM journal_events \
         WHERE stream_id = ? AND event_type = 'runtime.stopping') \
         FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(JournalStreamId::Runtime(runtime_id).to_string())
    .bind(runtime_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(evidence, ("stopped".into(), "graceful_shutdown".into(), 1));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
const CRASH_MESSAGE_ID: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";
#[cfg(feature = "test-failpoints")]
const CRASH_COMMAND_ID: &str = "01890f6c-7b3a-7cc0-a8f1-2e6f7a8b9c0d";

#[cfg(feature = "test-failpoints")]
struct ProcessFixture {
    guard: SqliteRuntimeGuard,
    store: SqliteStateStore,
    identity: V0IdentityReference,
    device_id: DeviceId,
}

#[cfg(feature = "test-failpoints")]
async fn process_fixture(root: &Path) -> ProcessFixture {
    let guard = SqliteRuntimeGuard::start(root, 4).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let identity = store
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
            created_at: at(T0),
            observation: observation(),
        })
        .await
        .unwrap()
        .identity;
    let service = DeviceProvisioningService::new(&store);
    let devices = service.list().await.unwrap();
    let device_id = if let Some(device) = devices.first() {
        device.device_id
    } else {
        service
            .provision_fixture_token(
                DeviceDisplayName::try_new("Stage 10 crash device".into()).unwrap(),
                at(T0),
                BearerToken::parse(TOKEN.to_owned()).unwrap(),
            )
            .await
            .unwrap()
            .summary
            .device_id
    };
    ProcessFixture {
        guard,
        store,
        identity,
        device_id,
    }
}

#[cfg(feature = "test-failpoints")]
async fn process_start_runtime(
    fixture: &ProcessFixture,
    started_at: UtcTimestamp,
) -> RuntimeBootstrapReceipt {
    let clock = TestClock::new(started_at.to_offset_datetime(), Duration::ZERO);
    bootstrap_runtime(
        &fixture.store,
        runtime_evidence(fixture.identity, RuntimeInstanceId::generate(), started_at),
        0,
        &clock,
    )
    .await
    .unwrap()
}

#[cfg(feature = "test-failpoints")]
fn crash_message_command(identity: V0IdentityReference, text: &str) -> AcceptMessageCommand {
    let id = ClientMessageId::parse_canonical(CRASH_MESSAGE_ID).unwrap();
    AcceptMessageCommand {
        idempotency_key: IdempotencyKey::for_message(id),
        client_message_id: id,
        conversation_id: identity.conversation_id,
        content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
        accepted_at: at(T2),
    }
}

#[cfg(feature = "test-failpoints")]
fn crash_cancel_command(work_id: WorkId) -> CancelWorkCommand {
    let id = ClientCommandId::parse_canonical(CRASH_COMMAND_ID).unwrap();
    CancelWorkCommand {
        idempotency_key: IdempotencyKey::for_cancellation(id),
        client_command_id: id,
        work_id,
        requested_at: at(T3),
    }
}

#[cfg(feature = "test-failpoints")]
struct PendingRunner;

#[cfg(feature = "test-failpoints")]
impl WorkRunner for PendingRunner {
    fn start(
        &self,
        _: ClaimedWork,
        _: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        Ok(Box::pin(std::future::pending()))
    }
}

#[cfg(feature = "test-failpoints")]
struct ObservedAbnormalRunner {
    started: Arc<tokio::sync::Notify>,
}

#[cfg(feature = "test-failpoints")]
impl WorkRunner for ObservedAbnormalRunner {
    fn start(
        &self,
        _: ClaimedWork,
        _: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let started = Arc::clone(&self.started);
        Ok(Box::pin(async move {
            started.notify_one();
            crate::application::scheduler::WorkRunnerExit::Abnormal
        }))
    }
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn stage10_failpoint_crash_child() {
    let Ok(scenario) = std::env::var("CRAXII_STAGE10_CRASH_SCENARIO") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("CRAXII_STAGE10_CRASH_ROOT").unwrap());
    let fixture = process_fixture(&root).await;
    let runtime = process_start_runtime(&fixture, at(T1)).await;
    match scenario.as_str() {
        "message" => {
            let _ = CommandService::new(&fixture.store)
                .accept_message(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_message_command(fixture.identity, "message crash"),
                )
                .await;
        }
        "claim" => {
            CommandService::new(&fixture.store)
                .accept_message(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_message_command(fixture.identity, "claim crash"),
                )
                .await
                .unwrap();
            let health = Health::new();
            let (fatal, _) = tokio::sync::watch::channel(false);
            let _scheduler = start_scheduler(
                Arc::new(fixture.store.clone()),
                Arc::new(PendingRunner),
                Arc::new(TestClock::new(at(T2).to_offset_datetime(), Duration::ZERO)),
                health,
                fatal,
                SchedulerStart {
                    runtime_instance_id: runtime.runtime_instance_id,
                    conversation_id: fixture.identity.conversation_id,
                    readiness: crate::application::scheduler::SchedulerReadiness::RemainLiveUnready,
                },
            )
            .unwrap();
            std::future::pending::<()>().await;
        }
        "cancel" => {
            let receipt = CommandService::new(&fixture.store)
                .accept_message(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_message_command(fixture.identity, "cancel crash"),
                )
                .await
                .unwrap()
                .into_receipt();
            fixture
                .store
                .claim_next_work(ClaimNextWorkRequest {
                    conversation_id: fixture.identity.conversation_id,
                    runtime_id: runtime.runtime_instance_id,
                    claimed_at: at(T2),
                    event_id: JournalEventId::generate(),
                })
                .await
                .unwrap()
                .unwrap();
            let _ = CommandService::new(&fixture.store)
                .cancel_work(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_cancel_command(receipt.work_id),
                )
                .await;
        }
        "shutdown" => {
            let receipt = CommandService::new(&fixture.store)
                .accept_message(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_message_command(fixture.identity, "shutdown crash"),
                )
                .await
                .unwrap()
                .into_receipt();
            fixture
                .store
                .claim_next_work(ClaimNextWorkRequest {
                    conversation_id: fixture.identity.conversation_id,
                    runtime_id: runtime.runtime_instance_id,
                    claimed_at: at(T2),
                    event_id: JournalEventId::generate(),
                })
                .await
                .unwrap()
                .unwrap();
            let store = Arc::new(fixture.store.clone());
            let clock = Arc::new(TestClock::new(
                at(T3).to_offset_datetime(),
                Duration::from_secs(3),
            ));
            let health = Health::new();
            let (fatal, _) = tokio::sync::watch::channel(false);
            let heartbeat = HeartbeatTask::start(
                Arc::clone(&store),
                Arc::clone(&clock),
                health.clone(),
                runtime.runtime_instance_id,
                fatal,
            );
            let controller = ShutdownController::new(
                store,
                clock,
                health,
                runtime.runtime_instance_id,
                runtime.correlation_id,
                1_000,
                heartbeat,
            );
            let _ = controller.request().await;
            let _ = receipt;
        }
        "recovery" => {
            let receipt = CommandService::new(&fixture.store)
                .accept_message(
                    AuthenticatedDevice::new(fixture.device_id),
                    crash_message_command(fixture.identity, "recovery crash"),
                )
                .await
                .unwrap()
                .into_receipt();
            fixture
                .store
                .claim_next_work(ClaimNextWorkRequest {
                    conversation_id: fixture.identity.conversation_id,
                    runtime_id: runtime.runtime_instance_id,
                    claimed_at: at(T2),
                    event_id: JournalEventId::generate(),
                })
                .await
                .unwrap()
                .unwrap();
            let recovering_runtime = RuntimeInstanceId::generate();
            fixture
                .store
                .create_runtime_and_started_event(CreateRuntimeRequest {
                    evidence: runtime_evidence(fixture.identity, recovering_runtime, at(T3)),
                    event_id: JournalEventId::generate(),
                    correlation_id: CorrelationId::generate(),
                })
                .await
                .unwrap();
            fixture
                .store
                .recover_stale_runtime_ownership(RecoverStaleRuntimeRequest {
                    stale_runtime_id: runtime.runtime_instance_id,
                    current_runtime_id: recovering_runtime,
                    recovered_at: at(T4),
                })
                .await
                .unwrap();
            let _ = receipt;
            std::process::abort();
        }
        _ => panic!("unknown Stage 10 crash scenario"),
    }
    panic!("Stage 10 failpoint did not abort the child process");
}

#[cfg(feature = "test-failpoints")]
fn run_crash_child(root: &Path, scenario: &str, hook: &str) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adapters::sqlite::stage10_tests::stage10_failpoint_crash_child",
            "--nocapture",
        ])
        .env("CRAXII_STAGE10_CRASH_ROOT", root)
        .env("CRAXII_STAGE10_CRASH_SCENARIO", scenario)
        .env("CRAXII_TEST_ABORT_AT_FAILPOINT", hook)
        .status()
        .unwrap();
    assert!(!status.success(), "failpoint child unexpectedly survived");
}

#[cfg(feature = "test-failpoints")]
async fn crash_root() -> TestRoot {
    TestRoot::new()
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn after_message_commit_process_loss_replays_once_and_scheduler_scan_claims() {
    let root = crash_root().await;
    run_crash_child(root.path(), "message", "after_message_transaction_commit");
    let fixture = process_fixture(root.path()).await;
    let current = process_start_runtime(&fixture, at(T3)).await;
    assert_eq!(current.recovery.retained_queued_work, 1);
    let replay = CommandService::new(&fixture.store)
        .accept_message(
            AuthenticatedDevice::new(fixture.device_id),
            crash_message_command(fixture.identity, "message crash"),
        )
        .await
        .unwrap();
    assert!(replay.is_replay());
    let receipt = replay.into_receipt();
    let started = Arc::new(tokio::sync::Notify::new());
    let health = Health::new();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let scheduler = start_scheduler(
        Arc::new(fixture.store.clone()),
        Arc::new(ObservedAbnormalRunner {
            started: Arc::clone(&started),
        }),
        Arc::new(TestClock::new(at(T4).to_offset_datetime(), Duration::ZERO)),
        health,
        fatal,
        SchedulerStart {
            runtime_instance_id: current.runtime_instance_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: crate::application::scheduler::SchedulerReadiness::RemainLiveUnready,
        },
    )
    .unwrap();
    started.notified().await;
    scheduler.stop_and_join().await.unwrap();
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    for (table, expected) in [
        ("client_commands", 1_i64),
        ("messages", 1),
        ("work_items", 1),
    ] {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&mut *connection)
                .await
                .unwrap();
        assert_eq!(count, expected);
    }
    let started_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type = 'work.started'",
    )
    .bind(receipt.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(started_events, 1);
    drop(connection);
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn after_claim_commit_process_loss_is_recovered_without_runner_retry() {
    let root = crash_root().await;
    run_crash_child(root.path(), "claim", "after_work_claim_commit");
    let fixture = process_fixture(root.path()).await;
    let current = process_start_runtime(&fixture, at(T3)).await;
    assert_eq!(current.recovery.stale_runtimes_observed, 1);
    assert_eq!(current.recovery.interrupted_work, 1);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let work: (String, i64) = sqlx::query_as(
        "SELECT state, (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.started') \
         FROM work_items",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work, ("interrupted".into(), 1));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn after_cancel_requested_process_loss_replays_and_recovery_converges_once() {
    let root = crash_root().await;
    run_crash_child(root.path(), "cancel", "after_cancel_requested_commit");
    let fixture = process_fixture(root.path()).await;
    let current = process_start_runtime(&fixture, at(T4)).await;
    assert_eq!(current.recovery.stale_runtimes_observed, 1);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let work_id: WorkId = sqlx::query_scalar::<_, String>("SELECT work_id FROM work_items")
        .fetch_one(&mut *connection)
        .await
        .unwrap()
        .parse()
        .unwrap();
    drop(connection);
    let replay = CommandService::new(&fixture.store)
        .cancel_work(
            AuthenticatedDevice::new(fixture.device_id),
            crash_cancel_command(work_id),
        )
        .await
        .unwrap();
    assert!(replay.is_replay());
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let row: (String, i64, i64) = sqlx::query_as(
        "SELECT state, \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancel_requested'), \
         (SELECT COUNT(*) FROM journal_events WHERE event_type = 'work.cancelled') \
         FROM work_items",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(row, ("cancelled".into(), 1, 1));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn graceful_shutdown_process_loss_recovers_stopping_runtime_without_duplicate_event() {
    let root = crash_root().await;
    run_crash_child(root.path(), "shutdown", "during_graceful_shutdown");
    let fixture = process_fixture(root.path()).await;
    let current = process_start_runtime(&fixture, at(T4)).await;
    assert_eq!(current.recovery.stale_runtimes_observed, 1);
    assert_eq!(current.recovery.interrupted_work, 1);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let old: (String, String, i64) = sqlx::query_as(
        "SELECT state, stop_reason, (SELECT COUNT(*) FROM journal_events \
         WHERE event_type = 'runtime.stopping') FROM runtime_instances \
         WHERE runtime_instance_id <> ?",
    )
    .bind(current.runtime_instance_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(old, ("stopped".into(), "startup_failure".into(), 1));
    let current_events: (i64, i64) = sqlx::query_as(
        "SELECT \
         (SELECT COUNT(*) FROM journal_events WHERE stream_id = ? AND event_type = 'runtime.started'), \
         (SELECT COUNT(*) FROM journal_events WHERE stream_id = ? AND event_type = 'runtime.recovery_performed')",
    )
    .bind(JournalStreamId::Runtime(current.runtime_instance_id).to_string())
    .bind(JournalStreamId::Runtime(current.runtime_instance_id).to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(current_events, (1, 1));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[cfg(feature = "test-failpoints")]
#[tokio::test]
async fn process_loss_between_recovery_units_is_idempotent_on_the_next_startup() {
    let root = crash_root().await;
    run_crash_child(root.path(), "recovery", "none");
    let fixture = process_fixture(root.path()).await;
    let current = process_start_runtime(&fixture, at(T4)).await;
    assert_eq!(current.recovery.stale_runtimes_observed, 1);
    assert_eq!(current.recovery.stale_runtimes_closed, 1);
    assert_eq!(current.recovery.interrupted_work, 0);
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let work: (String, i64) = sqlx::query_as(
        "SELECT state, (SELECT COUNT(*) FROM journal_events \
         WHERE event_type = 'work.interrupted') FROM work_items",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(work, ("interrupted".into(), 1));
    let runtime_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT state, COALESCE(stop_reason, '') FROM runtime_instances \
         WHERE runtime_instance_id <> ? ORDER BY started_at, runtime_instance_id",
    )
    .bind(current.runtime_instance_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        runtime_rows,
        vec![
            ("stopped".into(), "startup_failure".into()),
            ("stopped".into(), "startup_failure".into()),
        ]
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

struct Stage17FixedEstimator {
    identity: TokenEstimatorIdentity,
}

impl TokenEstimator for Stage17FixedEstimator {
    fn identity(&self) -> &TokenEstimatorIdentity {
        &self.identity
    }

    fn estimate(
        &self,
        _: &ModelTarget,
        _: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError> {
        ConservativeTokenEstimate::try_new(self.identity.clone(), 1_024)
    }
}

struct Stage17MinimumJitter;

impl FullJitterSource for Stage17MinimumJitter {
    fn sample_inclusive(&mut self, _: u64) -> u64 {
        0
    }
}

enum Stage17ProviderPlan {
    ReadFile,
    ReadFiles,
    FinalText(&'static str),
    Structured,
    Refusal(&'static str),
}

struct Stage17Provider {
    provider_id: ProviderId,
    plans: Mutex<VecDeque<Stage17ProviderPlan>>,
    requests: Mutex<Vec<ModelRequest>>,
    first_started: tokio::sync::Notify,
    release_first: tokio::sync::Notify,
    invocation_count: AtomicUsize,
}

impl Stage17Provider {
    fn new(plans: Vec<Stage17ProviderPlan>) -> Self {
        Self {
            provider_id: ProviderId::try_new("stage17-fixture").unwrap(),
            plans: Mutex::new(plans.into()),
            requests: Mutex::new(Vec::new()),
            first_started: tokio::sync::Notify::new(),
            release_first: tokio::sync::Notify::new(),
            invocation_count: AtomicUsize::new(0),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct Stage17ProviderStream {
    events: VecDeque<ModelStreamEvent>,
}

impl ModelProviderStream for Stage17ProviderStream {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>> {
        Box::pin(async move { Ok(self.events.pop_front()) })
    }
}

impl ModelProvider for Stage17Provider {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        if target.reference().provider_id() != &self.provider_id {
            return Err(ProviderError::new(
                ProviderErrorKind::UnknownModel,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            ));
        }
        Ok(target.reference().capabilities().clone())
    }

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            let ordinal = self.invocation_count.fetch_add(1, Ordering::SeqCst) + 1;
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(invocation.request.clone());
            let plan = self
                .plans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorKind::ScriptMismatch,
                        ProviderOutcomeCertainty::DefinitelyNotSent,
                    )
                })?;
            if ordinal == 1 {
                self.first_started.notify_one();
                self.release_first.notified().await;
            }
            let request_id = ProviderEvidenceId::try_new(format!("req-{ordinal}")).unwrap();
            let response_id = ProviderEvidenceId::try_new(format!("resp-{ordinal}")).unwrap();
            let usage = crate::domain::model::ModelUsage::try_new(10, 0, 5, 0, 15).unwrap();
            let target = invocation.request.target().identity();
            let (items, stop_reason, semantic_events) = match plan {
                Stage17ProviderPlan::ReadFile => {
                    let call = CanonicalModelToolCall::try_new(
                        ModelToolCallId::try_new("read-note").unwrap(),
                        "read_file",
                        "{\"path\":\"note.txt\"}",
                    )
                    .unwrap();
                    (
                        vec![ModelOutputItem::ToolCall(call.clone())],
                        ModelStopReason::ToolContinuation,
                        vec![
                            ModelStreamEvent::ToolCallStarted {
                                item_ordinal: 0,
                                call_id: call.call_id().clone(),
                                name: call.name().clone(),
                            },
                            ModelStreamEvent::ToolArgumentDelta {
                                item_ordinal: 0,
                                call_id: call.call_id().clone(),
                                delta: call.raw_arguments().to_owned(),
                            },
                            ModelStreamEvent::ToolCallCompleted {
                                item_ordinal: 0,
                                call,
                            },
                        ],
                    )
                }
                Stage17ProviderPlan::ReadFiles => {
                    let first = CanonicalModelToolCall::try_new(
                        ModelToolCallId::try_new("read-first-note").unwrap(),
                        "read_file",
                        "{\"path\":\"note.txt\"}",
                    )
                    .unwrap();
                    let second = CanonicalModelToolCall::try_new(
                        ModelToolCallId::try_new("read-second-note").unwrap(),
                        "read_file",
                        "{\"path\":\"note-two.txt\"}",
                    )
                    .unwrap();
                    let missing = CanonicalModelToolCall::try_new(
                        ModelToolCallId::try_new("read-missing-note").unwrap(),
                        "read_file",
                        "{\"path\":\"missing-note.txt\"}",
                    )
                    .unwrap();
                    let mut semantic_events = Vec::new();
                    for (item_ordinal, call) in [
                        (0, first.clone()),
                        (1, second.clone()),
                        (2, missing.clone()),
                    ] {
                        semantic_events.extend([
                            ModelStreamEvent::ToolCallStarted {
                                item_ordinal,
                                call_id: call.call_id().clone(),
                                name: call.name().clone(),
                            },
                            ModelStreamEvent::ToolArgumentDelta {
                                item_ordinal,
                                call_id: call.call_id().clone(),
                                delta: call.raw_arguments().to_owned(),
                            },
                            ModelStreamEvent::ToolCallCompleted { item_ordinal, call },
                        ]);
                    }
                    (
                        vec![
                            ModelOutputItem::ToolCall(first),
                            ModelOutputItem::ToolCall(second),
                            ModelOutputItem::ToolCall(missing),
                        ],
                        ModelStopReason::ToolContinuation,
                        semantic_events,
                    )
                }
                Stage17ProviderPlan::FinalText(text) => {
                    let part = ModelTextPart::try_new(text).unwrap();
                    (
                        vec![ModelOutputItem::text(vec![part.clone()]).unwrap()],
                        ModelStopReason::Completed,
                        vec![ModelStreamEvent::TextDelta {
                            item_ordinal: 0,
                            delta: part,
                        }],
                    )
                }
                Stage17ProviderPlan::Structured => {
                    let data = serde_json::json!({"z": 2, "a": 1});
                    (
                        vec![ModelOutputItem::structured_data(data.clone()).unwrap()],
                        ModelStopReason::Completed,
                        vec![ModelStreamEvent::StructuredData {
                            item_ordinal: 0,
                            data,
                        }],
                    )
                }
                Stage17ProviderPlan::Refusal(text) => {
                    let part = ModelTextPart::try_new(text).unwrap();
                    (
                        vec![ModelOutputItem::refusal(vec![part.clone()]).unwrap()],
                        ModelStopReason::Refusal,
                        vec![
                            ModelStreamEvent::RefusalDelta {
                                item_ordinal: 0,
                                delta: part,
                            },
                            ModelStreamEvent::RefusalCompleted { item_ordinal: 0 },
                        ],
                    )
                }
            };
            let response = ModelResponse::try_new(ModelResponseInput {
                selected_target: target.clone(),
                output_items: items,
                stop_reason,
                usage: Some(usage),
                provider_request_id: Some(request_id.clone()),
                provider_response_id: Some(response_id.clone()),
                provider_continuation: None,
                provider_metadata: ProviderMetadata::default(),
            })
            .unwrap();
            let mut events = VecDeque::from([ModelStreamEvent::ResponseStarted {
                target,
                provider_request_id: Some(request_id),
                provider_response_id: Some(response_id),
            }]);
            events.extend(semantic_events);
            events.push_back(ModelStreamEvent::Usage(usage));
            events.push_back(ModelStreamEvent::Completed(Box::new(response)));
            Ok(Box::new(Stage17ProviderStream { events }) as Box<dyn ModelProviderStream>)
        })
    }
}

fn stage17_target() -> ModelTarget {
    let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: false,
        context_window_tokens: TokenCount::try_new(128_000).unwrap(),
        max_output_tokens: TokenCount::try_new(4_096).unwrap(),
    });
    ModelTarget::try_new(ModelTargetInput {
        reference: ProviderModelReference::new(
            ModelTargetId::try_new("stage17-primary").unwrap(),
            ProviderId::try_new("stage17-fixture").unwrap(),
            ProviderModelId::try_new("fixture-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities,
        ),
        enabled: true,
        endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1").unwrap(),
        account_reference: ModelConfigReference::named("fixture-account").unwrap(),
        requested_output_tokens: TokenCount::try_new(512).unwrap(),
        estimator: TokenEstimatorIdentity::try_new("stage17_fixed", 1).unwrap(),
        provider_native_options: ProviderNativeOptions::new(false),
    })
    .unwrap()
}

struct Stage17ObservedRunner {
    inner: Arc<AgentLoop>,
    abnormal: Arc<tokio::sync::Notify>,
    release_abnormal: Arc<tokio::sync::Notify>,
}

impl WorkRunner for Stage17ObservedRunner {
    fn start(
        &self,
        work: ClaimedWork,
        cancellation: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let future = self.inner.start(work, cancellation)?;
        let abnormal = Arc::clone(&self.abnormal);
        let release = Arc::clone(&self.release_abnormal);
        Ok(Box::pin(async move {
            let exit = future.await;
            if exit == crate::application::scheduler::WorkRunnerExit::Abnormal {
                abnormal.notify_one();
                release.notified().await;
            }
            exit
        }))
    }
}

#[tokio::test]
async fn real_agent_loop_scheduler_preserves_fifo_tools_fresh_context_and_atomic_messages() {
    let fixture = fixture_with_observation(|root| {
        let workspace_root = root.path().join("stage17-workspace");
        let mut value = observation();
        value.default_shell = "/bin/bash".into();
        value.workspace_logical_root = workspace_root.to_string_lossy().into_owned();
        value.workspace_resolved_root = value.workspace_logical_root.clone();
        value.execution_capabilities = ExecutionCapabilityObservation {
            foreground_execute: true,
            privilege_administrative: false,
            process_group_cleanup: true,
            cgroup_cleanup: false,
        };
        value
    })
    .await;
    let runtime_id = RuntimeInstanceId::generate();
    start_runtime(&fixture, runtime_id, at(T1)).await;
    let first = accept(&fixture, "read the note", at(T1)).await;
    let second = accept(&fixture, "read both notes in order", at(T1)).await;
    let third = accept(&fixture, "return structured data", at(T1)).await;
    let fourth = accept(&fixture, "decline this one", at(T1)).await;

    let workspace_root = fixture._root.path().join("stage17-workspace");
    fs::create_dir(&workspace_root).unwrap();
    fs::write(workspace_root.join("note.txt"), "durable tool evidence\n").unwrap();
    fs::write(
        workspace_root.join("note-two.txt"),
        "ordered second tool evidence\n",
    )
    .unwrap();
    let artifact_store = Arc::new(
        LocalArtifactStore::initialize(&fixture._root.path().join("stage17-artifacts")).unwrap(),
    );
    let clock = Arc::new(TestClock::new(
        at(T2).to_offset_datetime(),
        Duration::from_secs(10),
    ));
    let snapshot = fixture.store.load_bootstrap_snapshot().await.unwrap();
    let workstation_clock: Arc<dyn Clock> = clock.clone();
    let workstation_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
    let local_workstation = Arc::new(
        LocalWorkstation::new(
            &snapshot.workstation,
            &snapshot.workspace,
            LocalWorkstationOptions {
                default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
                configured_workspace_root: workspace_root,
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
        read_file_default_bytes: 4_096,
        read_file_max_bytes: 65_536,
        run_shell_command_max_bytes: 65_536,
        run_shell_default_timeout_ms: 60_000,
        run_shell_max_timeout_ms: 900_000,
    };
    let registry = Arc::new(ToolRegistry::v0(policy).unwrap());
    let tool_store: Arc<dyn ToolStateStore> = Arc::new(fixture.store.clone());
    let workstation: Arc<dyn Workstation> = local_workstation.clone();
    let preparation: Arc<dyn WorkstationPreparation> = local_workstation;
    let tool_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
    let tool_clock: Arc<dyn Clock> = clock.clone();
    let tool_execution = Arc::new(
        ToolExecutionService::new(
            Arc::clone(&registry),
            Arc::new(V0AuthorityEvaluator) as Arc<dyn AuthorityEvaluator>,
            tool_store,
            workstation,
            preparation,
            tool_artifacts,
            tool_clock,
            ToolRuntimeLimits {
                read_file_default_bytes: policy.read_file_default_bytes,
                read_file_max_bytes: policy.read_file_max_bytes,
                run_shell_command_max_bytes: policy.run_shell_command_max_bytes,
                run_shell_default_timeout_ms: policy.run_shell_default_timeout_ms,
                run_shell_max_timeout_ms: policy.run_shell_max_timeout_ms,
                stdout_capture_bytes: 32_768,
                stderr_capture_bytes: 32_768,
                inline_model_result_bytes: 65_536,
                per_stream_projection_bytes: 32_768,
            },
        )
        .unwrap(),
    );

    let target = stage17_target();
    let required_capabilities = crate::domain::model::RequiredModelCapabilities {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: false,
        required_output_tokens: TokenCount::try_new(512).unwrap(),
    };
    let selection = Arc::new(ModelSelectionPolicy::new(Arc::new(
        ModelTargetSnapshot::try_new(target.reference().model_target_id().clone(), vec![target])
            .unwrap(),
    )));
    let source_store: Arc<dyn crate::ports::context_source_store::ContextSourceStore> =
        Arc::new(fixture.store.clone());
    let context_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
    let estimator: Arc<dyn TokenEstimator> = Arc::new(Stage17FixedEstimator {
        identity: TokenEstimatorIdentity::try_new("stage17_fixed", 1).unwrap(),
    });
    let context_clock: Arc<dyn Clock> = clock.clone();
    let assembler = Arc::new(ContextAssembler::new(
        source_store,
        Some(context_artifacts),
        estimator,
        Arc::clone(&registry),
        VersionedInstructionSnapshot::v0(),
        context_clock,
    ));
    let provider = Arc::new(Stage17Provider::new(vec![
        Stage17ProviderPlan::ReadFile,
        Stage17ProviderPlan::FinalText("the note was read once"),
        Stage17ProviderPlan::ReadFiles,
        Stage17ProviderPlan::FinalText("both notes were read in order"),
        Stage17ProviderPlan::Structured,
        Stage17ProviderPlan::Refusal("request refused"),
    ]));
    let gateway_store: Arc<dyn ModelStateStore> = Arc::new(fixture.store.clone());
    let gateway_artifacts: Arc<dyn ArtifactStore> = artifact_store;
    let gateway_provider: Arc<dyn ModelProvider> = provider.clone();
    let gateway_clock: Arc<dyn Clock> = clock.clone();
    let gateway = Arc::new(
        ModelGateway::new(
            gateway_store,
            gateway_artifacts,
            gateway_provider,
            Arc::new(NoopDraftSink),
            gateway_clock,
            Box::new(Stage17MinimumJitter),
            ModelGatewayLimits::default(),
        )
        .unwrap(),
    );
    let loop_store: Arc<dyn crate::application::agent_loop::AgentLoopStateStore> =
        Arc::new(fixture.store.clone());
    let loop_clock: Arc<dyn Clock> = clock.clone();
    let agent_loop = Arc::new(
        AgentLoop::new(
            selection,
            assembler,
            ContextAssemblyVersions::v0(),
            gateway,
            tool_execution,
            loop_store,
            loop_clock,
            required_capabilities,
            AgentLoopRuntimeContext {
                workstation: snapshot.workstation,
                workspace: snapshot.workspace,
                authority_constraints: V0AuthorityConstraints::default(),
            },
            AgentLoopLimits::default(),
        )
        .unwrap(),
    );
    let abnormal = Arc::new(tokio::sync::Notify::new());
    let runner = Arc::new(Stage17ObservedRunner {
        inner: agent_loop,
        abnormal: Arc::clone(&abnormal),
        release_abnormal: Arc::new(tokio::sync::Notify::new()),
    });
    let store = Arc::new(fixture.store.clone());
    let health = Health::new();
    let (fatal, _) = tokio::sync::watch::channel(false);
    let scheduler = start_scheduler(
        Arc::clone(&store),
        runner,
        Arc::clone(&clock),
        health.clone(),
        fatal,
        SchedulerStart {
            runtime_instance_id: runtime_id,
            conversation_id: fixture.identity.conversation_id,
            readiness: SchedulerReadiness::ReadyAfterInitialScan,
        },
    )
    .unwrap();

    provider.first_started.notified().await;
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let queued_followers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_items WHERE work_id IN (?, ?, ?) AND state = 'queued'",
    )
    .bind(second.work_id.to_string())
    .bind(third.work_id.to_string())
    .bind(fourth.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let leaked_sources: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM context_manifest_sources \
         WHERE source_record_kind = 'message' AND source_record_id IN (?, ?, ?)",
    )
    .bind(second.message_id.to_string())
    .bind(third.message_id.to_string())
    .bind(fourth.message_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert_eq!(queued_followers, 3);
    assert_eq!(leaked_sources, 0);
    provider.release_first.notify_one();

    let terminal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut connection = fixture.store.runtime.acquire().await.unwrap();
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM work_items WHERE work_id IN (?, ?, ?, ?) AND state = 'completed'",
            )
            .bind(first.work_id.to_string())
            .bind(second.work_id.to_string())
            .bind(third.work_id.to_string())
            .bind(fourth.work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap();
            drop(connection);
            if count == 4 {
                break true;
            }
            tokio::select! {
                () = abnormal.notified() => break false,
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await;
    if terminal != Ok(true) {
        let mut connection = fixture.store.runtime.acquire().await.unwrap();
        let states: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT work_id, state, COALESCE(terminal_reason_code, '') FROM work_items \
             WHERE work_id IN (?, ?, ?, ?) ORDER BY conversation_work_ordinal",
        )
        .bind(first.work_id.to_string())
        .bind(second.work_id.to_string())
        .bind(third.work_id.to_string())
        .bind(fourth.work_id.to_string())
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let models: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT work_id, state, COALESCE(provider_error_kind, '') FROM model_invocations \
             ORDER BY started_at, model_invocation_id",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let tools: Vec<(String, String)> = sqlx::query_as(
            "SELECT work_id, state FROM tool_executions ORDER BY requested_at, tool_execution_id",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM journal_events WHERE work_id = ? ORDER BY journal_offset",
        )
        .bind(first.work_id.to_string())
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        panic!(
            "Stage 17 loop did not terminalize: states={states:?}, models={models:?}, \
             tools={tools:?}, events={events:?}, provider_calls={}, health={:?}",
            provider.invocation_count.load(Ordering::SeqCst),
            health.snapshot()
        );
    }
    assert!(health.snapshot().is_ready());
    assert_eq!(provider.invocation_count.load(Ordering::SeqCst), 6);
    let requests = provider.requests();
    assert_eq!(requests.len(), 6);
    assert!(
        requests[1]
            .ordered_input_items()
            .iter()
            .any(|item| matches!(item, ModelInputItem::ToolResult { .. }))
    );
    let prior_assistant_parts: Vec<&str> = requests[2]
        .ordered_input_items()
        .iter()
        .filter_map(|item| match item {
            ModelInputItem::PriorAssistant { content_parts } => Some(content_parts.as_slice()),
            _ => None,
        })
        .flatten()
        .map(ModelTextPart::as_str)
        .collect();
    assert_eq!(prior_assistant_parts, ["the note was read once"]);
    assert_eq!(
        requests[3]
            .ordered_input_items()
            .iter()
            .filter(|item| matches!(item, ModelInputItem::ToolResult { .. }))
            .count(),
        4
    );
    let mut connection = fixture.store.runtime.acquire().await.unwrap();
    let evidence: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
         (SELECT COUNT(*) FROM model_invocations WHERE work_id = ?), \
         (SELECT COUNT(*) FROM tool_executions WHERE work_id = ?), \
         (SELECT COUNT(*) FROM model_invocations WHERE work_id = ?), \
         (SELECT COUNT(*) FROM tool_executions WHERE work_id = ?), \
         (SELECT COUNT(*) FROM messages WHERE produced_by_work_id IN (?, ?, ?, ?) AND role = 'assistant'), \
         (SELECT COUNT(*) FROM journal_events WHERE work_id IN (?, ?, ?, ?) AND event_type = 'assistant.message_committed')",
    )
    .bind(first.work_id.to_string())
    .bind(first.work_id.to_string())
    .bind(second.work_id.to_string())
    .bind(second.work_id.to_string())
    .bind(first.work_id.to_string())
    .bind(second.work_id.to_string())
    .bind(third.work_id.to_string())
    .bind(fourth.work_id.to_string())
    .bind(first.work_id.to_string())
    .bind(second.work_id.to_string())
    .bind(third.work_id.to_string())
    .bind(fourth.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(evidence, (2, 1, 2, 3, 4, 4));
    let ordered_tool_ordinals: Vec<i64> = sqlx::query_scalar(
        "SELECT tool_ordinal FROM tool_executions WHERE work_id = ? ORDER BY tool_ordinal",
    )
    .bind(second.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(ordered_tool_ordinals, [1, 2, 3]);
    let result_kinds: Vec<String> = sqlx::query_scalar(
        "SELECT json_extract(result_json, '$.result_kind') FROM tool_executions \
         WHERE work_id = ? ORDER BY tool_ordinal",
    )
    .bind(second.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(result_kinds, ["success", "success", "file_error"]);
    let structured: String = sqlx::query_scalar(
        "SELECT content_json FROM messages WHERE produced_by_work_id = ? AND role = 'assistant'",
    )
    .bind(third.work_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(structured.contains("{\\\"a\\\":1,\\\"z\\\":2}"));
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    scheduler.stop_and_join().await.unwrap();
    fixture.guard.shutdown().await;
}
