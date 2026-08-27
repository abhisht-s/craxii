use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use craxii_server::test_failpoints::{
    self, BoundaryMetadata, ControlSelection, DUMMY_BYTES, DUMMY_FILE_NAME, DurableClassification,
    FailpointName, FoundationHook, MARKER_FILE_DESCRIPTOR, MARKER_PROTOCOL, MAX_MARKER_BYTES,
    PhysicalHook,
};
use serde::Serialize;
use serde_json::Value;

const MARKER_TIMEOUT: Duration = Duration::from_secs(3);
const TRAILING_MARKER_TIMEOUT: Duration = Duration::from_secs(1);
const LOG_CAPTURE_LIMIT: u64 = 4_096;
const CONFIG_IDENTITY: &str = "test-failpoint-foundation-without-product-config";

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedSignal {
    Sigkill,
    Sigterm,
}

impl RequestedSignal {
    const fn number(self) -> i32 {
        match self {
            Self::Sigkill => 9,
            Self::Sigterm => 15,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitDisposition {
    Signaled,
    Exited,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DummyFileObservation {
    Absent,
    PresentOnceWithExpectedBytes,
    Unexpected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CapturedLog {
    pub observed_bytes: u64,
    pub captured_content: Option<&'static str>,
    pub redacted: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildIdentity {
    pub package_version: &'static str,
    pub git_revision: &'static str,
    pub git_dirty: &'static str,
    pub target_triple: &'static str,
}

const BUILD_IDENTITY: BuildIdentity = BuildIdentity {
    package_version: env!("CARGO_PKG_VERSION"),
    git_revision: env!("CRAXII_GIT_REVISION"),
    git_dirty: env!("CRAXII_GIT_DIRTY"),
    target_triple: env!("CRAXII_BUILD_TARGET"),
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CrashResult {
    pub protocol: &'static str,
    pub run_id: String,
    pub build_identity: BuildIdentity,
    pub config_identity: &'static str,
    pub architecture_alias: Option<FailpointName>,
    pub selected_physical_hook: PhysicalHook,
    pub boundary: BoundaryMetadata,
    pub marker_observed: bool,
    pub marker_observed_before_signal: bool,
    pub requested_signal: RequestedSignal,
    pub observed_disposition: ExitDisposition,
    pub observed_signal: Option<i32>,
    pub observed_exit_code: Option<i32>,
    pub child_reaped: bool,
    pub timeout: bool,
    pub dummy_before_signal: DummyFileObservation,
    pub dummy_after_signal: DummyFileObservation,
    pub stdout: CapturedLog,
    pub stderr: CapturedLog,
    pub pass: bool,
    pub startup_ready: bool,
    pub evidence_role: &'static str,
    pub recovery_truth: bool,
}

pub enum ControllerError {
    Spawn,
    ControlWrite,
    MarkerTimeout(Box<CrashResult>),
    MissingMarker,
    MarkerTooLarge,
    MalformedMarker,
    MismatchedMarker,
    DuplicateMarker,
    Signal,
    Wait,
    LogCapture,
    ProbeObservation,
}

impl ControllerError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Spawn => "child_spawn_failure",
            Self::ControlWrite => "control_write_failure",
            Self::MarkerTimeout(_) => "marker_timeout",
            Self::MissingMarker => "marker_missing",
            Self::MarkerTooLarge => "marker_too_large",
            Self::MalformedMarker => "marker_malformed",
            Self::MismatchedMarker => "marker_mismatch",
            Self::DuplicateMarker => "marker_duplicate",
            Self::Signal => "child_signal_failure",
            Self::Wait => "child_wait_failure",
            Self::LogCapture => "log_capture_failure",
            Self::ProbeObservation => "probe_observation_failure",
        }
    }
}

impl Display for ControllerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Debug for ControllerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ControllerError {}

pub fn run_foundation_probe(
    hook: FoundationHook,
    signal: RequestedSignal,
) -> Result<CrashResult, ControllerError> {
    let run_id = next_run_id();
    let selection = ControlSelection::foundation(hook);
    let directory = test_failpoints::foundation_directory(&run_id)
        .map_err(|_| ControllerError::ProbeObservation)?;
    if directory.exists() {
        return Err(ControllerError::ProbeObservation);
    }

    let mut running = RunningChild::spawn()?;
    let control = selection
        .encode(&run_id)
        .map_err(|_| ControllerError::ControlWrite)?;
    if let Err(error) = running.write_control(&control) {
        running.force_cleanup();
        return Err(error);
    }
    let marker = match running.read_marker_line() {
        Ok(marker) => marker,
        Err(MarkerReadError::Timeout) => {
            let dummy_before_signal =
                observe_dummy_file(&directory).unwrap_or(DummyFileObservation::Unexpected);
            let process = match running.terminate(RequestedSignal::Sigkill) {
                Ok(process) => process,
                Err(error) => {
                    running.force_cleanup();
                    let _ = fs::remove_dir_all(&directory);
                    return Err(error);
                }
            };
            let dummy_after_signal =
                observe_dummy_file(&directory).unwrap_or(DummyFileObservation::Unexpected);
            let result = timeout_result(
                run_id,
                selection,
                process,
                dummy_before_signal,
                dummy_after_signal,
            );
            let _ = fs::remove_dir_all(&directory);
            return Err(ControllerError::MarkerTimeout(Box::new(result)));
        }
        Err(error) => {
            running.force_cleanup();
            let _ = fs::remove_dir_all(&directory);
            return Err(marker_read_controller_error(error));
        }
    };
    if let Err(error) = validate_marker(&marker, &run_id, selection) {
        running.force_cleanup();
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }

    let dummy_before_signal = match observe_dummy_file(&directory) {
        Ok(observation) => observation,
        Err(error) => {
            running.force_cleanup();
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
    };
    let process = match running.terminate(signal) {
        Ok(process) => process,
        Err(error) => {
            running.force_cleanup();
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
    };
    let dummy_after_signal = match observe_dummy_file(&directory) {
        Ok(observation) => observation,
        Err(error) => {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
    };

    let expected_dummy = match hook {
        FoundationHook::BeforeDummyRename => DummyFileObservation::Absent,
        FoundationHook::AfterDummyRename => DummyFileObservation::PresentOnceWithExpectedBytes,
    };
    let observed_signal = process.observed_signal;
    let pass = observed_signal == Some(signal.number())
        && dummy_before_signal == expected_dummy
        && dummy_after_signal == expected_dummy;
    let result = CrashResult {
        protocol: "craxii.crash-result.v1",
        run_id,
        build_identity: BUILD_IDENTITY.clone(),
        config_identity: CONFIG_IDENTITY,
        architecture_alias: selection.architecture_name,
        selected_physical_hook: selection.physical_hook,
        boundary: selection.boundary,
        marker_observed: true,
        marker_observed_before_signal: true,
        requested_signal: signal,
        observed_disposition: process.disposition,
        observed_signal,
        observed_exit_code: process.observed_exit_code,
        child_reaped: true,
        timeout: false,
        dummy_before_signal,
        dummy_after_signal,
        stdout: process.stdout,
        stderr: process.stderr,
        pass,
        startup_ready: false,
        evidence_role: "operational_only",
        recovery_truth: false,
    };
    fs::remove_dir_all(&directory).map_err(|_| ControllerError::ProbeObservation)?;
    Ok(result)
}

pub fn run_missing_marker_timeout() -> Result<CrashResult, ControllerError> {
    let run_id = next_run_id();
    let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
    let mut running = RunningChild::spawn()?;

    match running.read_marker_line() {
        Err(MarkerReadError::Timeout) => {}
        Err(error) => {
            running.force_cleanup();
            return Err(marker_read_controller_error(error));
        }
        Ok(_) => {
            running.force_cleanup();
            return Err(ControllerError::MismatchedMarker);
        }
    }

    let process = running.terminate(RequestedSignal::Sigkill)?;
    let result = timeout_result(
        run_id,
        selection,
        process,
        DummyFileObservation::Absent,
        DummyFileObservation::Absent,
    );
    Err(ControllerError::MarkerTimeout(Box::new(result)))
}

fn next_run_id() -> String {
    let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("run-{}-{sequence}", std::process::id())
}

fn observe_dummy_file(
    directory: &std::path::Path,
) -> Result<DummyFileObservation, ControllerError> {
    let final_path = directory.join(DUMMY_FILE_NAME);
    if !final_path.exists() {
        return Ok(DummyFileObservation::Absent);
    }
    let matching_entries = fs::read_dir(directory)
        .map_err(|_| ControllerError::ProbeObservation)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() == DUMMY_FILE_NAME)
        .count();
    let bytes = fs::read(&final_path).map_err(|_| ControllerError::ProbeObservation)?;
    if matching_entries == 1 && bytes == DUMMY_BYTES {
        Ok(DummyFileObservation::PresentOnceWithExpectedBytes)
    } else {
        Ok(DummyFileObservation::Unexpected)
    }
}

struct RunningChild {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    marker: UnixStream,
    stdout: Option<JoinHandle<CapturedLog>>,
    stderr: Option<JoinHandle<CapturedLog>>,
}

impl RunningChild {
    fn spawn() -> Result<Self, ControllerError> {
        let (controller_marker, child_marker) =
            std::os::unix::net::UnixStream::pair().map_err(|_| ControllerError::Spawn)?;
        let child_marker = if child_marker.as_raw_fd() == MARKER_FILE_DESCRIPTOR {
            let replacement = child_marker
                .try_clone()
                .map_err(|_| ControllerError::Spawn)?;
            drop(child_marker);
            replacement
        } else {
            child_marker
        };
        let source_descriptor = child_marker.as_raw_fd();

        let mut command = Command::new(env!("CARGO_BIN_EXE_craxii-server"));
        command
            .arg(test_failpoints::CONTROL_ARGUMENT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // SAFETY: the pre-exec closure calls only the async-signal-safe `dup2`
        // operation, copying the live controller-owned Unix descriptor to the
        // fixed marker descriptor before `exec`. The parent retains its peer.
        unsafe {
            command.pre_exec(move || {
                unsafe extern "C" {
                    fn dup2(old_descriptor: i32, new_descriptor: i32) -> i32;
                }
                // SAFETY: both integers are validated live descriptors or a
                // fixed nonnegative target; `dup2` reports failure with `-1`.
                if dup2(source_descriptor, MARKER_FILE_DESCRIPTOR) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = command.spawn().map_err(|_| ControllerError::Spawn)?;
        drop(child_marker);
        let stdin = child.stdin.take().ok_or(ControllerError::Spawn)?;
        let stdout = child.stdout.take().ok_or(ControllerError::Spawn)?;
        let stderr = child.stderr.take().ok_or(ControllerError::Spawn)?;
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            marker: controller_marker,
            stdout: Some(capture_log(stdout)),
            stderr: Some(capture_log(stderr)),
        })
    }

    fn write_control(&mut self, control: &str) -> Result<(), ControllerError> {
        let mut stdin = self.stdin.take().ok_or(ControllerError::ControlWrite)?;
        stdin
            .write_all(control.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|_| ControllerError::ControlWrite)?;
        drop(stdin);
        Ok(())
    }

    fn read_marker_line(&mut self) -> Result<Vec<u8>, MarkerReadError> {
        read_marker_line_until(&mut self.marker, Instant::now() + MARKER_TIMEOUT)
    }

    fn terminate(&mut self, signal: RequestedSignal) -> Result<ProcessEvidence, ControllerError> {
        let child = self.child.as_mut().ok_or(ControllerError::Wait)?;
        send_signal(child.id(), signal)?;
        self.stdin.take();
        let status = child.wait().map_err(|_| ControllerError::Wait)?;
        self.child.take();

        let trailing_result =
            reject_trailing_marker(&mut self.marker, Instant::now() + TRAILING_MARKER_TIMEOUT);
        let stdout = join_capture(&mut self.stdout)?;
        let stderr = join_capture(&mut self.stderr)?;
        trailing_result?;
        let observed_signal = status.signal();
        let disposition = if observed_signal.is_some() {
            ExitDisposition::Signaled
        } else if status.code().is_some() {
            ExitDisposition::Exited
        } else {
            ExitDisposition::Unknown
        };
        Ok(ProcessEvidence {
            disposition,
            observed_signal,
            observed_exit_code: status.code(),
            stdout,
            stderr,
        })
    }

    fn force_cleanup(&mut self) -> bool {
        let mut reaped = self.child.is_none();
        if let Some(child) = self.child.as_mut() {
            let _ = send_signal(child.id(), RequestedSignal::Sigkill);
            self.stdin.take();
            reaped = child.wait().is_ok();
        }
        self.child.take();
        let _ = join_capture(&mut self.stdout);
        let _ = join_capture(&mut self.stderr);
        reaped
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.force_cleanup();
    }
}

struct ProcessEvidence {
    disposition: ExitDisposition,
    observed_signal: Option<i32>,
    observed_exit_code: Option<i32>,
    stdout: CapturedLog,
    stderr: CapturedLog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkerReadError {
    Timeout,
    Missing,
    Closed,
    TooLarge,
    Malformed,
}

fn read_with_deadline(
    marker: &mut UnixStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<usize, MarkerReadError> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(MarkerReadError::Timeout)?;
        match marker.read(buffer) {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(remaining.min(Duration::from_millis(1)));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                ) =>
            {
                return Err(MarkerReadError::Closed);
            }
            Err(_) => return Err(MarkerReadError::Malformed),
        }
    }
}

fn read_marker_line_until(
    marker: &mut UnixStream,
    deadline: Instant,
) -> Result<Vec<u8>, MarkerReadError> {
    marker
        .set_nonblocking(true)
        .map_err(|_| MarkerReadError::Malformed)?;
    let mut line = Vec::with_capacity(MAX_MARKER_BYTES);
    let mut byte = [0_u8; 1];
    while line.len() < MAX_MARKER_BYTES {
        match read_with_deadline(marker, &mut byte, deadline) {
            Err(MarkerReadError::Closed) if line.is_empty() => {
                return Err(MarkerReadError::Missing);
            }
            Err(MarkerReadError::Closed) => return Err(MarkerReadError::Malformed),
            Err(error) => return Err(error),
            Ok(0) if line.is_empty() => return Err(MarkerReadError::Missing),
            Ok(0) => return Err(MarkerReadError::Malformed),
            Ok(1) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(line);
                }
            }
            Ok(_) => unreachable!("one-byte marker read returned more than one byte"),
        }
    }
    Err(MarkerReadError::TooLarge)
}

fn marker_read_controller_error(error: MarkerReadError) -> ControllerError {
    match error {
        MarkerReadError::Timeout | MarkerReadError::Missing | MarkerReadError::Closed => {
            ControllerError::MissingMarker
        }
        MarkerReadError::TooLarge => ControllerError::MarkerTooLarge,
        MarkerReadError::Malformed => ControllerError::MalformedMarker,
    }
}

fn trailing_marker_controller_error(error: MarkerReadError) -> ControllerError {
    match error {
        MarkerReadError::TooLarge => ControllerError::MarkerTooLarge,
        MarkerReadError::Timeout
        | MarkerReadError::Missing
        | MarkerReadError::Closed
        | MarkerReadError::Malformed => ControllerError::MalformedMarker,
    }
}

fn timeout_result(
    run_id: String,
    selection: ControlSelection,
    process: ProcessEvidence,
    dummy_before_signal: DummyFileObservation,
    dummy_after_signal: DummyFileObservation,
) -> CrashResult {
    CrashResult {
        protocol: "craxii.crash-result.v1",
        run_id,
        build_identity: BUILD_IDENTITY.clone(),
        config_identity: CONFIG_IDENTITY,
        architecture_alias: selection.architecture_name,
        selected_physical_hook: selection.physical_hook,
        boundary: selection.boundary,
        marker_observed: false,
        marker_observed_before_signal: false,
        requested_signal: RequestedSignal::Sigkill,
        observed_disposition: process.disposition,
        observed_signal: process.observed_signal,
        observed_exit_code: process.observed_exit_code,
        child_reaped: true,
        timeout: true,
        dummy_before_signal,
        dummy_after_signal,
        stdout: process.stdout,
        stderr: process.stderr,
        pass: false,
        startup_ready: false,
        evidence_role: "operational_only",
        recovery_truth: false,
    }
}

fn capture_log(mut reader: impl Read + Send + 'static) -> JoinHandle<CapturedLog> {
    std::thread::spawn(move || {
        let mut observed_bytes = 0_u64;
        let mut buffer = [0_u8; 1_024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => observed_bytes = observed_bytes.saturating_add(read as u64),
            }
        }
        let had_content = observed_bytes > 0;
        CapturedLog {
            observed_bytes: observed_bytes.min(LOG_CAPTURE_LIMIT + 1),
            captured_content: had_content.then_some("[redacted]"),
            redacted: had_content,
            truncated: observed_bytes > LOG_CAPTURE_LIMIT,
        }
    })
}

fn join_capture(
    handle: &mut Option<JoinHandle<CapturedLog>>,
) -> Result<CapturedLog, ControllerError> {
    handle
        .take()
        .ok_or(ControllerError::LogCapture)?
        .join()
        .map_err(|_| ControllerError::LogCapture)
}

fn validate_marker(
    line: &[u8],
    expected_run_id: &str,
    selection: ControlSelection,
) -> Result<(), ControllerError> {
    let marker: Value =
        serde_json::from_slice(line).map_err(|_| ControllerError::MalformedMarker)?;
    let object = marker.as_object().ok_or(ControllerError::MalformedMarker)?;
    let expected_fields = [
        "protocol",
        "run_id",
        "architecture_name",
        "physical_hook",
        "commit_side",
        "io_side",
        "cleanup_phase",
        "expected_durable_classification",
        "sequence",
        "evidence_role",
        "recovery_truth",
        "startup_ready",
    ];
    if object.len() != expected_fields.len()
        || !expected_fields
            .iter()
            .all(|field| object.contains_key(*field))
    {
        return Err(ControllerError::MalformedMarker);
    }
    let expected_architecture = selection
        .architecture_name
        .map_or(Value::Null, |name| Value::String(name.as_str().to_owned()));
    let durable_classification = object
        .get("expected_durable_classification")
        .and_then(Value::as_str)
        .and_then(DurableClassification::parse)
        .ok_or(ControllerError::MalformedMarker)?;
    let matches = marker["protocol"] == MARKER_PROTOCOL
        && marker["run_id"] == expected_run_id
        && marker["architecture_name"] == expected_architecture
        && marker["physical_hook"] == selection.physical_hook.as_str()
        && marker["commit_side"] == selection.boundary.commit_side.as_str()
        && marker["io_side"] == selection.boundary.io_side.as_str()
        && marker["cleanup_phase"] == selection.boundary.cleanup_phase
        && durable_classification == selection.boundary.expected_durable_classification
        && marker["sequence"] == 1
        && marker["evidence_role"] == "operational_only"
        && marker["recovery_truth"] == false
        && marker["startup_ready"] == false;
    if matches {
        Ok(())
    } else {
        Err(ControllerError::MismatchedMarker)
    }
}

fn reject_trailing_marker(
    marker: &mut UnixStream,
    deadline: Instant,
) -> Result<(), ControllerError> {
    marker
        .set_nonblocking(true)
        .map_err(|_| ControllerError::MalformedMarker)?;
    let mut observed = 0_usize;
    let mut buffer = [0_u8; 256];
    loop {
        if observed == MAX_MARKER_BYTES {
            let mut overflow = [0_u8; 1];
            return match read_with_deadline(marker, &mut overflow, deadline) {
                Ok(0) | Err(MarkerReadError::Closed) => Ok(()),
                Ok(1) if !overflow[0].is_ascii_whitespace() => {
                    Err(ControllerError::DuplicateMarker)
                }
                Ok(1) => Err(ControllerError::MarkerTooLarge),
                Ok(_) => unreachable!("one-byte trailing read returned more than one byte"),
                Err(error) => Err(trailing_marker_controller_error(error)),
            };
        }
        let remaining = MAX_MARKER_BYTES - observed;
        let read_limit = remaining.min(buffer.len());
        let read = match read_with_deadline(marker, &mut buffer[..read_limit], deadline) {
            Ok(read) => read,
            Err(MarkerReadError::Closed) => return Ok(()),
            Err(error) => return Err(trailing_marker_controller_error(error)),
        };
        if read == 0 {
            return Ok(());
        }
        observed += read;
        if buffer[..read]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            return Err(ControllerError::DuplicateMarker);
        }
    }
}

fn send_signal(pid: u32, signal: RequestedSignal) -> Result<(), ControllerError> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    let pid = i32::try_from(pid).map_err(|_| ControllerError::Signal)?;
    if pid <= 0 {
        return Err(ControllerError::Signal);
    }
    // SAFETY: `pid` is a validated positive child PID returned by `Child::id`,
    // and `signal` is closed to the fixed SIGKILL/SIGTERM values 9 and 15.
    if unsafe { kill(pid, signal.number()) } == 0 {
        Ok(())
    } else {
        Err(ControllerError::Signal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_value(selection: ControlSelection, run_id: &str) -> Value {
        serde_json::json!({
            "protocol": MARKER_PROTOCOL,
            "run_id": run_id,
            "architecture_name": Value::Null,
            "physical_hook": selection.physical_hook.as_str(),
            "commit_side": selection.boundary.commit_side.as_str(),
            "io_side": selection.boundary.io_side.as_str(),
            "cleanup_phase": selection.boundary.cleanup_phase,
            "expected_durable_classification": selection
                .boundary
                .expected_durable_classification
                .as_str(),
            "sequence": 1,
            "evidence_role": "operational_only",
            "recovery_truth": false,
            "startup_ready": false,
        })
    }

    fn marker(selection: ControlSelection, run_id: &str) -> Vec<u8> {
        serde_json::to_vec(&marker_value(selection, run_id)).unwrap()
    }

    fn stream_containing(bytes: &[u8]) -> UnixStream {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(bytes).unwrap();
        drop(writer);
        reader
    }

    #[test]
    fn exact_durable_classification_is_accepted() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let valid = marker(selection, "run-validation-1");
        assert!(validate_marker(&valid, "run-validation-1", selection).is_ok());
    }

    #[test]
    fn missing_durable_classification_is_rejected() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let mut value = marker_value(selection, "run-validation-1");
        value
            .as_object_mut()
            .unwrap()
            .remove("expected_durable_classification");
        assert!(matches!(
            validate_marker(
                &serde_json::to_vec(&value).unwrap(),
                "run-validation-1",
                selection
            ),
            Err(ControllerError::MalformedMarker)
        ));
    }

    #[test]
    fn malformed_or_unknown_durable_classification_is_rejected() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        for invalid in [serde_json::json!(7), serde_json::json!("unknown_state")] {
            let mut value = marker_value(selection, "run-validation-1");
            value["expected_durable_classification"] = invalid;
            assert!(matches!(
                validate_marker(
                    &serde_json::to_vec(&value).unwrap(),
                    "run-validation-1",
                    selection
                ),
                Err(ControllerError::MalformedMarker)
            ));
        }
    }

    #[test]
    fn mismatched_valid_durable_classification_is_rejected() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let mut value = marker_value(selection, "run-validation-1");
        value["expected_durable_classification"] =
            serde_json::json!(DurableClassification::DummyFinalPresent.as_str());
        assert!(matches!(
            validate_marker(
                &serde_json::to_vec(&value).unwrap(),
                "run-validation-1",
                selection
            ),
            Err(ControllerError::MismatchedMarker)
        ));
    }

