#![cfg(all(feature = "test-failpoints", unix))]

mod support;

use std::io::Write as _;
use std::process::{Command, Stdio};

use craxii_server::test_failpoints::{DurableClassification, FoundationHook};
use serde_json::Value;
use support::failpoint_controller::{
    self, ControllerError, DummyFileObservation, ExitDisposition, RequestedSignal,
};

#[test]
fn sigkill_before_dummy_rename_leaves_final_file_absent() {
    let result = failpoint_controller::run_foundation_probe(
        FoundationHook::BeforeDummyRename,
        RequestedSignal::Sigkill,
    )
    .unwrap();
    assert!(result.marker_observed_before_signal);
    assert_eq!(result.observed_disposition, ExitDisposition::Signaled);
    assert_eq!(result.observed_signal, Some(9));
    assert!(result.child_reaped);
    assert_eq!(
        result.boundary.expected_durable_classification,
        DurableClassification::DummyFinalAbsent
    );
    assert_eq!(result.dummy_before_signal, DummyFileObservation::Absent);
    assert_eq!(result.dummy_after_signal, DummyFileObservation::Absent);
    assert!(result.pass);
}

#[test]
fn sigkill_after_synced_dummy_rename_leaves_one_expected_final_file() {
    let result = failpoint_controller::run_foundation_probe(
        FoundationHook::AfterDummyRename,
        RequestedSignal::Sigkill,
    )
    .unwrap();
    assert!(result.marker_observed_before_signal);
    assert_eq!(result.observed_disposition, ExitDisposition::Signaled);
    assert_eq!(result.observed_signal, Some(9));
    assert_eq!(
        result.dummy_before_signal,
        DummyFileObservation::PresentOnceWithExpectedBytes
    );
    assert_eq!(
        result.dummy_after_signal,
        DummyFileObservation::PresentOnceWithExpectedBytes
    );
    assert_eq!(
        result.boundary.expected_durable_classification,
        DurableClassification::DummyFinalPresent
    );
    assert!(result.pass);
}

#[test]
fn sigterm_path_reports_signal_fifteen() {
    let result = failpoint_controller::run_foundation_probe(
        FoundationHook::BeforeDummyRename,
        RequestedSignal::Sigterm,
    )
    .unwrap();
    assert_eq!(result.observed_disposition, ExitDisposition::Signaled);
    assert_eq!(result.observed_signal, Some(15));
    assert!(result.pass);
}

#[test]
fn missing_marker_times_out_then_kills_and_reaps_child() {
    let error = failpoint_controller::run_missing_marker_timeout().unwrap_err();
    let ControllerError::MarkerTimeout(result) = error else {
        panic!("unexpected controller result: {error:?}");
    };
    assert!(result.timeout);
    assert!(!result.marker_observed);
    assert_eq!(result.observed_signal, Some(9));
    assert!(result.child_reaped);
    assert!(!result.pass);
}

#[test]
fn malformed_control_fails_before_probe_without_echoing_input() {
    const SENTINEL: &str = "sentinel-secret /private/sentinel/path Authorization: Bearer token";
    let mut child = Command::new(env!("CARGO_BIN_EXE_craxii-server"))
        .arg(craxii_server::test_failpoints::CONTROL_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(SENTINEL.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(combined, "craxii fatal: invalid_test_control\n");
    assert!(!combined.contains(SENTINEL));
}

#[test]
fn crash_manifest_is_operational_redacted_and_startup_stays_unready() {
    let result = failpoint_controller::run_foundation_probe(
        FoundationHook::AfterDummyRename,
        RequestedSignal::Sigkill,
    )
    .unwrap();
    let serialized = serde_json::to_string(&result).unwrap();
    let value: Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(value["protocol"], "craxii.crash-result.v1");
    assert_eq!(value["architecture_alias"], Value::Null);
    assert_eq!(value["evidence_role"], "operational_only");
    assert_eq!(value["recovery_truth"], false);
    assert_eq!(value["startup_ready"], false);
    assert_eq!(
        value["boundary"]["expected_durable_classification"],
        "dummy_final_present"
    );
    for forbidden in [
        "sentinel-secret",
        "/private/sentinel/path",
        "Authorization: Bearer",
        "raw-user-content",
        "raw-command-content",
    ] {
        assert!(!serialized.contains(forbidden));
    }
}

#[test]
fn repeated_disposable_runs_have_equivalent_normalized_outcomes() {
    let first = failpoint_controller::run_foundation_probe(
        FoundationHook::AfterDummyRename,
        RequestedSignal::Sigkill,
    )
    .unwrap();
    let second = failpoint_controller::run_foundation_probe(
        FoundationHook::AfterDummyRename,
        RequestedSignal::Sigkill,
    )
    .unwrap();
    assert_ne!(first.run_id, second.run_id);
    assert_eq!(first.selected_physical_hook, second.selected_physical_hook);
    assert_eq!(first.boundary, second.boundary);
    assert_eq!(first.marker_observed, second.marker_observed);
    assert_eq!(first.requested_signal, second.requested_signal);
    assert_eq!(first.observed_disposition, second.observed_disposition);
    assert_eq!(first.observed_signal, second.observed_signal);
    assert_eq!(first.dummy_before_signal, second.dummy_before_signal);
    assert_eq!(first.dummy_after_signal, second.dummy_after_signal);
    assert_eq!(first.pass, second.pass);
}
