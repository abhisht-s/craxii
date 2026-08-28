use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

const LOCAL: &str = include_str!("fixtures/config/valid/local.toml");
const EC2: &str = include_str!("fixtures/config/valid/ec2-shape.toml");
const SECRET_SENTINEL: &str = "startup-secret-sentinel-XYZ";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn valid_local_config_emits_pretty_startup_evidence_and_remains_unready() {
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
        "max_supported_schema_version=3",
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
fn startup_does_not_read_declared_credential_files() {
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

    let input = LOCAL
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
        );
    let config_path = root.join("config.toml");
    fs::write(&config_path, input).unwrap();

    let output = run(&["--config", config_path.to_str().unwrap()]);
    fs::set_permissions(&credential_file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert!(!text(&output.stdout).contains(SECRET_SENTINEL));
    assert!(output.stderr.is_empty());
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

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_craxii-server"))
        .args(arguments)
        .output()
        .expect("craxii-server binary should execute")
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
}

impl TempConfig {
    fn new(contents: &str) -> Self {
        let root = temporary_root();
        fs::create_dir(&root).unwrap();
        let state_root = root.join("state");
        fs::create_dir(&state_root).unwrap();
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o700)).unwrap();
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
        let path = root.join("config.toml");
        fs::write(&path, contents).unwrap();
        Self { root, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
