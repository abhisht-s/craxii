#![cfg(target_os = "macos")]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use craxii_server::adapters::scripted_provider::ScriptGate;
use craxii_server::domain::ClientMessageId;
use serde_json::{Value, json};
use sqlx::{Connection as _, Row as _};
use stage18_harness::{EstimatorMode, Stage18Harness, Stage18Root, gated_answer_program};

const LIVE_ENV: &str = "CRAXII_STAGE26_CANCELLATION";
const CONTROL_ENV: &str = "CRAXII_STAGE26_CONTROL_URL";

#[test]
fn stage26_wrapper_is_opt_in_and_exposes_no_provider_secret_interface() {
    let wrapper = include_str!("../../scripts/verify-stage26-native-local");
    for marker in [
        "CRAXII_STAGE26_LIVE",
        "gpt-5.6-luna",
        "stage26_native_live.py",
        "deterministic_native_cancellation_smoke",
        "test-without-building",
    ] {
        assert!(
            wrapper.contains(marker),
            "Stage 26 wrapper omitted {marker}"
        );
    }
    assert!(!wrapper.contains("--api-key"));
    let output = std::process::Command::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/verify-stage26-native-local"
    ))
    .env_remove("CRAXII_STAGE26_LIVE")
    .output()
    .expect("run Stage 26 wrapper without opt-in");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("LIVE_OPT_IN_REQUIRED"));
}