    #[test]
    fn other_marker_dimension_mismatch_remains_rejected() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let valid = marker(selection, "run-validation-1");
        assert!(matches!(
            validate_marker(&valid, "run-validation-2", selection),
            Err(ControllerError::MismatchedMarker)
        ));
    }

    #[test]
    fn marker_exactly_at_maximum_size_is_received_and_validated() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let mut exact = marker(selection, "run-boundary-1");
        assert!(exact.len() < MAX_MARKER_BYTES);
        exact.resize(MAX_MARKER_BYTES - 1, b' ');
        exact.push(b'\n');
        let mut reader = stream_containing(&exact);
        let received =
            read_marker_line_until(&mut reader, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(received.len(), MAX_MARKER_BYTES);
        assert!(validate_marker(&received, "run-boundary-1", selection).is_ok());
    }

    #[test]
    fn marker_over_maximum_size_is_boundedly_rejected() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let mut oversized = marker(selection, "run-boundary-2");
        oversized.resize(MAX_MARKER_BYTES, b' ');
        oversized.push(b'\n');
        let mut reader = stream_containing(&oversized);
        assert_eq!(
            read_marker_line_until(&mut reader, Instant::now() + Duration::from_secs(1)),
            Err(MarkerReadError::TooLarge)
        );
    }

    #[test]
    fn oversized_no_newline_stream_is_bounded_and_does_not_escape_sentinel() {
        const SENTINEL: &str = "sentinel-secret /private/sentinel/path raw-user-content";
        let mut oversized = SENTINEL.repeat(MAX_MARKER_BYTES / SENTINEL.len() + 2);
        oversized.truncate(MAX_MARKER_BYTES + 1);
        let mut reader = stream_containing(oversized.as_bytes());
        let error = read_marker_line_until(&mut reader, Instant::now() + Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(error, MarkerReadError::TooLarge);
        let controller_error = marker_read_controller_error(error);
        let evidence = format!("{controller_error:?} {controller_error}");
        assert_eq!(evidence, "marker_too_large marker_too_large");
        assert!(!evidence.contains(SENTINEL));
    }

    #[test]
    fn partial_no_newline_stream_without_eof_respects_one_total_deadline() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            loop {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        let error =
            read_marker_line_until(&mut reader, start + Duration::from_millis(120)).unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(error, MarkerReadError::Timeout);
        assert!(elapsed < Duration::from_millis(500));
        drop(reader);
        writer.join().unwrap();
    }

    #[test]
    fn partial_no_newline_marker_at_eof_is_malformed() {
        let mut reader = stream_containing(b"partial");
        assert_eq!(
            read_marker_line_until(&mut reader, Instant::now() + Duration::from_secs(1)),
            Err(MarkerReadError::Malformed)
        );
    }

    #[test]
    fn second_marker_is_detected_without_waiting_for_eof() {
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let mut first = marker(selection, "run-duplicate-1");
        first.push(b'\n');
        let mut bytes = first.clone();
        bytes.extend_from_slice(&first);
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&bytes).unwrap();
        let received =
            read_marker_line_until(&mut reader, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(received, first);
        let start = Instant::now();
        assert!(matches!(
            reject_trailing_marker(&mut reader, start + Duration::from_secs(1)),
            Err(ControllerError::DuplicateMarker)
        ));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn excessive_trailing_bytes_are_boundedly_rejected() {
        let trailing = vec![b' '; MAX_MARKER_BYTES + 1];
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&trailing).unwrap();
        assert!(matches!(
            reject_trailing_marker(&mut reader, Instant::now() + Duration::from_secs(1)),
            Err(ControllerError::MarkerTooLarge)
        ));
    }

    #[test]
    fn trailing_whitespace_exactly_at_bound_is_accepted_at_eof() {
        let trailing = vec![b' '; MAX_MARKER_BYTES];
        let mut reader = stream_containing(&trailing);
        assert!(
            reject_trailing_marker(&mut reader, Instant::now() + Duration::from_secs(1)).is_ok()
        );
    }

    #[test]
    fn trailing_check_has_one_bounded_deadline_without_waiting_for_eof() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b" \t").unwrap();
        let start = Instant::now();
        assert!(matches!(
            reject_trailing_marker(&mut reader, start + Duration::from_millis(120)),
            Err(ControllerError::MalformedMarker)
        ));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn protocol_error_cleanup_kills_and_reaps_child() {
        let run_id = next_run_id();
        let selection = ControlSelection::foundation(FoundationHook::BeforeDummyRename);
        let directory = test_failpoints::foundation_directory(&run_id).unwrap();
        assert!(!directory.exists());
        let mut running = RunningChild::spawn().unwrap();
        let control = selection.encode(&run_id).unwrap();
        running.write_control(&control).unwrap();
        let observed = running.read_marker_line().unwrap();
        assert!(matches!(
            validate_marker(&observed, "run-deliberate-mismatch", selection),
            Err(ControllerError::MismatchedMarker)
        ));
        assert!(running.force_cleanup());
        assert!(running.child.is_none());
        assert!(running.stdout.is_none());
        assert!(running.stderr.is_none());
        fs::remove_dir_all(directory).unwrap();
    }
}
