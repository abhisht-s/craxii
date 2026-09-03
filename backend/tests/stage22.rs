#![cfg(target_os = "macos")]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use craxii_server::adapters::scripted_provider::{ScriptGate, ScriptedStep};
use stage18_harness::{EstimatorMode, ProgramPlan, Stage18Harness, Stage18Root, programs};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run by scripts/verify-stage22-macos-client with the native probe"]
async fn native_conversation_queues_reconnects_and_cancels_against_localhost_backend() {
    let probe = std::env::var_os("CRAXII_STAGE22_PROBE").map(PathBuf::from);
    let Some(probe) = probe.filter(|path| path.is_file()) else {
        panic!("CRAXII_STAGE22_PROBE must identify the prebuilt native integration executable");
    };

    let first_gate = ScriptGate::new();
    let second_gate = ScriptGate::new();
    let cancellation_gate = ScriptGate::new();
    let mut provider_programs = programs(&[
        ProgramPlan::Answer {
            text: "stage22 first authoritative answer".to_owned(),
            require_tool_result: None,
        },
        ProgramPlan::Answer {
            text: "stage22 second authoritative answer".to_owned(),
            require_tool_result: None,
        },
        ProgramPlan::Answer {
            text: "must be cancelled before completion".to_owned(),
            require_tool_result: None,
        },
    ]);
    provider_programs[0]
        .steps
        .insert(2, ScriptedStep::AwaitRelease(first_gate.clone()));
    provider_programs[1]
        .steps
        .insert(2, ScriptedStep::AwaitRelease(second_gate.clone()));
    provider_programs[2]
        .steps
        .insert(2, ScriptedStep::AwaitRelease(cancellation_gate.clone()));

    let harness = Stage18Harness::start(
        Stage18Root::new("stage22-native"),
        provider_programs,
        EstimatorMode::Normal,
    )
    .await
    .expect("start deterministic backend harness");
    let profile_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let state_directory = harness.root.path().join("native-client-state");
    let mut child = tokio::process::Command::new(probe)
        .env("CRAXII_STAGE22_INTEGRATION", "1")
        .env(
            "CRAXII_STAGE22_ENDPOINT",
            format!("http://{}/", harness.authority),
        )
        .env("CRAXII_STAGE22_TOKEN", &harness.bearer)
        .env("CRAXII_STAGE22_PROFILE_ID", &profile_id)
        .env("CRAXII_STAGE22_STATE_DIR", state_directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn native Stage 22 integration client");
    let stdout = child.stdout.take().expect("capture native probe stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut observations = Vec::new();
    let read_result = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(line) = lines.next_line().await.expect("read native probe output") {
            match line.as_str() {
                "FIRST_DRAFT_SECOND_QUEUED" => first_gate.release(),
                "DISCONNECT_CLEARED_DRAFT" => second_gate.release(),
                _ => {}
            }
            let complete = line == "STAGE22_NATIVE_INTEGRATION_PASSED";
            observations.push(line);
            if complete {
                break;
            }
        }
    })
    .await;
    if read_result.is_err() {
        first_gate.release();
        second_gate.release();
        cancellation_gate.release();
        let _ = child.kill().await;
    }
    let status = child.wait().await.expect("join native probe");
    let mut stderr = Vec::new();
    if let Some(mut stream) = child.stderr.take() {
        stream
            .read_to_end(&mut stderr)
            .await
            .expect("read native probe diagnostic stderr");
    }
    let captures = harness.provider.captures();
    let capture_summary = captures
        .iter()
        .map(|capture| {
            format!(
                "{:?}/{}",
                capture.terminal(),
                capture.cancellation_observed()
            )
        })
        .collect::<Vec<_>>();
    let keychain_cleanup = tokio::process::Command::new("/usr/bin/security")
        .args([
            "delete-generic-password",
            "-s",
            "com.craxii.device-token.v1",
            "-a",
            &profile_id,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .expect("run scoped integration Keychain cleanup");
    harness.shutdown().await;

    assert!(read_result.is_ok(), "native integration probe timed out");
    assert!(
        keychain_cleanup.success() || keychain_cleanup.code() == Some(44),
        "scoped integration Keychain cleanup failed",
    );
    assert!(
        status.success(),
        "native integration probe failed: {}; observations={observations:?}; captures={capture_summary:?}",
        String::from_utf8_lossy(&stderr),
    );
    for expected in [
        "BOOTSTRAP_LIVE",
        "FIRST_PREPARED",
        "FIRST_DRAFT_SECOND_QUEUED",
        "FIFO_SECOND_ACTIVE",
        "DISCONNECT_CLEARED_DRAFT",
        "RECONNECT_REPLAY_NO_DUPLICATES",
        "CANCELLATION_PREPARED",
        "CANCELLATION_DURABLE_TRUTH",
        "FINAL_BOOTSTRAP_CONVERGED",
        "STAGE22_NATIVE_INTEGRATION_PASSED",
    ] {
        assert!(
            observations.iter().any(|line| line == expected),
            "missing {expected}"
        );
    }
    assert_eq!(
        captures.len(),
        3,
        "all three FIFO work items must reach the provider"
    );
}