#[test]
fn stage26_native_xcui_path_is_separate_from_fixture_activation() {
    let source = include_str!("../../clients/macos/CraxiiUITests/CraxiiUITests.swift");
    assert!(source.contains("testStage26LiveNativeLunaRestartRelaunchAndFollowUp"));
    assert!(source.contains("testStage26DeterministicNativeCancellationSmoke"));
    assert!(source.contains("assertFixtureActivationAbsent"));
    let stage26 = source
        .split_once("func testStage26LiveNativeLunaRestartRelaunchAndFollowUp")
        .unwrap()
        .1;
    assert!(!stage26.contains("launchEnvironment[\"CRAXII_STAGE22_UI_SMOKE\"]"));
    assert!(!stage26.contains("launchEnvironment[\"CRAXII_STAGE21_UI_SMOKE\"]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by scripts/verify-stage26-native-local after the one-time Debug build"]
async fn deterministic_native_cancellation_smoke() {
    assert_eq!(std::env::var(LIVE_ENV).as_deref(), Ok("1"));
    for name in [
        "OPENAI_API_KEY",
        "OPENAI_KEY",
        "CRAXII_OPENAI_API_KEY",
        "CRAXII_STAGE25_OPENAI_API_KEY",
        "CRAXII_STAGE22_UI_SMOKE",
        "CRAXII_STAGE21_UI_SMOKE",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "forbidden Stage 26 environment: {name}"
        );
    }
    let repository = required_directory("CRAXII_STAGE26_REPOSITORY");
    let derived = required_directory("CRAXII_STAGE26_DERIVED_DATA");
    let report_path = required_new_path("CRAXII_STAGE26_CANCELLATION_REPORT_PATH");
    let xcresult = required_new_path("CRAXII_STAGE26_CANCELLATION_XCRESULT_PATH");
    let architecture = std::env::var("CRAXII_STAGE26_ARCH").expect("Stage 26 architecture");

    let gate = ScriptGate::new();
    let scripted = gated_answer_program("this answer must never commit", gate.clone(), false);
    let harness = Stage18Harness::start(
        Stage18Root::new("stage26-native-cancellation"),
        scripted,
        EstimatorMode::Normal,
    )
    .await
    .expect("start deterministic Stage 26 backend composition");
    let database = harness.root.database();
    seed_blocking_predecessor(&harness, &database).await;
    let state_directory = harness.root.path().join("native-client-state");
    fs::create_dir(&state_directory).unwrap();
    fs::set_permissions(&state_directory, fs::Permissions::from_mode(0o700)).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let control_authority = listener.local_addr().unwrap();
    let control_key = uuid::Uuid::now_v7().simple().to_string();
    let endpoint = format!("http://{}/", harness.authority);
    let bearer = harness.bearer.clone();
    let setup = json!({
        "endpoint": endpoint,
        "credential": bearer,
        "state_directory": state_directory,
    });
    let status = Arc::new(Mutex::new(None::<(String, String)>));
    let observations = Arc::new(Mutex::new(Vec::<Value>::new()));
    let complete = Arc::new(AtomicBool::new(false));
    let controller = spawn_controller(
        listener,
        control_key.clone(),
        setup,
        Arc::clone(&status),
        Arc::clone(&observations),
        Arc::clone(&complete),
    );

    let watcher_status = Arc::clone(&status);
    let watcher_database = database.clone();
    let watcher = tokio::spawn(async move {
        loop {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&watcher_database)
                .read_only(true);
            if let Ok(mut connection) = sqlx::SqliteConnection::connect_with(&options).await {
                let row = sqlx::query(
                    "SELECT work_id, state FROM work_items ORDER BY created_at DESC LIMIT 1",
                )
                .fetch_optional(&mut connection)
                .await
                .ok()
                .flatten();
                let _ = connection.close().await;
                if let Some(row) = row {
                    *watcher_status.lock().unwrap() = Some((
                        row.get::<String, _>("work_id"),
                        row.get::<String, _>("state"),
                    ));
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let log_path = report_path
        .parent()
        .unwrap()
        .join("stage26-cancellation-xcui.log");
    let log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log_path)
        .unwrap();
    let xctestrun = prepare_xctestrun(
        &repository,
        &derived,
        "cancellation",
        &format!("http://{control_authority}/{control_key}/"),
    );
    let mut command = tokio::process::Command::new("/usr/bin/xcodebuild");
    command
        .args([
            "test-without-building",
            "-xctestrun",
            xctestrun.to_str().unwrap(),
            "-destination",
            &format!("platform=macOS,arch={architecture}"),
            "-resultBundlePath",
            xcresult.to_str().unwrap(),
            "-only-testing:CraxiiUITests/CraxiiUITests/testStage26DeterministicNativeCancellationSmoke",
        ])
        .current_dir(&repository)
        .env(LIVE_ENV, "1")
        .env(
            CONTROL_ENV,
            format!("http://{control_authority}/{control_key}/"),
        )
        .env_remove("CRAXII_STAGE26_LIVE")
        .env_remove("CRAXII_STAGE22_UI_SMOKE")
        .env_remove("CRAXII_STAGE21_UI_SMOKE")
        .env_remove("CRAXII_STAGE21_INTEGRATION")
        .env_remove("CRAXII_STAGE22_INTEGRATION")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_KEY")
        .env_remove("CRAXII_OPENAI_API_KEY")
        .env_remove("CRAXII_STAGE25_OPENAI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log));
    command.kill_on_drop(true);
    let mut child = command.spawn().expect("spawn Stage 26 cancellation XCUI");
    let child_status = match tokio::time::timeout(Duration::from_secs(3 * 60), child.wait()).await {
        Ok(status) => status.expect("wait for Stage 26 cancellation XCUI"),
        Err(_) => {
            let _ = child.kill().await;
            terminate_owned_debug_app(&derived);
            let _ = fs::remove_file(&xctestrun);
            panic!("Stage 26 cancellation XCUI timed out");
        }
    };
    let _ = fs::remove_file(&xctestrun);
    watcher.abort();
    let controller_deadline = Instant::now() + Duration::from_secs(2);
    while !complete.load(Ordering::SeqCst) && Instant::now() < controller_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let final_status = status.lock().unwrap().clone();
    let native_observations = observations.lock().unwrap().clone();
    let captures = harness.provider.captures();
    gate.release();
    assert!(
        child_status.success(),
        "Stage 26 cancellation XCUI failed; see retained log"
    );
    assert!(
        complete.load(Ordering::SeqCst),
        "XCUI omitted completion handshake"
    );
    let (work_id, work_state) = final_status.expect("native cancellation created no work");
    assert_eq!(work_state, "cancelled");
    assert_eq!(captures.len(), 1);
    assert!(!captures[0].cancellation_observed());
    assert!(native_observations.iter().any(|value| {
        value["phase"] == "deterministic_cancellation"
            && value["row_identifier"] == format!("work.row.{work_id}")
    }));

    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .read_only(true),
    )
    .await
    .unwrap();
    let message_commands: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM client_commands WHERE command_type = 'message'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let cancel_commands: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM client_commands WHERE command_type = 'cancel'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let cancellation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? \
         AND event_type IN ('work.cancel_requested','work.cancelled')",
    )
    .bind(&work_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let cancelled_model_invocations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM model_invocations WHERE work_id = ?")
            .bind(&work_id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let terminal_assistants: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE produced_by_work_id = ?")
            .bind(&work_id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    assert_eq!(message_commands, 2);
    assert_eq!(cancel_commands, 1);
    assert!(cancellation_events >= 1);
    assert_eq!(cancelled_model_invocations, 0);
    assert_eq!(terminal_assistants, 0);

    let root = harness.shutdown().await;
    let evidence_root = root.preserve();
    let profile_id = profile_id(&state_directory);
    let keychain_cleanup = profile_id.map_or("profile_not_available", |profile| {
        let status = std::process::Command::new("/usr/bin/security")
            .args([
                "delete-generic-password",
                "-s",
                "com.craxii.device-token.v1",
                "-a",
                &profile,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        if status.success() {
            "deleted"
        } else {
            "already_absent"
        }
    });
    let report = json!({
        "status": "passed",
        "backend_composition": "deterministic_stage18_real_http_cancellation",
        "provider_spend": false,
        "real_native_swiftui_controls": true,
        "real_client_transport": true,
        "real_keychain": true,
        "work_id": work_id,
        "native_message_command_count": message_commands - 1,
        "blocking_fixture_message_command_count": 1,
        "cancellation_command_count": cancel_commands,
        "durable_cancel_event_count": cancellation_events,
        "terminal_state": work_state,
        "cancelled_work_model_invocation_count": cancelled_model_invocations,
        "blocking_fixture_provider_invocation_count": captures.len(),
        "assistant_message_count": terminal_assistants,
        "relaunch_projection_remained_cancelled": true,
        "native_observations": native_observations,
        "fixture_ui_state_used": false,
        "evidence_root": evidence_root,
        "xcresult": xcresult,
        "keychain_cleanup": keychain_cleanup,
    });
    write_report(&report_path, &report, bearer.as_bytes());
    assert_secret_absent(&evidence_root, bearer.as_bytes());
    assert_secret_absent(&xcresult, bearer.as_bytes());
    assert_secret_absent(&log_path, bearer.as_bytes());
    assert_secret_absent(&report_path, bearer.as_bytes());
    drop(controller);
}

async fn seed_blocking_predecessor(harness: &Stage18Harness, database: &Path) {
    let client_message_id: ClientMessageId = uuid::Uuid::now_v7().to_string().parse().unwrap();
    let response = harness
        .submit_message(
            "hold the deterministic Stage 26 predecessor",
            client_message_id,
        )
        .await;
    assert!(matches!(response.status, 200 | 202));
    let work_id = response.json()["work_id"].as_str().unwrap().to_owned();
    for _ in 0..1_000 {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(database)
            .read_only(true);
        if let Ok(mut connection) = sqlx::SqliteConnection::connect_with(&options).await {
            let state: Option<String> =
                sqlx::query_scalar("SELECT state FROM work_items WHERE work_id = ?")
                    .bind(&work_id)
                    .fetch_optional(&mut connection)
                    .await
                    .unwrap();
            let _ = connection.close().await;
            if state.as_deref() == Some("waiting_on_model") {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Stage 26 predecessor never blocked in the model invocation");
}

fn prepare_xctestrun(repository: &Path, derived: &Path, mode: &str, control_url: &str) -> PathBuf {
    let output = std::process::Command::new("python3")
        .arg(repository.join("scripts/support/stage26_xctestrun.py"))
        .args(["--derived", derived.to_str().unwrap()])
        .args(["--mode", mode])
        .args(["--control-url", control_url])
        .stdin(Stdio::null())
        .output()
        .expect("prepare scoped Stage 26 xctestrun");
    assert!(
        output.status.success(),
        "Stage 26 xctestrun preparation failed"
    );
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("Stage 26 xctestrun path UTF-8")
            .trim(),
    )
}

fn terminate_owned_debug_app(derived: &Path) {
    let unresolved = derived.join("Build/Products/Debug/Craxii.app/Contents/MacOS/Craxii");
    let executable = fs::canonicalize(&unresolved).unwrap_or(unresolved);
    let pattern = format!("^{}$", executable.display());
    let Ok(output) = std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", &pattern])
        .stdin(Stdio::null())
        .output()
    else {
        return;
    };
    for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-TERM", pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn spawn_controller(
    listener: TcpListener,
    key: String,
    setup: Value,
    status: Arc<Mutex<Option<(String, String)>>>,
    observations: Arc<Mutex<Vec<Value>>>,
    complete: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut setup_served = false;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Ok((method, route, body)) = read_request(&mut stream) else {
                respond(&mut stream, 400, None);
                continue;
            };
            let expected_prefix = format!("/{key}/");
            let Some(route) = route.strip_prefix(&expected_prefix) else {
                respond(&mut stream, 404, None);
                continue;
            };
            match (method.as_str(), route) {
                ("GET", "setup") if !setup_served => {
                    setup_served = true;
                    respond(&mut stream, 200, Some(&setup));
                }
                ("GET", "cancellation") => {
                    let current = status.lock().unwrap().clone();
                    if let Some((work_id, state)) =
                        current.filter(|(_, state)| state == "cancelled")
                    {
                        respond(
                            &mut stream,
                            200,
                            Some(&json!({"work_id": work_id, "state": state})),
                        );
                    } else {
                        respond(&mut stream, 425, Some(&json!({"pending": true})));
                    }
                }
                ("POST", "observation") => match serde_json::from_slice::<Value>(&body) {
                    Ok(value) => {
                        observations.lock().unwrap().push(value);
                        respond(&mut stream, 204, None);
                    }
                    Err(_) => respond(&mut stream, 400, None),
                },
                ("POST", "complete") => {
                    complete.store(true, Ordering::SeqCst);
                    respond(&mut stream, 204, None);
                    break;
                }
                _ => respond(&mut stream, 404, None),
            }
        }
    })
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), ()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| ())?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2048];
    let header_end;
    loop {
        let count = stream.read(&mut buffer).map_err(|_| ())?;
        if count == 0 || bytes.len() + count > 8192 {
            return Err(());
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }
    let headers = std::str::from_utf8(&bytes[..header_end]).map_err(|_| ())?;
    let mut lines = headers.lines();
    let mut request = lines.next().ok_or(())?.split_ascii_whitespace();
    let method = request.next().ok_or(())?.to_owned();
    let route = request.next().ok_or(())?.to_owned();
    let content_length = lines
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or(0);
    if content_length > 4096 {
        return Err(());
    }
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer).map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok((
        method,
        route,
        bytes[header_end..header_end + content_length].to_vec(),
    ))
}

fn respond(stream: &mut TcpStream, status: u16, value: Option<&Value>) {
    let body = value.map_or_else(Vec::new, |value| serde_json::to_vec(value).unwrap());
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        425 => "Too Early",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&body);
}

fn required_directory(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} required")));
    assert!(path.is_absolute() && path.is_dir());
    path
}

fn required_new_path(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} required")));
    assert!(path.is_absolute() && !path.exists());
    let parent = path.parent().unwrap();
    assert!(parent.is_dir());
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    path
}

fn profile_id(state_directory: &Path) -> Option<String> {
    let bytes = fs::read(state_directory.join("client-state-v1.json")).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["profile"]["profileID"].as_str().map(str::to_owned)
}

fn write_report(path: &Path, value: &Value, bearer: &[u8]) {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    assert!(!contains(&bytes, bearer));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&bytes).unwrap();
}

fn assert_secret_absent(path: &Path, bearer: &[u8]) {
    if path.is_file() {
        assert!(!contains(&fs::read(path).unwrap(), bearer));
        return;
    }
    scan_directory(path, bearer);
}

fn scan_directory(path: &Path, bearer: &[u8]) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            scan_directory(&entry.path(), bearer);
        } else if kind.is_file() {
            assert!(!contains(&fs::read(entry.path()).unwrap(), bearer));
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}
