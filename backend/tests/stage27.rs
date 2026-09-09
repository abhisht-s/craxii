#![cfg(target_os = "linux")]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use craxii_server::domain::{ClientCommandId, ClientMessageId, ModelToolCallId, WorkId};
use serde_json::json;
use sqlx::{Connection as _, Row as _};
use stage18_harness::{
    EstimatorMode, ProgramPlan, Stage18Harness, Stage18Root, ToolPlan, programs_for_user_messages,
    query_string,
};

const LIVE_HOST_ENV: &str = "CRAXII_STAGE27_LIVE_HOST";
const DEPLOYMENT_COMMIT_ENV: &str = "CRAXII_STAGE27_DEPLOYMENT_COMMIT";
const LAUNCHER_ENV: &str = "CRAXII_STAGE27_USER_SWITCH_LAUNCHER";
const CGROUP_ROOT_ENV: &str = "CRAXII_STAGE27_CGROUP_ROOT";
const RESTART_READY_ENV: &str = "CRAXII_STAGE27_RESTART_READY";
const RESTART_DONE_ENV: &str = "CRAXII_STAGE27_RESTART_DONE";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the production Stage 27 identities, launcher, and delegated service cgroup"]
async fn live_linux_cancellation_cleans_process_tree_and_preserves_follower() {
    require_live_host();
    let launcher = required_path(LAUNCHER_ENV);
    let cgroup_root = required_path(CGROUP_ROOT_ENV);
    assert_execution_cgroups_empty(&cgroup_root);

    let root = Stage18Root::new("stage27-live-cancellation");
    root.allow_disposable_workstation_identity();
    let workspace = root.workspace();
    let canonical_workspace = fs::canonicalize(&workspace).unwrap();
    let ordinary_message = "run disposable ordinary Linux workload";
    let cancellation_message = "start disposable cancellation workload";
    let follower_message = "following work must remain runnable";
    let ordinary_call = ModelToolCallId::try_new("stage27-ordinary-shell").unwrap();
    let plans = [
        ProgramPlan::Tools(vec![ToolPlan::new(
            ordinary_call.as_str(),
            "run_shell",
            json!({
                "command": "test \"$(umask)\" = 0077; test \"$(ulimit -n)\" = 65536; test \"$(ulimit -u)\" = 16384; test \"$(ulimit -c)\" = 0; set -- $(/usr/bin/ps -o pid= -o pgid= -o sid= -p $$); test \"$1\" = \"$2\"; test \"$1\" = \"$3\"; /usr/bin/grep -Eq '^Groups:[[:space:]]*$' /proc/self/status; for field in CapInh CapPrm CapEff CapBnd CapAmb; do /usr/bin/grep -Eq \"^${field}:[[:space:]]*0+$\" /proc/self/status; done; /usr/bin/grep -Eq '^NoNewPrivs:[[:space:]]*1$' /proc/self/status; /usr/bin/grep -Eq '^0::/system.slice/craxii-server.service/craxii-executions/[0-9a-f-]+$' /proc/self/cgroup; test \"$HOME:$USER:$LOGNAME\" = /home/craxii:craxii:craxii; test ! -r /run/credentials/craxii-server.service/openai_provider; for fd in /proc/self/fd/*; do case \"${fd##*/}\" in 0|1|2) ;; *) test ! -e \"$fd\" || exit 1 ;; esac; done; for name in CREDENTIALS_DIRECTORY OPENAI_API_KEY AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_PROFILE AWS_SHARED_CREDENTIALS_FILE AWS_WEB_IDENTITY_TOKEN_FILE AWS_CONTAINER_CREDENTIALS_RELATIVE_URI AWS_CONTAINER_CREDENTIALS_FULL_URI; do test -z \"${!name-}\"; done; test -x /usr/bin/curl; ! /usr/bin/curl --noproxy '*' --silent --show-error --connect-timeout 1 --max-time 2 http://169.254.169.254/ >/dev/null 2>&1; ! /usr/bin/curl --noproxy '*' --silent --show-error --connect-timeout 1 --max-time 2 'http://[fd00:ec2::254]/' >/dev/null 2>&1; umask 022; /bin/sleep 0.1 & wait && printf '%s:%s\\n%s\\n' \"$(/usr/bin/id -un)\" \"$(/usr/bin/id -gn)\" \"$(/usr/bin/pwd -P)\" | /usr/bin/tee ordinary-execution-identity"
            }),
        )]),
        ProgramPlan::Answer {
            text: "ordinary Linux workload completed".to_owned(),
            require_tool_result: Some(ordinary_call),
        },
        ProgramPlan::Tools(vec![ToolPlan::new(
            "stage27-cancel-shell",
            "run_shell",
            json!({
                "command": "trap 'printf term > cancellation-term-observed; exit 0' TERM; /bin/sleep 300 & printf '%s' \"$!\" > cancellation-descendant.pid; printf started > cancellation-started; while :; do /bin/sleep 1; done; printf late > cancellation-late-side-effect"
            }),
        )]),
        ProgramPlan::Answer {
            text: "following work completed after cancelled predecessor".to_owned(),
            require_tool_result: None,
        },
    ];
    let harness = Stage18Harness::start_with_linux_workstation(
        root,
        programs_for_user_messages(
            &plans,
            &[
                ordinary_message,
                ordinary_message,
                cancellation_message,
                follower_message,
            ],
        ),
        EstimatorMode::Normal,
        launcher,
        cgroup_root.clone(),
    )
    .await
    .expect("start Stage 27 Linux cancellation harness");

    let ordinary_work = submit(&harness, ordinary_message).await;
    assert_eq!(harness.wait_terminal(ordinary_work).await, "completed");
    let expected_ordinary_output = format!("craxii:craxii\n{}\n", canonical_workspace.display());
    let mut connection = read_only(&harness.root.database()).await;
    let ordinary_tools = sqlx::query(
        "SELECT state, requested_cwd, resolved_cwd, effective_privilege, exit_code, signal, \
         timed_out, cancelled, cleanup_confirmed, result_json, stdout_observed_bytes, \
         stdout_captured_bytes, stdout_returned_inline_bytes, stderr_observed_bytes, \
         stderr_captured_bytes, stderr_returned_inline_bytes \
         FROM tool_executions WHERE work_id = ?",
    )
    .bind(ordinary_work.to_string())
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(ordinary_tools.len(), 1);
    let ordinary_tool = &ordinary_tools[0];
    assert_eq!(ordinary_tool.get::<String, _>("state"), "completed");
    assert_eq!(
        ordinary_tool.get::<String, _>("requested_cwd"),
        canonical_workspace.to_str().unwrap()
    );
    assert_eq!(
        ordinary_tool.get::<Option<String>, _>("resolved_cwd"),
        Some(canonical_workspace.to_str().unwrap().to_owned())
    );
    assert_eq!(
        ordinary_tool.get::<Option<String>, _>("effective_privilege"),
        Some("user".to_owned())
    );
    assert_eq!(ordinary_tool.get::<Option<i64>, _>("exit_code"), Some(0));
    assert_eq!(ordinary_tool.get::<Option<i64>, _>("signal"), None);
    assert_eq!(ordinary_tool.get::<Option<i64>, _>("timed_out"), Some(0));
    assert_eq!(ordinary_tool.get::<Option<i64>, _>("cancelled"), Some(0));
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("cleanup_confirmed"),
        Some(1)
    );
    let expected_stdout_bytes = i64::try_from(expected_ordinary_output.len()).unwrap();
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stdout_observed_bytes"),
        Some(expected_stdout_bytes)
    );
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stdout_captured_bytes"),
        Some(expected_stdout_bytes)
    );
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stdout_returned_inline_bytes"),
        Some(expected_stdout_bytes)
    );
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stderr_observed_bytes"),
        Some(0)
    );
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stderr_captured_bytes"),
        Some(0)
    );
    assert_eq!(
        ordinary_tool.get::<Option<i64>, _>("stderr_returned_inline_bytes"),
        Some(0)
    );
    let result: serde_json::Value = serde_json::from_str(
        &ordinary_tool
            .get::<Option<String>, _>("result_json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["result_kind"], "success");
    let fields = result["fields"].as_array().unwrap();
    let stdout = fields.iter().find_map(|field| {
        let pair = field.as_array()?;
        (pair.first()?.as_str()? == "stdout_0001")
            .then(|| pair.get(1)?.as_str())
            .flatten()
    });
    assert_eq!(stdout, Some(expected_ordinary_output.as_str()));
    connection.close().await.unwrap();
    assert_eq!(
        fs::read_to_string(workspace.join("ordinary-execution-identity")).unwrap(),
        expected_ordinary_output
    );
    wait_for_empty_execution_cgroups(&cgroup_root).await;

    let cancelled_work = submit(&harness, cancellation_message).await;
    wait_for_path(&workspace.join("cancellation-started")).await;
    let follower = submit(&harness, follower_message).await;
    wait_for_work_state(&harness, follower, "queued").await;

    let response = harness.cancel_work(cancelled_work, command_id()).await;
    assert!(matches!(response.status, 200 | 202));
    assert_eq!(harness.wait_terminal(cancelled_work).await, "cancelled");
    assert_eq!(harness.wait_terminal(follower).await, "completed");

    let mut connection = read_only(&harness.root.database()).await;
    let work: (String, Option<String>) =
        sqlx::query_as("SELECT state, terminal_reason_code FROM work_items WHERE work_id = ?")
            .bind(cancelled_work.to_string())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        work,
        ("cancelled".to_owned(), Some("user_request".to_owned()))
    );
    let tool = sqlx::query(
        "SELECT state, cancelled, timed_out, cleanup_confirmed FROM tool_executions WHERE work_id = ?",
    )
    .bind(cancelled_work.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(tool.get::<String, _>("state"), "completed");
    assert_eq!(tool.get::<Option<i64>, _>("cancelled"), Some(1));
    assert_eq!(tool.get::<Option<i64>, _>("timed_out"), Some(0));
    assert_eq!(tool.get::<Option<i64>, _>("cleanup_confirmed"), Some(1));
    let cancellation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? AND event_type IN ('work.cancel_requested','work.cancelled')",
    )
    .bind(cancelled_work.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(cancellation_events, 2);
    connection.close().await.unwrap();

    assert_eq!(
        fs::read_to_string(workspace.join("cancellation-term-observed")).unwrap(),
        "term"
    );
    assert!(!workspace.join("cancellation-late-side-effect").exists());
    let descendant = fs::read_to_string(workspace.join("cancellation-descendant.pid"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(!Path::new(&format!("/proc/{descendant}")).exists());
    assert_eq!(harness.provider.invocation_count(), 4);
    assert_eq!(harness.provider.remaining_programs(), 0);
    wait_for_empty_execution_cgroups(&cgroup_root).await;

    let root = harness.shutdown().await;
    assert_execution_cgroups_empty(&cgroup_root);
    root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the production Stage 27 identities, launcher, and delegated service cgroup"]
async fn live_linux_terminal_outcome_matrix_is_canonical() {
    require_live_host();
    let launcher = required_path(LAUNCHER_ENV);
    let cgroup_root = required_path(CGROUP_ROOT_ENV);
    assert_execution_cgroups_empty(&cgroup_root);
    let root = Stage18Root::new("stage27-live-outcome-matrix");
    root.allow_disposable_workstation_identity();
    let messages = [
        "run disposable nonzero workload",
        "run disposable signal workload",
        "run disposable timeout workload",
    ];
    let call_ids = ["stage27-nonzero", "stage27-signal", "stage27-timeout"];
    let plans = [
        ProgramPlan::Tools(vec![ToolPlan::new(
            call_ids[0],
            "run_shell",
            json!({"command": "exit 23"}),
        )]),
        ProgramPlan::Answer {
            text: "nonzero evidence observed".to_owned(),
            require_tool_result: Some(ModelToolCallId::try_new(call_ids[0]).unwrap()),
        },
        ProgramPlan::Tools(vec![ToolPlan::new(
            call_ids[1],
            "run_shell",
            json!({"command": "kill -TERM $$"}),
        )]),
        ProgramPlan::Answer {
            text: "signal evidence observed".to_owned(),
            require_tool_result: Some(ModelToolCallId::try_new(call_ids[1]).unwrap()),
        },
        ProgramPlan::Tools(vec![ToolPlan::new(
            call_ids[2],
            "run_shell",
            json!({"command": "/bin/sleep 300", "timeout_seconds": 1}),
        )]),
        ProgramPlan::Answer {
            text: "timeout evidence observed".to_owned(),
            require_tool_result: Some(ModelToolCallId::try_new(call_ids[2]).unwrap()),
        },
    ];
    let harness = Stage18Harness::start_with_linux_workstation(
        root,
        programs_for_user_messages(
            &plans,
            &[
                messages[0],
                messages[0],
                messages[1],
                messages[1],
                messages[2],
                messages[2],
            ],
        ),
        EstimatorMode::Normal,
        launcher,
        cgroup_root.clone(),
    )
    .await
    .expect("start Stage 27 terminal outcome harness");

    let expected = [
        ("process_exit", Some(23_i64), None, Some(0_i64), Some(0_i64)),
        (
            "signal_termination",
            None,
            Some(15_i64),
            Some(0_i64),
            Some(0_i64),
        ),
        ("timeout", None, None, Some(1_i64), Some(0_i64)),
    ];
    for (message, expected) in messages.into_iter().zip(expected) {
        let work = submit(&harness, message).await;
        assert_eq!(harness.wait_terminal(work).await, "completed");
        let mut connection = read_only(&harness.root.database()).await;
        let row = sqlx::query(
            "SELECT state, dispatch_intent_at, started_at, completed_at, exit_code, signal, \
             timed_out, cancelled, cleanup_confirmed, result_json \
             FROM tool_executions WHERE work_id = ?",
        )
        .bind(work.to_string())
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("state"), "completed");
        assert!(row.get::<Option<String>, _>("dispatch_intent_at").is_some());
        assert!(row.get::<Option<String>, _>("started_at").is_some());
        assert!(row.get::<Option<String>, _>("completed_at").is_some());
        assert_eq!(row.get::<Option<i64>, _>("exit_code"), expected.1);
        if expected.0 != "timeout" {
            assert_eq!(row.get::<Option<i64>, _>("signal"), expected.2);
        }
        assert_eq!(row.get::<Option<i64>, _>("timed_out"), expected.3);
        assert_eq!(row.get::<Option<i64>, _>("cancelled"), expected.4);
        assert_eq!(row.get::<Option<i64>, _>("cleanup_confirmed"), Some(1));
        let result: serde_json::Value =
            serde_json::from_str(&row.get::<Option<String>, _>("result_json").unwrap()).unwrap();
        assert_eq!(result["result_kind"], expected.0);
        connection.close().await.unwrap();
        wait_for_empty_execution_cgroups(&cgroup_root).await;
    }
    assert_eq!(harness.provider.invocation_count(), 6);
    assert_eq!(harness.provider.remaining_programs(), 0);
    let root = harness.shutdown().await;
    assert_execution_cgroups_empty(&cgroup_root);
    root.remove();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "coordinated by the Stage 27 production service restart gate"]
async fn live_systemd_restart_kills_delegated_execution_but_not_verifier() {
    require_live_host();
    let launcher = required_path(LAUNCHER_ENV);
    let cgroup_root = required_path(CGROUP_ROOT_ENV);
    let ready = required_new_marker(RESTART_READY_ENV);
    let done = required_new_marker(RESTART_DONE_ENV);
    assert_execution_cgroups_empty(&cgroup_root);

    let root = Stage18Root::new("stage27-systemd-restart");
    root.allow_disposable_workstation_identity();
    let workspace = root.workspace();
    let restart_message = "start disposable service-cgroup workload";
    let plans = [
        ProgramPlan::Tools(vec![ToolPlan::new(
            "stage27-restart-shell",
            "run_shell",
            json!({
                "command": "/bin/sleep 300 & printf '%s' \"$!\" > restart-descendant.pid; printf started > restart-execution-started; wait"
            }),
        )]),
        ProgramPlan::Answer {
            text: "delegated execution ended during service restart".to_owned(),
            require_tool_result: None,
        },
    ];
    let harness = Stage18Harness::start_with_linux_workstation(
        root,
        programs_for_user_messages(&plans, &[restart_message, restart_message]),
        EstimatorMode::Normal,
        launcher,
        cgroup_root.clone(),
    )
    .await
    .expect("start Stage 27 systemd restart harness");
    let work = submit(&harness, restart_message).await;
    wait_for_path(&workspace.join("restart-execution-started")).await;
    fs::write(&ready, b"ready\n").unwrap();
    wait_for_path_with_timeout(&done, Duration::from_secs(90)).await;

    assert_eq!(harness.wait_terminal(work).await, "completed");
    let mut connection = read_only(&harness.root.database()).await;
    let tool = sqlx::query(
        "SELECT state, signal, timed_out, cancelled, cleanup_confirmed, result_json \
         FROM tool_executions WHERE work_id = ?",
    )
    .bind(work.to_string())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(tool.get::<String, _>("state"), "completed");
    assert_eq!(tool.get::<Option<i64>, _>("signal"), Some(15));
    assert_eq!(tool.get::<Option<i64>, _>("timed_out"), Some(0));
    assert_eq!(tool.get::<Option<i64>, _>("cancelled"), Some(0));
    assert_eq!(tool.get::<Option<i64>, _>("cleanup_confirmed"), Some(1));
    let result: serde_json::Value =
        serde_json::from_str(&tool.get::<Option<String>, _>("result_json").unwrap()).unwrap();
    assert_eq!(result["result_kind"], "signal_termination");
    assert_eq!(harness.provider.invocation_count(), 2);
    assert_eq!(harness.provider.remaining_programs(), 0);
    connection.close().await.unwrap();

    let descendant = fs::read_to_string(workspace.join("restart-descendant.pid"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(!Path::new(&format!("/proc/{descendant}")).exists());
    wait_for_empty_execution_cgroups(&cgroup_root).await;
    let root = harness.shutdown().await;
    assert_execution_cgroups_empty(&cgroup_root);
    root.remove();
}

fn require_live_host() {
    assert_eq!(std::env::var(LIVE_HOST_ENV).as_deref(), Ok("1"));
    let deployment_commit = std::env::var(DEPLOYMENT_COMMIT_ENV)
        .expect("Stage 27 deployed commit must be passed to the live test");
    assert_eq!(deployment_commit.len(), 40);
    assert!(
        deployment_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        env!("CRAXII_GIT_REVISION"),
        deployment_commit,
        "the compiled live test must come from the deployed release commit"
    );
    assert_eq!(env!("CRAXII_GIT_DIRTY"), "false");
    assert_eq!(std::env::consts::ARCH, "x86_64");
    assert_ne!(unsafe { nix::libc::geteuid() }, 0);
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let cap_eff = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .unwrap();
    assert_ne!(cap_eff & (1_u64 << 5), 0, "CAP_KILL is required");
    assert_ne!(
        cap_eff & (1_u64 << 21),
        0,
        "CAP_SYS_ADMIN is required by the credential-free verifier to migrate only its child across the cgroup delegation boundary"
    );
    let cgroup = fs::read_to_string("/proc/self/cgroup").unwrap();
    assert!(
        !cgroup.contains("/system.slice/craxii-server.service"),
        "the verifier must remain outside the service cgroup"
    );
    for forbidden in [
        "OPENAI_API_KEY",
        "CREDENTIALS_DIRECTORY",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
    ] {
        assert!(
            std::env::var_os(forbidden).is_none(),
            "forbidden test environment: {forbidden}"
        );
    }
}

fn required_path(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).expect("required Stage 27 path"));
    assert!(path.is_absolute());
    path
}

fn required_new_marker(name: &str) -> PathBuf {
    let path = required_path(name);
    assert!(!path.exists(), "coordination marker already exists");
    path
}

async fn submit(harness: &Stage18Harness, message: &str) -> WorkId {
    let response = harness.submit_message(message, client_id()).await;
    assert_eq!(response.status, 202);
    response.json()["work_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn client_id() -> ClientMessageId {
    ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

fn command_id() -> ClientCommandId {
    ClientCommandId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
}

async fn wait_for_work_state(harness: &Stage18Harness, work: WorkId, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if query_string(
            &harness.root.database(),
            "SELECT state FROM work_items WHERE work_id = ?",
            work.to_string(),
        )
        .await
        .as_deref()
            == Some(expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("work did not reach {expected}");
}

async fn wait_for_path(path: &Path) {
    wait_for_path_with_timeout(path, Duration::from_secs(10)).await;
}

async fn wait_for_path_with_timeout(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.is_file() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for disposable coordination marker");
}

async fn read_only(database: &Path) -> sqlx::SqliteConnection {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .foreign_keys(true);
    sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap()
}

fn execution_cgroup_directories(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

fn assert_execution_cgroups_empty(root: &Path) {
    assert!(root.is_dir(), "delegated execution cgroup root is absent");
    assert!(
        execution_cgroup_directories(root).is_empty(),
        "delegated execution cgroup root contains residue"
    );
}

async fn wait_for_empty_execution_cgroups(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if root.is_dir() && execution_cgroup_directories(root).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_execution_cgroups_empty(root);
}
