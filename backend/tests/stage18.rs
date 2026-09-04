#![cfg(all(feature = "test-failpoints", unix))]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use craxii_server::adapters::scripted_provider::ScriptGate;
use craxii_server::domain::{
    ArtifactId, CanonicalByteCount, ClientCommandId, ClientMessageId, ModelInputItem,
    ModelToolCallId, Sha256Digest, WorkId,
};
use craxii_server::ports::artifact_store::{ArtifactStore as _, BeginArtifactCapture};
use craxii_server::ports::state_store::BootstrapStateStore as _;
use craxii_server::test_failpoints::{
    self, ControlSelection, FailpointName, MARKER_FILE_DESCRIPTOR, PhysicalHook,
};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Connection as _;
use stage18_harness::{
    EstimatorMode, MachineFacts, ProgramPlan, Stage18Harness, Stage18Root, ToolPlan, effect_count,
    gated_answer_program, machine_plans, programs, query_count, read_invocation_records,
    retry_programs, sqlite_integrity,
};

#[test]
fn ubuntu_target_machine_contract_when_requested() {
    if std::env::var_os("CRAXII_STAGE18_REQUIRE_UBUNTU").is_none() {
        return;
    }

    let os_release = std::fs::read_to_string("/etc/os-release").expect("read /etc/os-release");
    assert!(os_release.lines().any(|line| line == "ID=ubuntu"));
    assert!(
        os_release
            .lines()
            .any(|line| line == "VERSION_ID=\"24.04\"" || line == "VERSION_ID=24.04")
    );
    assert!(Path::new("/sys/fs/cgroup/cgroup.controllers").is_file());

    let root = Stage18Root::new("ubuntu-target");
    let facts = MachineFacts::capture(&root.workspace());
    assert_eq!(facts.os, "Linux");
    assert_eq!(facts.architecture, "x86_64");
    assert_eq!(
        PathBuf::from(&facts.cwd).canonicalize().unwrap(),
        root.workspace().canonicalize().unwrap()
    );
    assert!(facts.git_version.starts_with("git version "));
    root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_http_machine_inspection_failures_idempotency_replay_and_cold_continuity() {
    let root = Stage18Root::new("spine");
    let facts = MachineFacts::capture(&root.workspace());
    let (mut plans, machine_shell_call) = machine_plans(&facts);
    let missing_call = ModelToolCallId::try_new("missing-file").unwrap();
    plans.extend([
        ProgramPlan::Tools(vec![
            ToolPlan::new(
                "missing-file",
                "read_file",
                json!({"path": "does-not-exist.txt"}),
            ),
            ToolPlan::new(
                "nonzero-shell",
                "run_shell",
                json!({"command": "printf definite-stderr >&2; exit 7"}),
            ),
        ]),
        ProgramPlan::Answer {
            text: "Both definite tool failures were observed without fabricating success."
                .to_owned(),
            require_tool_result: Some(missing_call),
        },
    ]);
    let harness = Stage18Harness::start(root, programs(&plans), EstimatorMode::Normal)
        .await
        .unwrap();
    assert!(harness.health.snapshot().is_ready());

    let machine_client_id = client_id();
    let accepted = harness
        .submit_message(
            "Inspect this machine and report OS, CPU architecture, current directory, and Git version.",
            machine_client_id,
        )
        .await;
    assert_eq!(accepted.status, 202);
    let accepted_json = accepted.json();
    assert_eq!(accepted_json["duplicate"], false);
    let machine_work: WorkId = accepted_json["work_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(harness.wait_terminal(machine_work).await, "completed");

    let duplicate = harness
        .submit_message(
            "Inspect this machine and report OS, CPU architecture, current directory, and Git version.",
            machine_client_id,
        )
        .await;
    assert_eq!(duplicate.status, 202);
    assert_eq!(duplicate.json()["duplicate"], true);
    assert_eq!(duplicate.json()["work_id"], accepted_json["work_id"]);

    let failure_client_id = client_id();
    harness
        .submit_message_losing_response(
            "Observe one missing file and one nonzero shell command, then continue honestly.",
            failure_client_id,
        )
        .await;
    let failure_work = wait_work_for_client(&harness.root.database(), failure_client_id).await;
    let replayed_response = harness
        .submit_message(
            "Observe one missing file and one nonzero shell command, then continue honestly.",
            failure_client_id,
        )
        .await;
    assert_eq!(replayed_response.status, 202);
    assert_eq!(replayed_response.json()["duplicate"], true);
    assert_eq!(
        replayed_response.json()["work_id"],
        failure_work.to_string()
    );
    assert_eq!(harness.wait_terminal(failure_work).await, "completed");

    assert_eq!(harness.provider.invocation_count(), 4);
    let captures = harness.provider.captures();
    assert_eq!(captures.len(), 4);
    assert!(captures[1]
        .request()
        .ordered_input_items()
        .iter()
        .any(|item| matches!(item, ModelInputItem::ToolResult { call_id, .. } if call_id.as_str() == machine_shell_call.as_str())));
    assert!(captures[3]
        .request()
        .ordered_input_items()
        .iter()
        .any(|item| matches!(item, ModelInputItem::ToolResult { call_id, .. } if call_id.as_str() == "missing-file")));

    let (result_kinds, assistant_count, work_count) =
        durable_spine_counts(&harness.root.database()).await;
    assert_eq!(result_kinds.len(), 4);
    assert_eq!(result_kinds[0], "success");
    assert_eq!(result_kinds[1], "success");
    assert!(result_kinds[2].contains("file"));
    assert!(result_kinds[3].contains("nonzero") || result_kinds[3].contains("exit"));
    assert_eq!(assistant_count, 2);
    assert_eq!(work_count, 2);

    let bootstrap = harness.bootstrap().await;
    assert_eq!(bootstrap.status, 200);
    let bootstrap_text = String::from_utf8(bootstrap.body.clone()).unwrap();
    assert!(bootstrap_text.contains(&facts.git_version));
    assert!(bootstrap_text.contains("Both definite tool failures"));
    let replay = harness.replay_from_zero().await;
    let replay_text = serde_json::to_string(&replay).unwrap();
    assert!(replay_text.contains("sync.complete"));
    assert!(replay_text.contains(&facts.git_version));
    let replayed_durable = replay
        .iter()
        .filter(|frame| frame["delivery_kind"] == "durable")
        .count() as u64;
    let delivery_metrics = harness.live_events.metrics();
    assert_eq!(delivery_metrics.replayed_durable_events, replayed_durable);
    assert_eq!(delivery_metrics.replay_connections, 1);

    let ledger_before_restart = read_invocation_records(&harness.root.invocation_log());
    assert_eq!(ledger_before_restart.len(), 4);
    assert_eq!(
        ledger_before_restart
            .iter()
            .filter(|record| record.work_id == machine_work.to_string())
            .count(),
        2
    );
    assert_eq!(
        ledger_before_restart
            .iter()
            .filter(|record| record.work_id == failure_work.to_string())
            .count(),
        2
    );

    let root = harness.shutdown().await;
    assert_eq!(
        sqlite_integrity(&root.database()).await,
        ("ok".to_owned(), 1)
    );

    let follow_up = ProgramPlan::Answer {
        text: format!("Previously observed Git version: {}", facts.git_version),
        require_tool_result: Some(machine_shell_call.clone()),
    };
    let reopened = Stage18Harness::start(
        Stage18Root::from_existing(root.path().to_owned()),
        programs(&[follow_up]),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    assert!(reopened.health.snapshot().is_ready());
    assert_eq!(reopened.provider.invocation_count(), 0);
    let follow_client = client_id();
    let response = reopened
        .submit_message(
            "What Git version did you observe before the restart?",
            follow_client,
        )
        .await;
    let follow_work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(reopened.wait_terminal(follow_work).await, "completed");
    assert_eq!(reopened.provider.invocation_count(), 1);
    assert!(reopened.provider.captures()[0]
        .request()
        .ordered_input_items()
        .iter()
        .any(|item| matches!(item, ModelInputItem::ToolResult { call_id, .. } if call_id.as_str() == machine_shell_call.as_str())));
    let reopened_bootstrap = reopened.bootstrap().await;
    assert_eq!(reopened_bootstrap.status, 200);
    assert!(
        String::from_utf8(reopened_bootstrap.body)
            .unwrap()
            .contains(&facts.git_version)
    );
    let all_records = read_invocation_records(&reopened.root.invocation_log());
    assert_eq!(all_records.len(), 5);
    assert_ne!(
        all_records[0].logical_invocation_id,
        all_records[4].logical_invocation_id
    );
    assert_eq!(
        query_count(
            &reopened.root.database(),
            "SELECT COUNT(*) FROM work_items WHERE state IN ('completed','failed','cancelled','interrupted') AND runtime_instance_id IS NOT NULL",
        )
        .await,
        0
    );
    assert_eq!(
        query_count(
            &reopened.root.database(),
            "SELECT COUNT(*) FROM messages m LEFT JOIN work_items w ON w.work_id = m.produced_by_work_id WHERE m.role = 'assistant' AND (w.work_id IS NULL OR w.state <> 'completed')",
        )
        .await,
        0
    );
    assert_eq!(
        query_count(
            &reopened.root.database(),
            "SELECT COUNT(*) FROM (SELECT logical_invocation_id, attempt_no, ROW_NUMBER() OVER (PARTITION BY logical_invocation_id ORDER BY attempt_no) AS expected FROM model_invocations) WHERE attempt_no <> expected",
        )
        .await,
        0
    );
    reopened
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let root = reopened.shutdown().await;
    root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safe_retry_and_context_limit_full_work_paths_have_exact_effect_counts() {
    let retry_root = Stage18Root::new("retry");
    let retry = Stage18Harness::start(
        retry_root,
        retry_programs("safe retry completed", 3),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let response = retry
        .submit_message("retry only when definitely safe", client_id())
        .await;
    let retry_work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(retry.wait_terminal(retry_work).await, "completed");
    assert_eq!(retry.provider.invocation_count(), 3);
    let attempts: Vec<u32> = read_invocation_records(&retry.root.invocation_log())
        .into_iter()
        .map(|record| record.physical_attempt)
        .collect();
    assert_eq!(attempts, [1, 2, 3]);
    assert_eq!(
        query_count(
            &retry.root.database(),
            "SELECT COUNT(*) FROM model_invocations WHERE retry_reason = 'classified_transient_before_output'",
        )
        .await,
        2
    );
    let retry_root = retry.shutdown().await;
    retry_root.remove();

    let limit_root = Stage18Root::new("context-limit");
    let limit = Stage18Harness::start(limit_root, Vec::new(), EstimatorMode::ContextLimit)
        .await
        .unwrap();
    let first = limit
        .submit_message("overflow context one", client_id())
        .await;
    let second = limit
        .submit_message("overflow context follower", client_id())
        .await;
    let first_work: WorkId = first.json()["work_id"].as_str().unwrap().parse().unwrap();
    let second_work: WorkId = second.json()["work_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(limit.wait_terminal(first_work).await, "failed");
    assert_eq!(limit.wait_terminal(second_work).await, "failed");
    assert_eq!(limit.provider.invocation_count(), 0);
    assert!(read_invocation_records(&limit.root.invocation_log()).is_empty());
    assert_eq!(
        query_count(
            &limit.root.database(),
            "SELECT COUNT(*) FROM tool_executions"
        )
        .await,
        0
    );
    assert_eq!(
        query_count(
            &limit.root.database(),
            "SELECT COUNT(*) FROM messages WHERE role = 'assistant'",
        )
        .await,
        0
    );
    assert_eq!(
        query_count(
            &limit.root.database(),
            "SELECT COUNT(*) FROM work_items WHERE state = 'failed' AND terminal_reason_code = 'lifecycle_limit' AND json_extract(terminal_detail_json, '$.limit') = 'context'",
        )
        .await,
        2
    );
    assert_eq!(effect_count(&limit.root.effect_log()), 0);
    let limit_root = limit.shutdown().await;
    limit_root.remove();
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct NormalizedEvidence {
    protocol: &'static str,
    request_hashes: Vec<String>,
    tool_result_classes: Vec<String>,
    assistant_semantics: Vec<String>,
    provider_calls: usize,
    work_states: Vec<String>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normalized_clean_state_evidence_repeats_and_matches_frozen_contract() {
    let root = Stage18Root::new("repeat");
    let repeat_path = root.path().to_owned();
    let first = normalized_baseline(root).await;
    let second = normalized_baseline(Stage18Root::recreate(repeat_path)).await;
    assert_eq!(first, second);
    let contract: Value =
        serde_json::from_str(include_str!("fixtures/stage18-v1/evidence-contract.json")).unwrap();
    assert_eq!(contract["protocol"], first.protocol);
    assert_eq!(
        contract["required_fields"].as_array().unwrap().len(),
        serde_json::to_value(&first)
            .unwrap()
            .as_object()
            .unwrap()
            .len()
    );
}

async fn normalized_baseline(root: Stage18Root) -> NormalizedEvidence {
    let facts = MachineFacts::capture(&root.workspace());
    let (programs, _) = stage18_harness::machine_programs(&facts);
    let harness = Stage18Harness::start(root, programs, EstimatorMode::Normal)
        .await
        .unwrap();
    let response = harness
        .submit_message("canonical machine inspection", client_id())
        .await;
    let work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(harness.wait_terminal(work).await, "completed");
    let request_hashes = harness
        .provider
        .captures()
        .iter()
        .map(|capture| {
            normalized_request_hash(&capture.request().canonical_bytes(), harness.root.path())
        })
        .collect();
    let (tool_result_classes, assistant_semantics, work_states) =
        normalized_database_rows(&harness.root.database(), harness.root.path()).await;
    let evidence = NormalizedEvidence {
        protocol: "craxii.stage18.evidence.v1",
        request_hashes,
        tool_result_classes,
        assistant_semantics,
        provider_calls: harness.provider.invocation_count() as usize,
        work_states,
    };
    let root = harness.shutdown().await;
    root.remove();
    evidence
}

fn normalized_request_hash(bytes: &[u8], root: &Path) -> String {
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    let canonical_root = root.canonicalize().unwrap();
    normalize_value(&mut value, canonical_root.to_str().unwrap());
    Sha256Digest::hash_bytes(&serde_json::to_vec(&value).unwrap()).to_string()
}

fn normalize_value(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => {
            if text.starts_with(root) {
                *text = text.replacen(root, "<root>", 1);
            } else if uuid::Uuid::parse_str(text).is_ok() {
                *text = "<uuidv7>".to_owned();
            }
        }
        Value::Array(values) => {
            if values.len() == 2 && values[0].as_str() == Some("duration_ms") {
                values[1] = Value::String("<latency>".to_owned());
                return;
            }
            for value in values {
                normalize_value(value, root);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_value(value, root);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

async fn wait_work_for_client(database: &Path, client_id: ClientMessageId) -> WorkId {
    for _ in 0..1_000 {
        let mut connection = open_read(database).await;
        let work: Option<String> = sqlx::query_scalar(
            "SELECT json_extract(response_json, '$.work_id') FROM client_commands WHERE idempotency_key = ? AND command_type = 'message'",
        )
        .bind(client_id.to_string())
        .fetch_optional(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        if let Some(work) = work {
            return work.parse().unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("lost-response command did not commit")
}

async fn durable_spine_counts(database: &Path) -> (Vec<String>, i64, i64) {
    let mut connection = open_read(database).await;
    let result_kinds = sqlx::query_scalar(
        "SELECT json_extract(result_json, '$.result_kind') FROM tool_executions ORDER BY requested_at, tool_ordinal",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let assistant_count =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role = 'assistant'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let work_count = sqlx::query_scalar("SELECT COUNT(*) FROM work_items")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    (result_kinds, assistant_count, work_count)
}

async fn normalized_database_rows(
    database: &Path,
    root: &Path,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let canonical_root = root.canonicalize().unwrap();
    let mut connection = open_read(database).await;
    let tool_result_classes = sqlx::query_scalar(
        "SELECT json_extract(result_json, '$.result_kind') FROM tool_executions ORDER BY tool_ordinal",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let mut assistant_semantics: Vec<String> = sqlx::query_scalar(
        "SELECT content_json FROM messages WHERE role = 'assistant' ORDER BY committed_at",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    for text in &mut assistant_semantics {
        *text = text.replace(canonical_root.to_str().unwrap(), "<root>");
    }
    let work_states = sqlx::query_scalar(
        "SELECT state || ':' || COALESCE(terminal_reason_code, '') FROM work_items ORDER BY conversation_work_ordinal",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    (tool_result_classes, assistant_semantics, work_states)
}

async fn open_read(database: &Path) -> sqlx::SqliteConnection {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

fn client_id() -> ClientMessageId {
    ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

fn command_id() -> ClientCommandId {
    ClientCommandId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

#[derive(Clone, Copy, Debug)]
enum CrashProgram {
    Answer,
    Tool,
    Cancel,
}

#[derive(Clone, Copy, Debug)]
struct CrashCase {
    label: &'static str,
    alias: FailpointName,
    physical: PhysicalHook,
    program: CrashProgram,
    calls_before_crash: usize,
    work_before_recovery: &'static str,
    work_after_recovery: &'static str,
    model_after_recovery: Option<&'static str>,
    tool_after_recovery: Option<&'static str>,
    minimum_effects_before_recovery: usize,
    maximum_effects_before_recovery: usize,
    assistant_after_recovery: i64,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_process_loss_covers_model_tool_and_final_answer_durable_classes() {
    let cases = [
        CrashCase {
            label: "before-claim",
            alias: FailpointName::AfterMessageTransactionCommit,
            physical: PhysicalHook::AfterMessageTransactionCommit,
            program: CrashProgram::Answer,
            calls_before_crash: 0,
            work_before_recovery: "queued",
            work_after_recovery: "completed:answered",
            model_after_recovery: Some("completed"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 1,
        },
        CrashCase {
            label: "after-claim",
            alias: FailpointName::AfterWorkClaimCommit,
            physical: PhysicalHook::AfterWorkClaimCommit,
            program: CrashProgram::Answer,
            calls_before_crash: 0,
            work_before_recovery: "running",
            work_after_recovery: "interrupted:runtime_ownership_lost",
            model_after_recovery: None,
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "manifest-rows-precommit",
            alias: FailpointName::AfterContextManifestCommit,
            physical: PhysicalHook::ModelAttemptAfterManifestRowsBeforeIntent,
            program: CrashProgram::Answer,
            calls_before_crash: 0,
            work_before_recovery: "running",
            work_after_recovery: "interrupted:runtime_ownership_lost",
            model_after_recovery: None,
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "all-model-rows-precommit",
            alias: FailpointName::AfterModelIntentCommit,
            physical: PhysicalHook::ModelAttemptAfterAllRowsBeforeCommit,
            program: CrashProgram::Answer,
            calls_before_crash: 0,
            work_before_recovery: "running",
            work_after_recovery: "interrupted:runtime_ownership_lost",
            model_after_recovery: None,
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "intent-before-provider",
            alias: FailpointName::AfterModelIntentCommit,
            physical: PhysicalHook::ModelAttemptAfterCommitBeforeProviderIo,
            program: CrashProgram::Answer,
            calls_before_crash: 0,
            work_before_recovery: "waiting_on_model",
            work_after_recovery: "interrupted:provider_outcome_unknown",
            model_after_recovery: Some("provider_outcome_unknown"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "first-provider-delta",
            alias: FailpointName::AfterFirstProviderDelta,
            physical: PhysicalHook::AfterFirstProviderDelta,
            program: CrashProgram::Answer,
            calls_before_crash: 1,
            work_before_recovery: "waiting_on_model",
            work_after_recovery: "interrupted:provider_outcome_unknown",
            model_after_recovery: Some("provider_outcome_unknown"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "model-terminal",
            alias: FailpointName::AfterModelResponseCommit,
            physical: PhysicalHook::AfterModelResponseCommit,
            program: CrashProgram::Answer,
            calls_before_crash: 1,
            work_before_recovery: "running",
            work_after_recovery: "interrupted:runtime_ownership_lost",
            model_after_recovery: Some("completed"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "tool-requested",
            alias: FailpointName::AfterToolRequestedCommit,
            physical: PhysicalHook::AfterToolRequestedCommit,
            program: CrashProgram::Tool,
            calls_before_crash: 1,
            work_before_recovery: "waiting_on_tool",
            work_after_recovery: "interrupted:tool_interrupted_before_dispatch",
            model_after_recovery: Some("completed"),
            tool_after_recovery: Some("interrupted_before_dispatch"),
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "tool-dispatch",
            alias: FailpointName::AfterToolDispatchIntentCommit,
            physical: PhysicalHook::AfterToolDispatchIntentCommit,
            program: CrashProgram::Tool,
            calls_before_crash: 1,
            work_before_recovery: "waiting_on_tool",
            work_after_recovery: "interrupted:tool_outcome_unknown",
            model_after_recovery: Some("completed"),
            tool_after_recovery: Some("outcome_unknown"),
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "tool-spawn",
            alias: FailpointName::AfterToolProcessSpawn,
            physical: PhysicalHook::AfterToolProcessSpawn,
            program: CrashProgram::Tool,
            calls_before_crash: 1,
            work_before_recovery: "waiting_on_tool",
            work_after_recovery: "interrupted:tool_outcome_unknown",
            model_after_recovery: Some("completed"),
            tool_after_recovery: Some("outcome_unknown"),
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 1,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "tool-exit",
            alias: FailpointName::AfterToolProcessExitBeforeOutcomeCommit,
            physical: PhysicalHook::AfterToolProcessExitBeforeOutcomeCommit,
            program: CrashProgram::Tool,
            calls_before_crash: 1,
            work_before_recovery: "waiting_on_tool",
            work_after_recovery: "interrupted:tool_outcome_unknown",
            model_after_recovery: Some("completed"),
            tool_after_recovery: Some("outcome_unknown"),
            minimum_effects_before_recovery: 1,
            maximum_effects_before_recovery: 1,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "assistant-precommit",
            alias: FailpointName::AfterAssistantMessageCommit,
            physical: PhysicalHook::FinalAnswerAfterAllRowsBeforeCommit,
            program: CrashProgram::Answer,
            calls_before_crash: 1,
            work_before_recovery: "running",
            work_after_recovery: "interrupted:runtime_ownership_lost",
            model_after_recovery: Some("completed"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
        CrashCase {
            label: "assistant-postcommit",
            alias: FailpointName::AfterAssistantMessageCommit,
            physical: PhysicalHook::FinalAnswerAfterCommitBeforeNotification,
            program: CrashProgram::Answer,
            calls_before_crash: 1,
            work_before_recovery: "completed",
            work_after_recovery: "completed:answered",
            model_after_recovery: Some("completed"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 1,
        },
        CrashCase {
            label: "cancel-requested",
            alias: FailpointName::AfterCancelRequestedCommit,
            physical: PhysicalHook::AfterCancelRequestedCommit,
            program: CrashProgram::Cancel,
            calls_before_crash: 1,
            work_before_recovery: "cancel_requested",
            work_after_recovery: "interrupted:provider_outcome_unknown",
            model_after_recovery: Some("provider_outcome_unknown"),
            tool_after_recovery: None,
            minimum_effects_before_recovery: 0,
            maximum_effects_before_recovery: 0,
            assistant_after_recovery: 0,
        },
    ];

    for case in cases {
        let root_path = Stage18Root::new(case.label).preserve();
        let root = Stage18Root::from_existing(root_path);
        run_crash_child(root.path(), case).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            crash_work_projection(&root.database()).await.0,
            case.work_before_recovery,
            "{} pre-recovery Work state",
            case.label
        );
        assert_eq!(
            read_invocation_records(&root.invocation_log()).len(),
            case.calls_before_crash,
            "{} provider calls before recovery",
            case.label
        );
        let effects = effect_count(&root.effect_log());
        assert!(
            (case.minimum_effects_before_recovery..=case.maximum_effects_before_recovery)
                .contains(&effects),
            "{} effects before recovery: {effects}",
            case.label
        );

        let recovery_programs = if case.label == "before-claim" {
            programs(&[ProgramPlan::Answer {
                text: "queued command completed only after cold recovery".to_owned(),
                require_tool_result: None,
            }])
        } else {
            Vec::new()
        };
        let reopened = Stage18Harness::start(
            Stage18Root::from_existing(root.path().to_owned()),
            recovery_programs,
            EstimatorMode::Normal,
        )
        .await
        .unwrap_or_else(|error| panic!("{} recovery startup: {error}", case.label));
        if case.label == "before-claim" {
            let work = only_work_id(&reopened.root.database()).await;
            assert_eq!(reopened.wait_terminal(work).await, "completed");
        }
        let projection = crash_work_projection(&reopened.root.database()).await;
        assert_eq!(
            projection.1, case.work_after_recovery,
            "{} recovery",
            case.label
        );
        assert_eq!(
            latest_state(&reopened.root.database(), "model_invocations").await,
            case.model_after_recovery.map(str::to_owned),
            "{} model state",
            case.label
        );
        assert_eq!(
            latest_state(&reopened.root.database(), "tool_executions").await,
            case.tool_after_recovery.map(str::to_owned),
            "{} tool state",
            case.label
        );
        assert_eq!(
            query_count(
                &reopened.root.database(),
                "SELECT COUNT(*) FROM messages WHERE role = 'assistant'",
            )
            .await,
            case.assistant_after_recovery,
            "{} assistant count",
            case.label
        );
        let expected_calls = case.calls_before_crash + usize::from(case.label == "before-claim");
        assert_eq!(
            read_invocation_records(&reopened.root.invocation_log()).len(),
            expected_calls,
            "{} provider calls after recovery",
            case.label
        );
        assert_eq!(
            effect_count(&reopened.root.effect_log()),
            effects,
            "{} tool effect repeated during recovery",
            case.label
        );
        reopened
            .store
            .verify_application_consistency()
            .await
            .unwrap();
        let reopened_root = reopened.shutdown().await;
        reopened_root.remove();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifo_follower_isolated_from_crashed_predecessor_and_progresses_after_recovery() {
    let case = CrashCase {
        label: "fifo-provider-delta",
        alias: FailpointName::AfterFirstProviderDelta,
        physical: PhysicalHook::AfterFirstProviderDelta,
        program: CrashProgram::Answer,
        calls_before_crash: 1,
        work_before_recovery: "waiting_on_model",
        work_after_recovery: "interrupted:provider_outcome_unknown",
        model_after_recovery: Some("provider_outcome_unknown"),
        tool_after_recovery: None,
        minimum_effects_before_recovery: 0,
        maximum_effects_before_recovery: 0,
        assistant_after_recovery: 0,
    };
    let root_path = Stage18Root::new(case.label).preserve();
    let root = Stage18Root::from_existing(root_path);
    run_crash_child_mode(root.path(), case, true).unwrap();

    let mut connection = open_read(&root.database()).await;
    let works: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT work_id, state, terminal_reason_code FROM work_items ORDER BY conversation_work_ordinal",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(works.len(), 2);
    assert_eq!(works[0].1, "waiting_on_model");
    assert_eq!(works[1].1, "queued");
    let user_messages: Vec<String> = sqlx::query_scalar(
        "SELECT message_id FROM messages WHERE role = 'user' ORDER BY committed_at",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(user_messages.len(), 2);
    let follower_in_predecessor_manifest: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM context_manifest_sources cms JOIN context_manifests cm ON cm.context_manifest_id = cms.context_manifest_id WHERE cm.work_id = ? AND cms.source_record_kind = 'message' AND cms.source_record_id = ?",
    )
    .bind(&works[0].0)
    .bind(&user_messages[1])
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(follower_in_predecessor_manifest, 0);
    connection.close().await.unwrap();

    let reopened = Stage18Harness::start(
        Stage18Root::from_existing(root.path().to_owned()),
        programs(&[ProgramPlan::Answer {
            text: "fresh follower answer after predecessor recovery".to_owned(),
            require_tool_result: None,
        }]),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let follower_work: WorkId = works[1].0.parse().unwrap();
    assert_eq!(reopened.wait_terminal(follower_work).await, "completed");
    let mut connection = open_read(&reopened.root.database()).await;
    let recovered: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT state, terminal_reason_code, runtime_instance_id FROM work_items ORDER BY conversation_work_ordinal",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        recovered,
        vec![
            (
                "interrupted".to_owned(),
                Some("provider_outcome_unknown".to_owned()),
                None,
            ),
            ("completed".to_owned(), Some("answered".to_owned()), None),
        ]
    );
    let manifest_works: Vec<String> = sqlx::query_scalar(
        "SELECT work_id FROM context_manifests ORDER BY created_at, context_manifest_id",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(manifest_works.len(), 2);
    assert_eq!(manifest_works[0], works[0].0);
    assert_eq!(manifest_works[1], works[1].0);
    connection.close().await.unwrap();
    let records = read_invocation_records(&reopened.root.invocation_log());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].work_id, works[0].0);
    assert_eq!(records[1].work_id, works[1].0);
    assert_ne!(
        records[0].logical_invocation_id,
        records[1].logical_invocation_id
    );
    let request =
        String::from_utf8(reopened.provider.captures()[0].request().canonical_bytes()).unwrap();
    assert!(request.contains("execute the subprocess crash scenario"));
    assert!(request.contains("follower accepted only after predecessor"));
    reopened
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let reopened_root = reopened.shutdown().await;
    reopened_root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_and_graceful_shutdown_preserve_definite_and_uncertain_boundaries() {
    let gate = ScriptGate::new();
    let cancellation = Stage18Harness::start(
        Stage18Root::new("cancel-model"),
        gated_answer_program("must never complete", gate, false),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let active_response = cancellation
        .submit_message("hold the predecessor provider", client_id())
        .await;
    let active: WorkId = active_response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for_provider_calls(&cancellation, 1).await;
    wait_for_state(&cancellation.root.database(), active, "waiting_on_model").await;

    let queued_response = cancellation
        .submit_message("cancel me before any provider side effect", client_id())
        .await;
    let queued: WorkId = queued_response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for_state(&cancellation.root.database(), queued, "queued").await;
    let queued_cancel = cancellation.cancel_work(queued, command_id()).await;
    assert!(matches!(queued_cancel.status, 200 | 202));
    assert_eq!(cancellation.wait_terminal(queued).await, "cancelled");
    assert_eq!(cancellation.provider.invocation_count(), 1);
    assert_eq!(
        query_count(
            &cancellation.root.database(),
            "SELECT COUNT(*) FROM model_invocations mi WHERE mi.work_id = (SELECT work_id FROM work_items WHERE conversation_work_ordinal = 2)",
        )
        .await,
        0
    );

    let active_cancel = cancellation.cancel_work(active, command_id()).await;
    assert!(matches!(active_cancel.status, 200 | 202));
    assert_eq!(cancellation.wait_terminal(active).await, "interrupted");
    assert_eq!(
        work_reason(&cancellation.root.database(), active).await,
        Some("provider_outcome_unknown".to_owned())
    );
    assert_eq!(
        latest_state(&cancellation.root.database(), "model_invocations").await,
        Some("provider_outcome_unknown".to_owned())
    );
    assert_eq!(
        query_count(
            &cancellation.root.database(),
            "SELECT COUNT(*) FROM messages WHERE role = 'assistant'",
        )
        .await,
        0
    );
    let cancellation_root = cancellation.shutdown().await;
    cancellation_root.remove();

    let tool_plans = [
        ProgramPlan::Tools(vec![ToolPlan::new(
            "cancel-real-tool",
            "run_shell",
            json!({"command": "sleep 30; printf 'late-effect\\n' >> stage18-effect.log"}),
        )]),
        ProgramPlan::Answer {
            text: "must not continue after cancellation".to_owned(),
            require_tool_result: Some(ModelToolCallId::try_new("cancel-real-tool").unwrap()),
        },
    ];
    let tool_cancel = Stage18Harness::start(
        Stage18Root::new("cancel-tool"),
        programs(&tool_plans),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let tool_response = tool_cancel
        .submit_message("start a real cancellable tool", client_id())
        .await;
    let tool_work: WorkId = tool_response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for_table_state(
        &tool_cancel.root.database(),
        "tool_executions",
        "dispatching",
    )
    .await;
    let response = tool_cancel.cancel_work(tool_work, command_id()).await;
    assert!(matches!(response.status, 200 | 202));
    assert_eq!(tool_cancel.wait_terminal(tool_work).await, "cancelled");
    assert_eq!(
        work_reason(&tool_cancel.root.database(), tool_work).await,
        Some("user_request".to_owned())
    );
    assert_eq!(effect_count(&tool_cancel.root.effect_log()), 0);
    assert_eq!(
        query_count(
            &tool_cancel.root.database(),
            "SELECT COUNT(*) FROM tool_executions WHERE cleanup_confirmed = 1",
        )
        .await,
        1
    );
    assert_eq!(tool_cancel.provider.invocation_count(), 1);
    assert_eq!(
        query_count(
            &tool_cancel.root.database(),
            "SELECT COUNT(*) FROM messages WHERE role = 'assistant'",
        )
        .await,
        0
    );
    let tool_root = tool_cancel.shutdown().await;
    tool_root.remove();

    let shutdown_gate = ScriptGate::new();
    let model_shutdown = Stage18Harness::start(
        Stage18Root::new("shutdown-model"),
        gated_answer_program("must never complete", shutdown_gate, false),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let response = model_shutdown
        .submit_message("wait on the provider during shutdown", client_id())
        .await;
    let work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for_provider_calls(&model_shutdown, 1).await;
    model_shutdown.begin_graceful_shutdown().await;
    assert_eq!(model_shutdown.wait_terminal(work).await, "interrupted");
    assert_eq!(
        work_reason(&model_shutdown.root.database(), work).await,
        Some("provider_outcome_unknown".to_owned())
    );
    assert_eq!(
        model_shutdown.health.snapshot().state().as_str(),
        "draining"
    );
    assert_eq!(model_shutdown.provider.invocation_count(), 1);
    let model_root = model_shutdown.shutdown().await;
    model_root.remove();

    let shutdown_tool_plans = [
        ProgramPlan::Tools(vec![ToolPlan::new(
            "shutdown-real-tool",
            "run_shell",
            json!({"command": "sleep 30; printf 'late-shutdown-effect\\n' >> stage18-effect.log"}),
        )]),
        ProgramPlan::Answer {
            text: "must not continue after shutdown".to_owned(),
            require_tool_result: Some(ModelToolCallId::try_new("shutdown-real-tool").unwrap()),
        },
    ];
    let tool_shutdown = Stage18Harness::start(
        Stage18Root::new("shutdown-tool"),
        programs(&shutdown_tool_plans),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let response = tool_shutdown
        .submit_message("run a real tool during graceful shutdown", client_id())
        .await;
    let work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for_table_state(
        &tool_shutdown.root.database(),
        "tool_executions",
        "dispatching",
    )
    .await;
    tool_shutdown.begin_graceful_shutdown().await;
    assert_eq!(tool_shutdown.wait_terminal(work).await, "cancelled");
    assert_eq!(
        work_reason(&tool_shutdown.root.database(), work).await,
        Some("graceful_shutdown".to_owned())
    );
    assert_eq!(effect_count(&tool_shutdown.root.effect_log()), 0);
    assert_eq!(
        query_count(
            &tool_shutdown.root.database(),
            "SELECT COUNT(*) FROM tool_executions WHERE cleanup_confirmed = 1",
        )
        .await,
        1
    );
    assert_eq!(tool_shutdown.provider.invocation_count(), 1);
    let tool_shutdown_root = tool_shutdown.shutdown().await;
    tool_shutdown_root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readiness_fatal_wal_and_artifact_integrity_are_cold_reopen_authoritative() {
    let root_path = Stage18Root::new("durability").preserve();
    let root = Stage18Root::from_existing(root_path);
    let facts = MachineFacts::capture(&root.workspace());
    let (provider_programs, _) = stage18_harness::machine_programs(&facts);
    let harness = Stage18Harness::start(root, provider_programs, EstimatorMode::Normal)
        .await
        .unwrap();
    assert!(harness.health.snapshot().is_ready());
    let first_runtime = harness.runtime_id;
    let response = harness
        .submit_message("produce referenced artifact evidence", client_id())
        .await;
    let work: WorkId = response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(harness.wait_terminal(work).await, "completed");

    let mut connection = open_read(&harness.root.database()).await;
    let referenced: (String, String, i64) = sqlx::query_as(
        "SELECT storage_key, sha256, captured_byte_count FROM artifacts ORDER BY created_at LIMIT 1",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let referenced_path = harness.root.artifact_root().join(&referenced.0);
    let referenced_bytes = std::fs::read(&referenced_path).unwrap();
    assert_eq!(i64::try_from(referenced_bytes.len()).unwrap(), referenced.2);
    assert_eq!(
        Sha256Digest::hash_bytes(&referenced_bytes).to_string(),
        referenced.1
    );

    let orphan_bytes = b"safe finalized unreferenced Stage 18 artifact\n";
    let mut orphan_capture = harness
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id: ArtifactId::generate(),
            hard_capture_limit: CanonicalByteCount::try_new(4_096).unwrap(),
        })
        .unwrap();
    orphan_capture.write_chunk(orphan_bytes).unwrap();
    let orphan = orphan_capture.finalize().unwrap();
    assert_eq!(orphan.sha256(), Sha256Digest::hash_bytes(orphan_bytes));
    assert_eq!(
        query_count(&harness.root.database(), "SELECT COUNT(*) FROM artifacts").await,
        1
    );
    let root = harness.shutdown().await;
    assert_eq!(
        sqlite_integrity(&root.database()).await,
        ("ok".to_owned(), 1)
    );
    let mut connection = open_read(&root.database()).await;
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(journal_mode, "wal");

    let reopened = Stage18Harness::start(
        Stage18Root::from_existing(root.path().to_owned()),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    assert!(reopened.health.snapshot().is_ready());
    assert_ne!(reopened.runtime_id, first_runtime);
    assert_eq!(reopened.provider.invocation_count(), 0);
    assert_eq!(
        query_count(
            &reopened.root.database(),
            "SELECT COUNT(*) FROM journal_events WHERE event_type = 'runtime.recovery_performed' AND json_extract(payload_json, '$.orphan_artifacts_observed') = 2",
        )
        .await,
        1
    );
    reopened
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    let root = reopened.shutdown().await;

    std::fs::write(&referenced_path, b"corrupt").unwrap();
    let corrupt_start = Stage18Harness::start(
        Stage18Root::from_existing(root.path().to_owned()),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await;
    assert!(corrupt_start.is_err());
    root.remove();

    let inconsistent_path = Stage18Root::new("projection-inconsistent").preserve();
    let clean = Stage18Harness::start(
        Stage18Root::from_existing(inconsistent_path.clone()),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let clean_root = clean.shutdown().await;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(clean_root.database())
        .foreign_keys(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET next_work_ordinal = next_work_ordinal + 7")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let inconsistent_start = Stage18Harness::start(
        Stage18Root::from_existing(clean_root.path().to_owned()),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await;
    assert!(inconsistent_start.is_err());
    clean_root.remove();

    let fatal = Stage18Harness::start(
        Stage18Root::new("post-ready-fatal"),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    assert!(fatal.health.snapshot().is_ready());
    fatal.induce_storage_failure().await;
    for _ in 0..1_000 {
        if fatal.health.snapshot().state().as_str() == "fatal" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fatal.health.snapshot().state().as_str(), "fatal");
    let fatal_root = fatal.teardown_after_fatal().await;
    fatal_root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_tool_result_survives_process_loss_without_invented_continuation() {
    let root_path = Stage18Root::new("tool-result-durable").preserve();
    let root = Stage18Root::from_existing(root_path);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "stage18_tool_result_subprocess_child",
            "--nocapture",
        ])
        .env("CRAXII_STAGE18_TOOL_RESULT_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let marker = root.path().join("tool-result-durable.marker");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        marker.exists(),
        "child did not reach post-tool context boundary"
    );
    let pid = i32::try_from(child.id()).unwrap();
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: this is the live direct test child and the signal is scoped to it.
    assert_eq!(unsafe { kill(pid, 9) }, 0);
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    assert_eq!(read_invocation_records(&root.invocation_log()).len(), 1);
    assert_eq!(effect_count(&root.effect_log()), 1);
    assert_eq!(
        latest_state(&root.database(), "tool_executions").await,
        Some("completed".to_owned())
    );

    let reopened = Stage18Harness::start(
        Stage18Root::from_existing(root.path().to_owned()),
        Vec::new(),
        EstimatorMode::Normal,
    )
    .await
    .unwrap();
    let work = only_work_id(&reopened.root.database()).await;
    assert_eq!(reopened.wait_terminal(work).await, "interrupted");
    assert_eq!(
        work_reason(&reopened.root.database(), work).await,
        Some("runtime_ownership_lost".to_owned())
    );
    assert_eq!(
        latest_state(&reopened.root.database(), "tool_executions").await,
        Some("completed".to_owned())
    );
    assert_eq!(
        read_invocation_records(&reopened.root.invocation_log()).len(),
        1
    );
    assert_eq!(effect_count(&reopened.root.effect_log()), 1);
    assert_eq!(
        query_count(
            &reopened.root.database(),
            "SELECT COUNT(*) FROM messages WHERE role = 'assistant'",
        )
        .await,
        0
    );
    let root = reopened.shutdown().await;
    root.remove();
}

#[test]
fn stage18_tool_result_subprocess_child() {
    let Some(root) = std::env::var_os("CRAXII_STAGE18_TOOL_RESULT_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let marker = root.join("tool-result-durable.marker");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let plans = [
            ProgramPlan::Tools(vec![ToolPlan::new(
                "durable-effect",
                "run_shell",
                json!({"command": "printf 'stable-tool-execution\\n' >> stage18-effect.log; sync"}),
            )]),
            ProgramPlan::Answer {
                text: "must not execute before process loss".to_owned(),
                require_tool_result: Some(ModelToolCallId::try_new("durable-effect").unwrap()),
            },
        ];
        let harness = Stage18Harness::start(
            Stage18Root::from_existing(root),
            programs(&plans),
            EstimatorMode::PauseOnSecond(marker),
        )
        .await
        .unwrap();
        harness
            .submit_message_losing_response("complete one durable tool result", client_id())
            .await;
        std::future::pending::<()>().await;
    });
}

#[test]
fn stage18_subprocess_child() {
    let Some(root) = std::env::var_os("CRAXII_STAGE18_CHILD_ROOT") else {
        return;
    };
    let program = std::env::var("CRAXII_STAGE18_CHILD_PROGRAM").unwrap();
    test_failpoints::initialize_controlled_process().expect("initialize Stage 18 child failpoint");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut cancellation_gate = None;
        let provider_programs = match program.as_str() {
            "answer" => programs(&[ProgramPlan::Answer {
                text: "subprocess answer".to_owned(),
                require_tool_result: None,
            }]),
            "tool" => programs(&[
                ProgramPlan::Tools(vec![ToolPlan::new(
                    "stable-effect",
                    "run_shell",
                    json!({"command": "printf 'stable-tool-execution\\n' >> stage18-effect.log; sync"}),
                )]),
                ProgramPlan::Answer {
                    text: "tool completed".to_owned(),
                    require_tool_result: Some(ModelToolCallId::try_new("stable-effect").unwrap()),
                },
            ]),
            "cancel" => {
                let gate = ScriptGate::new();
                let provider_programs = gated_answer_program("never completed", gate.clone(), false);
                cancellation_gate = Some(gate);
                provider_programs
            }
            other => panic!("unknown child program {other}"),
        };
        let harness = Stage18Harness::start(
            Stage18Root::from_existing(PathBuf::from(root)),
            provider_programs,
            EstimatorMode::Normal,
        )
        .await
        .unwrap();
        if cancellation_gate.is_some() {
            let response = harness
                .submit_message("cancel the subprocess crash scenario", client_id())
                .await;
            let work: WorkId = response.json()["work_id"].as_str().unwrap().parse().unwrap();
            wait_for_state(&harness.root.database(), work, "waiting_on_model").await;
            let _ = harness.cancel_work(work, command_id()).await;
        } else {
            harness
                .submit_message_losing_response(
                    "execute the subprocess crash scenario",
                    client_id(),
                )
                .await;
        }
        if std::env::var_os("CRAXII_STAGE18_CHILD_FOLLOWER").is_some() {
            harness
                .submit_message_losing_response(
                    "follower accepted only after predecessor",
                    client_id(),
                )
                .await;
            let marker_path = harness.root.path().join("follower-committed.marker");
            let marker = std::fs::File::create(marker_path).unwrap();
            marker.sync_all().unwrap();
            std::fs::File::open(harness.root.path())
                .unwrap()
                .sync_all()
                .unwrap();
        }
        std::future::pending::<()>().await;
    });
}

fn run_crash_child(root: &Path, case: CrashCase) -> Result<(), String> {
    run_crash_child_mode(root, case, false)
}

fn run_crash_child_mode(root: &Path, case: CrashCase, follower: bool) -> Result<(), String> {
    let resolved = test_failpoints::resolve_architecture_alias(
        case.alias.as_str(),
        Some(case.physical.as_str()),
    )
    .map_err(|error| format!("resolve failpoint: {error:?}"))?;
    let selection = ControlSelection {
        architecture_name: Some(case.alias),
        physical_hook: resolved.physical_hook,
        boundary: resolved.boundary,
    };
    let run_id = format!("run-stage18-{}", case.label);
    let control = selection
        .encode(&run_id)
        .map_err(|error| error.to_string())?;
    let (controller_marker, child_marker) =
        UnixStream::pair().map_err(|error| error.to_string())?;
    let child_marker = if child_marker.as_raw_fd() == MARKER_FILE_DESCRIPTOR {
        let replacement = child_marker
            .try_clone()
            .map_err(|error| error.to_string())?;
        drop(child_marker);
        replacement
    } else {
        child_marker
    };
    let source_descriptor = child_marker.as_raw_fd();
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .args(["--exact", "stage18_subprocess_child", "--nocapture"])
        .env("CRAXII_STAGE18_CHILD_ROOT", root)
        .env(
            "CRAXII_STAGE18_CHILD_PROGRAM",
            match case.program {
                CrashProgram::Answer => "answer",
                CrashProgram::Tool => "tool",
                CrashProgram::Cancel => "cancel",
            },
        );
    if follower {
        command.env("CRAXII_STAGE18_CHILD_FOLLOWER", "1");
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: `dup2` is async-signal-safe and copies a live UnixStream descriptor
    // to the fixed failpoint marker descriptor immediately before exec.
    unsafe {
        command.pre_exec(move || {
            unsafe extern "C" {
                fn dup2(old_descriptor: i32, new_descriptor: i32) -> i32;
            }
            if dup2(source_descriptor, MARKER_FILE_DESCRIPTOR) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    drop(child_marker);
    let mut stdin = child.stdin.take().ok_or("missing child stdin")?;
    stdin
        .write_all(control.as_bytes())
        .and_then(|()| stdin.flush())
        .map_err(|error| error.to_string())?;
    drop(stdin);
    controller_marker
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    let mut marker = String::new();
    let read = BufReader::new(controller_marker).read_line(&mut marker);
    if let Err(error) = read {
        kill_and_reap(&mut child);
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        return Err(format!(
            "{} marker read: {error}; child stderr: {stderr}",
            case.label
        ));
    }
    let value: Value = serde_json::from_str(marker.trim()).map_err(|error| error.to_string())?;
    if value["physical_hook"] != case.physical.as_str() || value["sequence"] != 1 {
        kill_and_reap(&mut child);
        return Err(format!("{} wrong marker: {marker}", case.label));
    }
    if follower {
        let follower_marker = root.join("follower-committed.marker");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !follower_marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !follower_marker.exists() {
            kill_and_reap(&mut child);
            return Err("follower command did not commit before crash".to_owned());
        }
    }
    let pid = i32::try_from(child.id()).map_err(|_| "child pid overflow")?;
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: the PID is the live direct child and SIGKILL is scoped to that child only.
    if unsafe { kill(pid, 9) } != 0 {
        kill_and_reap(&mut child);
        return Err(std::io::Error::last_os_error().to_string());
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if status.signal() != Some(9) {
        return Err(format!(
            "{} was not reaped after SIGKILL: {status}",
            case.label
        ));
    }
    Ok(())
}

fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn only_work_id(database: &Path) -> WorkId {
    let mut connection = open_read(database).await;
    let value: String = sqlx::query_scalar("SELECT work_id FROM work_items")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value.parse().unwrap()
}

async fn crash_work_projection(database: &Path) -> (String, String) {
    let mut connection = open_read(database).await;
    let (state, reason): (String, Option<String>) =
        sqlx::query_as("SELECT state, terminal_reason_code FROM work_items")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    let combined = reason.map_or_else(|| state.clone(), |reason| format!("{state}:{reason}"));
    (state, combined)
}

async fn latest_state(database: &Path, table: &'static str) -> Option<String> {
    assert!(matches!(table, "model_invocations" | "tool_executions"));
    let sql = format!("SELECT state FROM {table} LIMIT 1");
    let mut connection = open_read(database).await;
    let value = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_optional(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

async fn wait_for_state(database: &Path, work_id: WorkId, expected: &str) {
    for _ in 0..1_000 {
        let state = stage18_harness::query_string(
            database,
            "SELECT state FROM work_items WHERE work_id = ?",
            work_id.to_string(),
        )
        .await;
        if state.as_deref() == Some(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Work {work_id} did not reach {expected}");
}

async fn wait_for_table_state(database: &Path, table: &'static str, expected: &str) {
    for _ in 0..1_000 {
        if latest_state(database, table).await.as_deref() == Some(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{table} did not reach {expected}");
}

async fn wait_for_provider_calls(harness: &Stage18Harness, expected: u64) {
    for _ in 0..1_000 {
        if harness.provider.invocation_count() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("provider did not reach {expected} calls");
}

async fn work_reason(database: &Path, work_id: WorkId) -> Option<String> {
    stage18_harness::query_string(
        database,
        "SELECT terminal_reason_code FROM work_items WHERE work_id = ?",
        work_id.to_string(),
    )
    .await
}
