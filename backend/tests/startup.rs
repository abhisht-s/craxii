use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;

const LOCAL: &str = include_str!("fixtures/config/valid/local.toml");
const EC2: &str = include_str!("fixtures/config/valid/ec2-shape.toml");
const SECRET_SENTINEL: &str = "startup-secret-sentinel-XYZ";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn valid_local_config_emits_pretty_initial_live_unready_evidence() {
    let config = TempConfig::new(LOCAL);
    let output = run(&["--config", config.path().to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(output.stderr.is_empty());

    let stdout = text(&output.stdout);
    for expected in [
        "Craxii startup",
        "event_name=\"startup\"",
        "subsystem=\"bootstrap\"",
        "package_version=\"0.0.1\"",
        "git_revision=",
        "git_dirty=",
        "build_target=",
        "architecture_version=\"V0.0.01\"",
        "protocol_version=1",
        "configuration_version=1",
        "max_supported_schema_version=4",
        "configuration_fingerprint=\"sha256:",
        "health_state=\"live_unready\"",
        "live=true",
        "ready=false",
    ] {
        assert!(stdout.contains(expected), "missing {expected} in {stdout}");
    }
    assert!(!stdout.contains("ready=true"));
}

#[test]
fn ec2_shaped_config_emits_parseable_json_startup_evidence() {
    let config = TempConfig::new(EC2);
    let output = run(&["--config", config.path().to_str().unwrap()]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(output.stderr.is_empty());

    let stdout = text(&output.stdout);
    assert!(!stdout.contains(config.root.to_str().unwrap()));
    let records = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let operations = records
        .iter()
        .filter_map(|record| record["operation"].as_str())
        .collect::<Vec<_>>();
    for expected in [
        "database_open",
        "integrity_check",
        "migrate",
        "database_disposition",
    ] {
        assert!(
            operations.contains(&expected),
            "missing {expected} in {stdout}"
        );
    }
    let record = records
        .iter()
        .find(|record| record["event_name"] == "startup")
        .expect("startup evidence record must be present");
    assert_eq!(record["event_name"], "startup");
    assert_eq!(record["subsystem"], "bootstrap");
    assert_eq!(record["health_state"], "live_unready");
    assert_eq!(record["live"], true);
    assert_eq!(record["ready"], false);
    assert_eq!(record["evidence_role"], "operational_only");
    assert_eq!(record["recovery_truth"], false);
}

#[test]
fn error_filter_suppresses_startup_info_without_failing_startup() {
    let config = TempConfig::new(&LOCAL.replace("filter = \"info\"", "filter = \"error\""));
    let output = run(&["--config", config.path().to_str().unwrap()]);

    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn config_argument_is_required() {
    for arguments in [vec![], vec!["config.toml"], vec!["--config"]] {
        let output = run(&arguments);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(text(&output.stderr), "craxii fatal: invalid_cli\n");
    }
}

#[cfg(not(feature = "test-failpoints"))]
#[test]
fn default_binary_does_not_recognize_hidden_test_control() {
    let output = run(&["--test-failpoint-control-v1"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(text(&output.stderr), "craxii fatal: invalid_cli\n");
}

#[test]
fn invalid_config_is_redacted_and_exits_nonzero() {
    let config = TempConfig::new(&format!(
        "configuration_version = 1\nunknown_secret = \"{SECRET_SENTINEL}\"\n"
    ));
    let output = run(&["--config", config.path().to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "craxii fatal: invalid_configuration\n"
    );
    assert!(!text(&output.stderr).contains(SECRET_SENTINEL));
    assert!(!format!("{output:?}").contains(SECRET_SENTINEL));
}

#[test]
fn bind_failure_precedes_database_and_runtime_creation() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = occupied.local_addr().unwrap().to_string();
    let config = TempConfig::new_with_authority(LOCAL, &authority);
    let output = run(&["--config", config.path().to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(text(&output.stderr), "craxii fatal: server_bind_failure\n");
    assert!(!config.root.join("state/db/craxii.sqlite3").exists());
}

#[test]
fn unreadable_provider_credential_fails_with_a_fixed_redacted_code() {
    let root = temporary_root();
    fs::create_dir_all(&root).unwrap();
    let state_root = root.join("state");
    fs::create_dir(&state_root).unwrap();
    fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700)).unwrap();
    let credential_directory = root.join("credentials");
    fs::create_dir(&credential_directory).unwrap();
    let credential_file = credential_directory.join("openai_primary");
    fs::write(&credential_file, SECRET_SENTINEL).unwrap();
    fs::set_permissions(&credential_file, fs::Permissions::from_mode(0o000)).unwrap();
    let workspace_root = root.join("workspace");
    fs::create_dir(&workspace_root).unwrap();

    let input = LOCAL
        .replace(
            "bind_address = \"127.0.0.1:8080\"",
            &format!("bind_address = \"{}\"", available_loopback_authority()),
        )
        .replace(
            "artifact_root = \"/tmp/craxii-dev/state/artifacts\"",
            &format!(
                "artifact_root = \"{}\"",
                state_root.join("artifacts").to_str().unwrap()
            ),
        )
        .replace(
            "state_root = \"/tmp/craxii-dev/state\"",
            &format!("state_root = \"{}\"", state_root.to_str().unwrap()),
        )
        .replace(
            "/tmp/craxii-dev/credentials",
            credential_directory.to_str().unwrap(),
        )
        .replace(
            "primary_workspace_root = \"/tmp/craxii-dev/workspaces/primary\"",
            &format!(
                "primary_workspace_root = \"{}\"",
                workspace_root.to_str().unwrap()
            ),
        );
    let config_path = root.join("config.toml");
    fs::write(&config_path, input).unwrap();

    let output = run(&["--config", config_path.to_str().unwrap()]);
    fs::set_permissions(&credential_file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "craxii fatal: provider_credential_unavailable\n"
    );
    assert!(!text(&output.stdout).contains(SECRET_SENTINEL));
    assert!(!text(&output.stderr).contains(SECRET_SENTINEL));
}

#[test]
fn no_valid_startup_format_reports_ready() {
    for input in [LOCAL, EC2] {
        let config = TempConfig::new(input);
        let output = run(&["--config", config.path().to_str().unwrap()]);
        assert!(output.status.success());
        let stdout = text(&output.stdout);
        assert!(!stdout.contains("ready=true"));
        assert!(!stdout.contains("\"ready\":true"));
        assert!(!stdout.contains("health_state=ready"));
        assert!(!stdout.contains("\"health_state\":\"ready\""));
    }
}

#[test]
fn live_provider_composition_becomes_ready_after_scheduler_initial_scan() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    let config = TempConfig::new(LOCAL);
    let mut child = Command::new(env!("CARGO_BIN_EXE_craxii-server"))
        .args(["--config", config.path().to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("craxii-server binary should execute");
    let mut ready = false;
    for _ in 0..300 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if let Ok(mut stream) = TcpStream::connect(&config.authority) {
            stream
                .write_all(
                    format!(
                        "GET /health/ready HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                        config.authority
                    )
                    .as_bytes(),
                )
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            if response.starts_with("HTTP/1.1 200") {
                ready = true;
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    terminate(&mut child);
    let output = child.wait_with_output().unwrap();
    assert!(ready, "composition did not become ready: {}", text(&output.stderr));
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
}

fn run(arguments: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_craxii-server"))
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("craxii-server binary should execute");
    for _ in 0..200 {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Successful Stage 10 startup is a long-lived process. Exercise the real
    // composition-edge SIGTERM path so these startup assertions also wait for
    // a graceful RuntimeInstance close instead of leaving stale test state.
    terminate(&mut child);
    child.wait_with_output().unwrap()
}

fn terminate(child: &mut std::process::Child) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: `child.id()` is the live child we own and signal 15 is SIGTERM on every Unix target.
    assert_eq!(unsafe { kill(child.id() as i32, 15) }, 0);
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn temporary_root() -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "craxii-startup-test-{}-{sequence}",
        std::process::id()
    ))
}

struct TempConfig {
    root: PathBuf,
    path: PathBuf,
    authority: String,
}

impl TempConfig {
    fn new(contents: &str) -> Self {
        Self::new_with_authority(contents, &available_loopback_authority())
    }

    fn new_with_authority(contents: &str, authority: &str) -> Self {
        let root = temporary_root();
        fs::create_dir(&root).unwrap();
        let state_root = root.join("state");
        fs::create_dir(&state_root).unwrap();
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace_root = root.join("workspace");
        fs::create_dir(&workspace_root).unwrap();
        let credential_root = root.join("credentials");
        fs::create_dir(&credential_root).unwrap();
        for name in ["openai_primary", "openai_secondary", "openai_provider"] {
            let credential = credential_root.join(name);
            fs::write(&credential, "stage19-startup-fixture-key").unwrap();
            fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let contents = contents
            .replace(
                "bind_address = \"127.0.0.1:8080\"",
                &format!("bind_address = \"{authority}\""),
            )
            .replace(
                "public_base_url = \"http://127.0.0.1:8080\"",
                &format!("public_base_url = \"http://{authority}\""),
            );
        let contents = if contents.contains("source = \"systemd\"") {
            contents.replace(
                "source = \"systemd\"",
                &format!(
                    "source = \"local_directory\"\ndirectory = \"{}\"",
                    credential_root.to_str().unwrap()
                ),
            )
        } else {
            contents.replace(
                "directory = \"/tmp/craxii-dev/credentials\"",
                &format!("directory = \"{}\"", credential_root.to_str().unwrap()),
            )
        };
        let contents = match contents
            .lines()
            .find(|line| line.starts_with("state_root = "))
        {
            Some(state_root_line) => contents.replace(
                state_root_line,
                &format!("state_root = \"{}\"", state_root.to_str().unwrap()),
            ),
            None => contents.to_owned(),
        };
        let contents = match contents
            .lines()
            .find(|line| line.starts_with("artifact_root = "))
        {
            Some(artifact_root_line) => contents.replace(
                artifact_root_line,
                &format!(
                    "artifact_root = \"{}\"",
                    state_root.join("artifacts").to_str().unwrap()
                ),
            ),
            None => contents,
        };
        let contents = match contents
            .lines()
            .find(|line| line.starts_with("primary_workspace_root = "))
        {
            Some(workspace_root_line) => contents.replace(
                workspace_root_line,
                &format!(
                    "primary_workspace_root = \"{}\"",
                    workspace_root.to_str().unwrap()
                ),
            ),
            None => contents,
        };
        let path = root.join("config.toml");
        fs::write(&path, contents).unwrap();
        Self {
            root,
            path,
            authority: authority.to_owned(),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn available_loopback_authority() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap().to_string();
    drop(listener);
    authority
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
