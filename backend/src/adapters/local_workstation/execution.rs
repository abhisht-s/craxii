//! Owned Stage 13 foreground-process lifecycle for [`super::LocalWorkstation`].

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::fs::File;
use std::os::fd::AsRawFd as _;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::{Id as TaskId, JoinHandle, JoinSet};
use tracing::Instrument;

use crate::domain::{
    ArtifactId, Certainty, ExecutionId, MonotonicDuration, PrivilegeMode, Sha256Digest,
};
use crate::ports::artifact_store::{ArtifactCapture, ArtifactStore, BeginArtifactCapture};
use crate::ports::clock::Clock;
use crate::ports::workstation::{
    CancellationResult, EXECUTION_STREAM_PROJECTION_BYTES, EXECUTION_STREAM_PROJECTION_HEAD_BYTES,
    EXECUTION_STREAM_PROJECTION_TAIL_BYTES, EXECUTION_TERM_GRACE_MS, ExecutionCancellationState,
    ExecutionCleanupEvidence, ExecutionInspection, ExecutionInspectionState, ExecutionRequest,
    ExecutionResult, ExecutionResultKind, ExecutionStreamResult, HARD_EXECUTION_COMMAND_MAX_BYTES,
    HARD_EXECUTION_STREAM_CAPTURE_BYTES, HARD_EXECUTION_TIMEOUT_MS, WorkstationError,
    WorkstationErrorKind,
};

const BASH_PATH: &str = "/bin/bash";
const SUDO_PATH: &str = "/usr/bin/sudo";
const ENV_PATH: &str = "/usr/bin/env";
const READ_BUFFER_BYTES: usize = 16_384;
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(10);

const USER_HOME: &str = "/home/craxii";
const USER_PATH: &str = "/home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const ADMIN_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub(super) struct ExecutionCwd {
    pub(super) directory: File,
    pub(super) evidence: crate::domain::ResolvedPathEvidence,
}

#[derive(Clone)]
pub(super) struct ExecutionRuntimeConfig {
    pub(super) shell: PathBuf,
    pub(super) administrative_capable: bool,
    pub(super) cgroup_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LeaderObservationStatus {
    Pending,
    Interrupted,
    Terminal,
}

pub(super) trait LeaderObserver: Send + Sync {
    fn observe(&self, pid: i32) -> std::io::Result<LeaderObservationStatus>;
}

struct WaitIdLeaderObserver;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExecutionTestPoint {
    BeforeReservation,
    AfterReservation,
}

#[cfg(test)]
#[derive(Clone)]
struct ExecutionTestGate {
    point: ExecutionTestPoint,
    arrived: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

pub(super) struct ExecutionRuntime {
    registry: Mutex<ExecutionRegistry>,
    shutdown_deadline: Mutex<Option<tokio::time::Instant>>,
    shutdown_deadline_changed: Notify,
    shutdown_cleanup_failed: AtomicBool,
    manager: Mutex<Option<ManagerOwnership>>,
    empty: Notify,
    artifact_store: Arc<dyn ArtifactStore>,
    clock: Arc<dyn Clock>,
    config: ExecutionRuntimeConfig,
    leader_observer: Arc<dyn LeaderObserver>,
    #[cfg(test)]
    lifecycle_events: Mutex<Vec<&'static str>>,
    #[cfg(test)]
    execution_gate: Mutex<Option<ExecutionTestGate>>,
}

struct ExecutionRegistry {
    admission_open: bool,
    entries: HashMap<ExecutionId, Arc<ExecutionEntry>>,
}

struct ManagerOwnership {
    sender: mpsc::UnboundedSender<ManagerCommand>,
    join: Option<JoinHandle<()>>,
}

enum ManagerCommand {
    Launch(Box<Launch>),
    Stop(oneshot::Sender<()>),
}

struct Launch {
    entry: Arc<ExecutionEntry>,
    request: ExecutionRequest,
    cwd: ExecutionCwd,
}

struct ExecutionEntry {
    request: ExecutionRequest,
    lifecycle: Mutex<ExecutionLifecycle>,
    cause_changed: Notify,
    terminal: Mutex<Option<ExecutionResult>>,
    terminal_changed: Notify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutionPhase {
    Reserved,
    Spawning,
    Running,
    Terminating,
    Terminal,
}

struct ExecutionLifecycle {
    phase: ExecutionPhase,
    cause: TerminalCause,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalCause {
    None,
    Natural,
    Cancellation,
    Timeout,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LatchResult {
    Won,
    Same,
    Lost,
}

impl ExecutionEntry {
    fn new(request: ExecutionRequest) -> Self {
        Self {
            request,
            lifecycle: Mutex::new(ExecutionLifecycle {
                phase: ExecutionPhase::Reserved,
                cause: TerminalCause::None,
            }),
            cause_changed: Notify::new(),
            terminal: Mutex::new(None),
            terminal_changed: Notify::new(),
        }
    }

    fn latch(&self, cause: TerminalCause) -> LatchResult {
        let result = {
            let mut lifecycle = lock(&self.lifecycle);
            if lifecycle.cause == TerminalCause::None {
                lifecycle.cause = cause;
                LatchResult::Won
            } else if lifecycle.cause == cause {
                LatchResult::Same
            } else {
                LatchResult::Lost
            }
        };
        if result == LatchResult::Won {
            // Exactly one owned supervisor consumes this edge. `notify_one` retains a permit
            // when cancellation wins before the supervisor begins waiting.
            self.cause_changed.notify_one();
        }
        result
    }

    fn cause(&self) -> TerminalCause {
        lock(&self.lifecycle).cause
    }

    fn is_terminal(&self) -> bool {
        lock(&self.lifecycle).phase == ExecutionPhase::Terminal
    }

    fn claim_spawn(&self) -> Result<(), TerminalCause> {
        let mut lifecycle = lock(&self.lifecycle);
        debug_assert_eq!(lifecycle.phase, ExecutionPhase::Reserved);
        if lifecycle.cause != TerminalCause::None {
            return Err(lifecycle.cause);
        }
        lifecycle.phase = ExecutionPhase::Spawning;
        Ok(())
    }

    fn set_running(&self) {
        let mut lifecycle = lock(&self.lifecycle);
        debug_assert_eq!(lifecycle.phase, ExecutionPhase::Spawning);
        lifecycle.phase = ExecutionPhase::Running;
    }

    fn set_terminating(&self) {
        let mut lifecycle = lock(&self.lifecycle);
        lifecycle.phase = ExecutionPhase::Terminating;
    }

    fn complete(&self, result: ExecutionResult) {
        let mut terminal = lock(&self.terminal);
        if terminal.is_none() {
            *terminal = Some(result);
            lock(&self.lifecycle).phase = ExecutionPhase::Terminal;
            drop(terminal);
            self.terminal_changed.notify_waiters();
        }
    }

    async fn result(&self) -> ExecutionResult {
        loop {
            let notified = self.terminal_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = lock(&self.terminal).clone() {
                return result;
            }
            notified.await;
        }
    }
}

impl ExecutionRuntime {
    pub(super) fn new(
        artifact_store: Arc<dyn ArtifactStore>,
        clock: Arc<dyn Clock>,
        config: ExecutionRuntimeConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry: Mutex::new(ExecutionRegistry {
                admission_open: true,
                entries: HashMap::new(),
            }),
            shutdown_deadline: Mutex::new(None),
            shutdown_deadline_changed: Notify::new(),
            shutdown_cleanup_failed: AtomicBool::new(false),
            manager: Mutex::new(None),
            empty: Notify::new(),
            artifact_store,
            clock,
            config,
            leader_observer: Arc::new(WaitIdLeaderObserver),
            #[cfg(test)]
            lifecycle_events: Mutex::new(Vec::new()),
            #[cfg(test)]
            execution_gate: Mutex::new(None),
        })
    }

    pub(super) fn foreground_capable(&self) -> bool {
        cfg!(target_os = "macos") || self.config.cgroup_root.is_some()
    }

    pub(super) fn administrative_capable(&self) -> bool {
        self.config.administrative_capable
    }

    #[cfg(test)]
    pub(super) fn lifecycle_events(&self) -> Vec<&'static str> {
        lock(&self.lifecycle_events).clone()
    }

    #[cfg(test)]
    pub(super) fn set_shell_for_test(&mut self, shell: PathBuf) {
        self.config.shell = shell;
    }

    #[cfg(test)]
    pub(super) fn set_leader_observer_for_test(&mut self, observer: Arc<dyn LeaderObserver>) {
        self.leader_observer = observer;
    }

    #[cfg(test)]
    pub(super) fn set_execution_gate_for_test(
        &self,
        point: ExecutionTestPoint,
        arrived: Arc<tokio::sync::Barrier>,
        release: Arc<tokio::sync::Barrier>,
    ) {
        *lock(&self.execution_gate) = Some(ExecutionTestGate {
            point,
            arrived,
            release,
        });
    }

    #[cfg(test)]
    async fn reach_execution_gate(&self, point: ExecutionTestPoint) {
        let gate = lock(&self.execution_gate).clone();
        if let Some(gate) = gate.filter(|gate| gate.point == point) {
            gate.arrived.wait().await;
            gate.release.wait().await;
        }
    }

    pub(super) async fn execute(
        self: &Arc<Self>,
        request: ExecutionRequest,
        cwd: ExecutionCwd,
    ) -> Result<ExecutionResult, WorkstationError> {
        self.validate_request(&request)?;
        #[cfg(test)]
        self.reach_execution_gate(ExecutionTestPoint::BeforeReservation)
            .await;

        let entry = Arc::new(ExecutionEntry::new(request.clone()));
        {
            let mut registry = lock(&self.registry);
            if !registry.admission_open {
                return Err(WorkstationError::new(
                    WorkstationErrorKind::WorkstationUnavailable,
                ));
            }
            if registry.entries.contains_key(&request.execution_id) {
                return Err(WorkstationError::new(WorkstationErrorKind::SpawnFailed));
            }
            registry
                .entries
                .insert(request.execution_id, Arc::clone(&entry));
        }
        #[cfg(test)]
        self.reach_execution_gate(ExecutionTestPoint::AfterReservation)
            .await;

        let sender = self.ensure_manager();
        if sender
            .send(ManagerCommand::Launch(Box::new(Launch {
                entry: Arc::clone(&entry),
                request,
                cwd,
            })))
            .is_err()
        {
            self.remove(entry.request.execution_id);
            return Err(WorkstationError::new(
                WorkstationErrorKind::InternalWorkstationError,
            ));
        }

        Ok(entry.result().await)
    }

    pub(super) fn inspect(
        &self,
        operation_id: crate::domain::OperationId,
        execution_id: ExecutionId,
    ) -> Result<ExecutionInspection, WorkstationError> {
        let entry = lock(&self.registry)
            .entries
            .get(&execution_id)
            .cloned()
            .ok_or_else(|| WorkstationError::new(WorkstationErrorKind::InspectionNotFound))?;
        Ok(ExecutionInspection {
            operation_id,
            execution_id,
            state: if entry.is_terminal() {
                ExecutionInspectionState::Terminal
            } else {
                ExecutionInspectionState::Running
            },
        })
    }

    pub(super) async fn cancel(
        &self,
        operation_id: crate::domain::OperationId,
        execution_id: ExecutionId,
    ) -> CancellationResult {
        let entry = lock(&self.registry).entries.get(&execution_id).cloned();
        let Some(entry) = entry else {
            return CancellationResult {
                operation_id,
                execution_id,
                state: ExecutionCancellationState::NotFound,
            };
        };
        if entry.is_terminal() {
            return CancellationResult {
                operation_id,
                execution_id,
                state: ExecutionCancellationState::AlreadyTerminal,
            };
        }
        let latch = entry.latch(TerminalCause::Cancellation);
        let result = entry.result().await;
        CancellationResult {
            operation_id,
            execution_id,
            state: if !result.cleanup.confirmed() {
                ExecutionCancellationState::CleanupUnconfirmed
            } else if latch == LatchResult::Lost {
                ExecutionCancellationState::AlreadyTerminal
            } else {
                ExecutionCancellationState::Confirmed
            },
        }
    }

    pub(super) fn begin_shutdown(&self, deadline: tokio::time::Instant) {
        {
            let mut installed = lock(&self.shutdown_deadline);
            match *installed {
                Some(existing) => debug_assert_eq!(existing, deadline),
                None => *installed = Some(deadline),
            }
        }
        self.shutdown_deadline_changed.notify_waiters();
        {
            let mut registry = lock(&self.registry);
            registry.admission_open = false;
            for entry in registry.entries.values() {
                let _ = entry.latch(TerminalCause::Shutdown);
            }
        }
    }

    pub(super) async fn shutdown_before(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), WorkstationError> {
        self.begin_shutdown(deadline);
        loop {
            let notified = self.empty.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let empty = lock(&self.registry).entries.is_empty();
            if empty {
                break;
            }
            notified.await;
        }

        let ownership = lock(&self.manager).take();
        if let Some(mut ownership) = ownership {
            let (sent, received) = oneshot::channel();
            let _ = ownership.sender.send(ManagerCommand::Stop(sent));
            received
                .await
                .map_err(|_| WorkstationError::uncertain(WorkstationErrorKind::CleanupFailed))?;
            if let Some(join) = ownership.join.take() {
                join.await.map_err(|_| {
                    WorkstationError::uncertain(WorkstationErrorKind::CleanupFailed)
                })?;
            }
        }
        if self.shutdown_cleanup_failed.load(Ordering::Acquire) {
            Err(WorkstationError::uncertain(
                WorkstationErrorKind::CleanupFailed,
            ))
        } else {
            Ok(())
        }
    }

    fn effective_cleanup_remaining(
        &self,
        request_deadline: crate::ports::clock::MonotonicInstant,
    ) -> Option<Duration> {
        let request_remaining = remaining(self.clock.as_ref(), request_deadline)?;
        let shutdown_remaining = lock(&self.shutdown_deadline)
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()));
        Some(
            shutdown_remaining.map_or(request_remaining, |shutdown_remaining| {
                request_remaining.min(shutdown_remaining)
            }),
        )
        .filter(|remaining| !remaining.is_zero())
    }

    fn stage10_deadline_expired(&self) -> bool {
        lock(&self.shutdown_deadline)
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
    }

    fn validate_request(&self, request: &ExecutionRequest) -> Result<(), WorkstationError> {
        let timeout_ms = request.timeout.as_duration().as_millis();
        if request.command.is_empty()
            || request.command.len() > HARD_EXECUTION_COMMAND_MAX_BYTES
            || request.command.as_bytes().contains(&0)
            || timeout_ms == 0
            || timeout_ms > u128::from(HARD_EXECUTION_TIMEOUT_MS)
            || request.capture.stdout_max_bytes == 0
            || request.capture.stderr_max_bytes == 0
            || request.capture.stdout_max_bytes > HARD_EXECUTION_STREAM_CAPTURE_BYTES
            || request.capture.stderr_max_bytes > HARD_EXECUTION_STREAM_CAPTURE_BYTES
        {
            return Err(WorkstationError::new(WorkstationErrorKind::SpawnFailed));
        }
        if remaining(self.clock.as_ref(), request.deadline).is_none() {
            return Err(WorkstationError::new(WorkstationErrorKind::Timeout));
        }
        if !self.foreground_capable()
            || (request.effective_privilege == PrivilegeMode::Administrative
                && !self.administrative_capable())
        {
            return Err(WorkstationError::new(
                WorkstationErrorKind::UnsupportedCapability,
            ));
        }
        Ok(())
    }

    fn ensure_manager(self: &Arc<Self>) -> mpsc::UnboundedSender<ManagerCommand> {
        let mut manager = lock(&self.manager);
        if let Some(manager) = manager.as_ref() {
            return manager.sender.clone();
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let join = tokio::spawn(manager_loop(Arc::downgrade(self), receiver));
        *manager = Some(ManagerOwnership {
            sender: sender.clone(),
            join: Some(join),
        });
        sender
    }

    fn remove(&self, execution_id: ExecutionId) {
        let (removed, empty) = {
            let mut registry = lock(&self.registry);
            let removed = registry.entries.remove(&execution_id).is_some();
            (removed, registry.entries.is_empty())
        };
        if removed && empty {
            self.empty.notify_waiters();
        }
    }
}

async fn manager_loop(
    runtime: Weak<ExecutionRuntime>,
    mut receiver: mpsc::UnboundedReceiver<ManagerCommand>,
) {
    let mut supervisors = JoinSet::new();
    let mut task_execution_ids = HashMap::<TaskId, ExecutionId>::new();
    let mut stop_ack: Option<oneshot::Sender<()>> = None;
    loop {
        if stop_ack.is_some() && supervisors.is_empty() {
            if let Some(ack) = stop_ack.take() {
                let _ = ack.send(());
            }
            return;
        }
        tokio::select! {
            command = receiver.recv(), if stop_ack.is_none() => match command {
                Some(ManagerCommand::Launch(launch)) => {
                    let Some(runtime) = runtime.upgrade() else { return; };
                    let execution_id = launch.request.execution_id;
                    let handle = supervisors.spawn(supervise(Arc::clone(&runtime), *launch));
                    task_execution_ids.insert(handle.id(), execution_id);
                }
                Some(ManagerCommand::Stop(ack)) => stop_ack = Some(ack),
                None => return,
            },
            joined = supervisors.join_next_with_id(), if !supervisors.is_empty() => {
                let Some(joined) = joined else { continue; };
                match joined {
                    Ok((task_id, (execution_id, entry, result))) => {
                        task_execution_ids.remove(&task_id);
                        if let Some(runtime) = runtime.upgrade()
                            && lock(&runtime.shutdown_deadline).is_some()
                            && !result.cleanup.confirmed()
                        {
                            runtime
                                .shutdown_cleanup_failed
                                .store(true, Ordering::Release);
                        }
                        entry.complete(result);
                        tokio::task::yield_now().await;
                        if let Some(runtime) = runtime.upgrade() {
                            runtime.remove(execution_id);
                        }
                    }
                    Err(error) => {
                        let task_id = error.id();
                        if let Some(execution_id) = task_execution_ids.remove(&task_id)
                            && let Some(runtime) = runtime.upgrade()
                        {
                            if lock(&runtime.shutdown_deadline).is_some() {
                                runtime
                                    .shutdown_cleanup_failed
                                    .store(true, Ordering::Release);
                            }
                            if let Some(entry) = lock(&runtime.registry)
                                .entries
                                .get(&execution_id)
                                .cloned()
                            {
                                entry.complete(panic_failure_result(&entry.request));
                            }
                            runtime.remove(execution_id);
                            tracing::error!(%execution_id, lifecycle_phase = "supervisor_join", "workstation execution supervisor failed");
                        }
                    }
                }
            }
        }
    }
}

async fn supervise(
    runtime: Arc<ExecutionRuntime>,
    launch: Launch,
) -> (ExecutionId, Arc<ExecutionEntry>, ExecutionResult) {
    let execution_id = launch.request.execution_id;
    let entry = Arc::clone(&launch.entry);
    let result = supervise_inner(&runtime, launch).await;
    (execution_id, entry, result)
}

async fn supervise_inner(runtime: &ExecutionRuntime, launch: Launch) -> ExecutionResult {
    let started = Instant::now();
    let request = launch.request;
    let execution_id = request.execution_id;
    let resolved_cwd = launch.cwd.evidence.clone();
    let command_sha256 = Sha256Digest::hash_bytes(request.command.as_bytes());
    if let Err(cause) = launch.entry.claim_spawn() {
        record_lifecycle(runtime, "owned_pre_spawn_cancelled");
        return pre_spawn_terminal_result(&request, resolved_cwd, cause, started.elapsed());
    }
    record_lifecycle(runtime, "spawn_claimed");
    let stdout_capture = begin_capture(runtime, request.capture.stdout_max_bytes);
    let stderr_capture = begin_capture(runtime, request.capture.stderr_max_bytes);
    let (stdout_capture, stderr_capture) = match (stdout_capture, stderr_capture) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        _ => {
            return spawn_failure_result(
                &request,
                resolved_cwd,
                WorkstationErrorKind::InternalWorkstationError,
            );
        }
    };

    let cgroup = match ExecutionCgroup::create(runtime.config.cgroup_root.as_deref(), execution_id)
    {
        Ok(cgroup) => cgroup,
        Err(kind) => return spawn_failure_result(&request, resolved_cwd, kind),
    };

    let mut command = build_command(&runtime.config.shell, &request);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let cwd_fd = launch.cwd.directory.as_raw_fd();
    let cgroup_procs = cgroup.as_ref().map(|cgroup| cgroup.procs_cstring.clone());
    let inherited_fds = open_file_descriptors();
    // SAFETY: the closure performs only fchdir/setsid/open/write/close using already prepared
    // descriptors and bytes. It takes no locks, allocates nothing, and touches no Rust runtime
    // state after fork.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if nix::libc::fchdir(cwd_fd) != 0 || nix::libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(path) = cgroup_procs.as_ref() {
                let fd = nix::libc::open(path.as_ptr(), nix::libc::O_WRONLY | nix::libc::O_CLOEXEC);
                if fd == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let attached = nix::libc::write(fd, b"0\n".as_ptr().cast(), 2) == 2;
                let close_result = nix::libc::close(fd);
                if !attached || close_result != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            for fd in &inherited_fds {
                let flags = nix::libc::fcntl(*fd, nix::libc::F_GETFD);
                if flags != -1 {
                    let _ =
                        nix::libc::fcntl(*fd, nix::libc::F_SETFD, flags | nix::libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            let cleanup = cgroup.map_or_else(
                CgroupCleanup::not_applicable,
                ExecutionCgroup::cleanup_after_spawn_failure,
            );
            return ExecutionResult {
                operation_id: request.operation_id,
                execution_id,
                start_observed: false,
                requested_cwd: request.requested_cwd,
                resolved_cwd,
                effective_privilege: request.effective_privilege,
                command_sha256,
                result_kind: if cleanup.confirmed() {
                    ExecutionResultKind::SpawnFailed
                } else {
                    ExecutionResultKind::CleanupFailed
                },
                exit_code: None,
                terminating_signal: None,
                timed_out: false,
                cancelled: false,
                duration: MonotonicDuration::from_duration(started.elapsed()),
                stdout: finalize_empty(stdout_capture).ok(),
                stderr: finalize_empty(stderr_capture).ok(),
                cleanup: cleanup.evidence(true, true, true, true),
                error: Some(if cleanup.confirmed() {
                    WorkstationError::new(WorkstationErrorKind::SpawnFailed)
                } else {
                    WorkstationError::uncertain(WorkstationErrorKind::CleanupFailed)
                }),
                certainty: if cleanup.confirmed() {
                    Certainty::Definite
                } else {
                    Certainty::OutcomeUnknown
                },
            };
        }
    };
    let Some(pid_u32) = child.id() else {
        return spawn_failure_result(&request, resolved_cwd, WorkstationErrorKind::SpawnFailed);
    };
    let Ok(pgid) = i32::try_from(pid_u32) else {
        return spawn_failure_result(&request, resolved_cwd, WorkstationErrorKind::SpawnFailed);
    };
    let process_group = StableProcessGroup::new(pgid);
    launch.entry.set_running();
    tracing::info!(
        %execution_id,
        privilege = ?request.effective_privilege,
        lifecycle_phase = "spawned",
        diagnostic_pid = pgid,
        "workstation execution lifecycle advanced"
    );
    #[cfg(feature = "test-failpoints")]
    crate::test_failpoints::reach(crate::test_failpoints::PhysicalHook::AfterToolProcessSpawn);

    let stdout = child.stdout.take().expect("piped stdout is configured");
    let stderr = child.stderr.take().expect("piped stderr is configured");
    let mut drains = JoinSet::new();
    let stdout_abort = drains.spawn(drain_stream(stdout, stdout_capture));
    let stderr_abort = drains.spawn(drain_stream(stderr, stderr_capture));
    let stdout_task = stdout_abort.id();
    let stderr_task = stderr_abort.id();

    let runtime_budget = tokio::time::sleep(request.timeout.as_duration());
    tokio::pin!(runtime_budget);
    let absolute_budget = tokio::time::sleep(
        remaining(runtime.clock.as_ref(), request.deadline).unwrap_or(Duration::ZERO),
    );
    tokio::pin!(absolute_budget);

    let mut leader_terminal_observed = false;
    let mut leader_identity_stable = true;
    loop {
        match runtime.leader_observer.observe(process_group.leader_pid()) {
            Ok(LeaderObservationStatus::Terminal) => {
                leader_terminal_observed = true;
                record_lifecycle(runtime, "leader_terminal_observed");
                let _ = launch.entry.latch(TerminalCause::Natural);
                break;
            }
            Ok(LeaderObservationStatus::Pending | LeaderObservationStatus::Interrupted) => {}
            Err(_) => {
                leader_identity_stable = false;
                break;
            }
        }
        tokio::select! {
            () = tokio::time::sleep(GROUP_POLL_INTERVAL) => {}
            () = &mut runtime_budget => {
                let _ = launch.entry.latch(TerminalCause::Timeout);
                break;
            }
            () = &mut absolute_budget => {
                let _ = launch.entry.latch(TerminalCause::Timeout);
                break;
            }
            () = launch.entry.cause_changed.notified() => break,
        }
    }
    launch.entry.set_terminating();
    let cleanup_span = tracing::info_span!(
        "process_cleanup",
        execution_id = %execution_id,
        diagnostic_pid = process_group.leader_pid(),
        direct_child_reaped = tracing::field::Empty,
        stdout_drain_joined = tracing::field::Empty,
        stderr_drain_joined = tracing::field::Empty,
        process_group_empty = tracing::field::Empty,
        cgroup_empty = tracing::field::Empty,
        cgroup_removed = tracing::field::Empty,
        result_class = tracing::field::Empty,
    );
    let owned_cleanup = finish_owned_process_tree(
        runtime,
        &launch.entry,
        &mut child,
        process_group,
        cgroup,
        request.deadline,
        LeaderObservation {
            terminal: leader_terminal_observed,
            identity_stable: leader_identity_stable,
        },
    )
    .instrument(cleanup_span.clone())
    .await;
    let status = owned_cleanup.status;
    let process_group_empty = owned_cleanup.process_group_empty;
    let cgroup_cleanup = owned_cleanup.cgroup_cleanup;

    let mut stdout = None;
    let mut stderr = None;
    let mut stdout_joined = false;
    let mut stderr_joined = false;
    while !drains.is_empty() {
        let Some(budget) = runtime.effective_cleanup_remaining(request.deadline) else {
            drains.abort_all();
            while drains.join_next().await.is_some() {}
            break;
        };
        match tokio::time::timeout(budget, drains.join_next_with_id()).await {
            Ok(Some(Ok((task_id, Ok(stream))))) => {
                if task_id == stdout_task {
                    stdout = Some(stream);
                    stdout_joined = true;
                } else if task_id == stderr_task {
                    stderr = Some(stream);
                    stderr_joined = true;
                }
            }
            Ok(Some(Ok((_, Err(_))))) | Ok(Some(Err(_))) => {}
            Ok(None) => break,
            Err(_) => {
                drains.abort_all();
                while drains.join_next().await.is_some() {}
                break;
            }
        }
    }

    let cleanup = cgroup_cleanup.evidence(
        owned_cleanup.direct_child_reaped,
        stdout_joined,
        stderr_joined,
        process_group_empty,
    );
    let cleanup_confirmed = cleanup.confirmed();
    cleanup_span.record("direct_child_reaped", cleanup.direct_child_reaped);
    cleanup_span.record("stdout_drain_joined", cleanup.stdout_drain_joined);
    cleanup_span.record("stderr_drain_joined", cleanup.stderr_drain_joined);
    cleanup_span.record("process_group_empty", cleanup.process_group_empty);
    if let Some(value) = cleanup.cgroup_empty {
        cleanup_span.record("cgroup_empty", value);
    }
    if let Some(value) = cleanup.cgroup_removed {
        cleanup_span.record("cgroup_removed", value);
    }
    cleanup_span.record(
        "result_class",
        if cleanup_confirmed {
            "confirmed"
        } else {
            "unconfirmed"
        },
    );
    let cause = launch.entry.cause();
    let (mut result_kind, timed_out, cancelled) = match cause {
        TerminalCause::Timeout => (ExecutionResultKind::TimedOut, true, false),
        TerminalCause::Cancellation | TerminalCause::Shutdown => {
            (ExecutionResultKind::Cancelled, false, true)
        }
        _ => match status.as_ref().and_then(std::process::ExitStatus::code) {
            Some(_) => (ExecutionResultKind::Exited, false, false),
            None => (ExecutionResultKind::Signaled, false, false),
        },
    };
    if !cleanup_confirmed || stdout.is_none() || stderr.is_none() {
        result_kind = ExecutionResultKind::CleanupFailed;
    }
    let exit_code = status
        .as_ref()
        .and_then(std::process::ExitStatus::code)
        .map(i64::from);
    #[cfg(unix)]
    let terminating_signal = {
        use std::os::unix::process::ExitStatusExt as _;
        status
            .as_ref()
            .and_then(std::process::ExitStatus::signal)
            .map(i64::from)
    };
    let certainty = if cleanup_confirmed && stdout.is_some() && stderr.is_some() {
        Certainty::Definite
    } else {
        Certainty::OutcomeUnknown
    };
    let error = (result_kind == ExecutionResultKind::CleanupFailed)
        .then(|| WorkstationError::uncertain(WorkstationErrorKind::CleanupFailed));
    tracing::info!(
        %execution_id,
        privilege = ?request.effective_privilege,
        lifecycle_phase = "terminal",
        result_kind = ?result_kind,
        duration_ms = started.elapsed().as_millis(),
        stdout_observed = ?stdout.as_ref().map(|stream| stream.observed_bytes),
        stderr_observed = ?stderr.as_ref().map(|stream| stream.observed_bytes),
        cleanup_confirmed,
        "workstation execution lifecycle completed"
    );

    ExecutionResult {
        operation_id: request.operation_id,
        execution_id,
        start_observed: true,
        requested_cwd: request.requested_cwd,
        resolved_cwd,
        effective_privilege: request.effective_privilege,
        command_sha256,
        result_kind,
        exit_code,
        terminating_signal,
        timed_out,
        cancelled,
        duration: MonotonicDuration::from_duration(started.elapsed()),
        stdout,
        stderr,
        cleanup,
        error,
        certainty,
    }
}

fn begin_capture(
    runtime: &ExecutionRuntime,
    limit: u64,
) -> Result<Box<dyn ArtifactCapture>, WorkstationError> {
    runtime
        .artifact_store
        .begin_capture(BeginArtifactCapture {
            artifact_id: ArtifactId::generate(),
            hard_capture_limit: crate::domain::CanonicalByteCount::try_new(limit).map_err(
                |_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError),
            )?,
        })
        .map_err(|_| WorkstationError::new(WorkstationErrorKind::InternalWorkstationError))
}

async fn drain_stream<R: AsyncRead + Unpin>(
    mut reader: R,
    mut capture: Box<dyn ArtifactCapture>,
) -> Result<ExecutionStreamResult, WorkstationError> {
    let mut observed = 0_u64;
    let mut saturated = false;
    let mut first = Vec::with_capacity(EXECUTION_STREAM_PROJECTION_BYTES);
    let mut tail = VecDeque::with_capacity(EXECUTION_STREAM_PROJECTION_TAIL_BYTES);
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|_| WorkstationError::new(WorkstationErrorKind::IoError))?;
        if count == 0 {
            break;
        }
        let chunk = &buffer[..count];
        let (next, overflow) = observed.overflowing_add(count as u64);
        if overflow {
            observed = u64::MAX;
            saturated = true;
        } else if !saturated {
            observed = next;
        }
        capture
            .write_chunk(chunk)
            .map_err(|_| WorkstationError::new(WorkstationErrorKind::IoError))?;
        let first_remaining = EXECUTION_STREAM_PROJECTION_BYTES.saturating_sub(first.len());
        first.extend_from_slice(&chunk[..first_remaining.min(chunk.len())]);
        retain_tail(&mut tail, chunk);
    }
    let artifact = capture
        .finalize()
        .map_err(|_| WorkstationError::new(WorkstationErrorKind::IoError))?;
    let captured = artifact.captured_byte_count().get();
    let projection_bytes = if observed <= EXECUTION_STREAM_PROJECTION_BYTES as u64 && !saturated {
        first
    } else {
        let mut projection =
            first[..EXECUTION_STREAM_PROJECTION_HEAD_BYTES.min(first.len())].to_vec();
        projection.extend(tail);
        projection
    };
    let projection_had_utf8_replacement = std::str::from_utf8(&projection_bytes).is_err();
    let projection = String::from_utf8_lossy(&projection_bytes).into_owned();
    Ok(ExecutionStreamResult {
        artifact,
        projection,
        projection_had_utf8_replacement,
        observed_bytes: observed,
        captured_bytes: captured,
        omitted_bytes: observed.saturating_sub(captured),
        projection_omitted_bytes: observed.saturating_sub(projection_bytes.len() as u64),
        observed_count_saturated: saturated,
        truncated: observed > captured || saturated,
    })
}

fn retain_tail(tail: &mut VecDeque<u8>, chunk: &[u8]) {
    if chunk.len() >= EXECUTION_STREAM_PROJECTION_TAIL_BYTES {
        tail.clear();
        tail.extend(&chunk[chunk.len() - EXECUTION_STREAM_PROJECTION_TAIL_BYTES..]);
        return;
    }
    let excess = tail
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(EXECUTION_STREAM_PROJECTION_TAIL_BYTES);
    tail.drain(..excess);
    tail.extend(chunk);
}

fn finalize_empty(
    capture: Box<dyn ArtifactCapture>,
) -> Result<ExecutionStreamResult, WorkstationError> {
    let artifact = capture
        .finalize()
        .map_err(|_| WorkstationError::new(WorkstationErrorKind::IoError))?;
    Ok(ExecutionStreamResult {
        artifact,
        projection: String::new(),
        projection_had_utf8_replacement: false,
        observed_bytes: 0,
        captured_bytes: 0,
        omitted_bytes: 0,
        projection_omitted_bytes: 0,
        observed_count_saturated: false,
        truncated: false,
    })
}

fn build_command(shell: &Path, request: &ExecutionRequest) -> Command {
    let mut command = if request.effective_privilege == PrivilegeMode::Administrative {
        let mut command = Command::new(SUDO_PATH);
        command.env_clear();
        command.arg("-n").arg(ENV_PATH).arg("-i");
        for (name, value) in child_environment(request, PrivilegeMode::Administrative) {
            command.arg(format!("{name}={value}"));
        }
        command.arg(shell);
        command
    } else {
        let mut command = Command::new(shell);
        command.env_clear();
        command.envs(child_environment(request, PrivilegeMode::User));
        command
    };
    command
        .arg("--noprofile")
        .arg("--norc")
        .arg("-o")
        .arg("pipefail")
        .arg("-c")
        .arg(&request.command);
    command
}

fn child_environment(
    request: &ExecutionRequest,
    privilege: PrivilegeMode,
) -> Vec<(&'static str, String)> {
    let (home, user, path) = if privilege == PrivilegeMode::Administrative {
        ("/root", "root", ADMIN_PATH)
    } else {
        (USER_HOME, "craxii", USER_PATH)
    };
    vec![
        ("HOME", home.to_owned()),
        ("USER", user.to_owned()),
        ("LOGNAME", user.to_owned()),
        ("SHELL", BASH_PATH.to_owned()),
        ("LANG", "C.UTF-8".to_owned()),
        ("PATH", path.to_owned()),
        ("CRAXII_WORK_ID", request.work_id.to_string()),
        ("CRAXII_WORKSPACE_ID", request.workspace_id.to_string()),
    ]
}

fn open_file_descriptors() -> Vec<i32> {
    #[cfg(target_os = "linux")]
    const DIRECTORY: &str = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    const DIRECTORY: &str = "/dev/fd";
    let mut descriptors: Vec<_> = std::fs::read_dir(DIRECTORY)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|descriptor| *descriptor > 2)
        .collect();
    descriptors.sort_unstable();
    descriptors.dedup();
    descriptors
}

struct StableProcessGroup {
    pgid: i32,
}

impl StableProcessGroup {
    const fn new(pgid: i32) -> Self {
        Self { pgid }
    }

    const fn leader_pid(&self) -> i32 {
        self.pgid
    }

    fn signal(&self, signal: Signal) {
        let _ = killpg(Pid::from_raw(self.pgid), signal);
    }

    fn release_for_reap(self) -> ReleasedProcessGroup {
        ReleasedProcessGroup
    }
}

struct ReleasedProcessGroup;

struct OwnedProcessCleanup {
    status: Option<std::process::ExitStatus>,
    direct_child_reaped: bool,
    process_group_empty: bool,
    cgroup_cleanup: CgroupCleanup,
}

struct LeaderObservation {
    terminal: bool,
    identity_stable: bool,
}

async fn finish_owned_process_tree(
    runtime: &ExecutionRuntime,
    entry: &ExecutionEntry,
    child: &mut Child,
    process_group: StableProcessGroup,
    cgroup: Option<ExecutionCgroup>,
    request_deadline: crate::ports::clock::MonotonicInstant,
    observation: LeaderObservation,
) -> OwnedProcessCleanup {
    let LeaderObservation {
        terminal: mut leader_terminal_observed,
        identity_stable: mut leader_identity_stable,
    } = observation;
    let cause = entry.cause();
    let mut descendants_quiescent = leader_identity_stable
        && owned_descendants_quiescent(&process_group, cgroup.as_ref(), leader_terminal_observed);
    let termination_required = cause != TerminalCause::Natural || !descendants_quiescent;
    if termination_required && leader_identity_stable {
        signal_owned_tree(runtime, &process_group, cgroup.as_ref(), Signal::SIGTERM);
        #[cfg(feature = "test-failpoints")]
        stage13_marker("during_term_kill");
    }

    let grace = runtime
        .effective_cleanup_remaining(request_deadline)
        .unwrap_or(Duration::ZERO)
        .min(Duration::from_millis(EXECUTION_TERM_GRACE_MS));
    let grace_deadline = tokio::time::Instant::now() + grace;
    while leader_identity_stable
        && (!leader_terminal_observed || !descendants_quiescent)
        && wait_for_cleanup_tick(runtime, request_deadline, Some(grace_deadline)).await
    {
        refresh_leader_observation(
            runtime,
            &process_group,
            &mut leader_terminal_observed,
            &mut leader_identity_stable,
        );
        descendants_quiescent = leader_identity_stable
            && owned_descendants_quiescent(
                &process_group,
                cgroup.as_ref(),
                leader_terminal_observed,
            );
    }

    if leader_identity_stable && (!leader_terminal_observed || !descendants_quiescent) {
        signal_owned_tree(runtime, &process_group, cgroup.as_ref(), Signal::SIGKILL);
        if let Some(cgroup) = cgroup.as_ref() {
            cgroup.kill_all();
        }
        while leader_identity_stable
            && (!leader_terminal_observed || !descendants_quiescent)
            && wait_for_cleanup_tick(runtime, request_deadline, None).await
        {
            refresh_leader_observation(
                runtime,
                &process_group,
                &mut leader_terminal_observed,
                &mut leader_identity_stable,
            );
            descendants_quiescent = leader_identity_stable
                && owned_descendants_quiescent(
                    &process_group,
                    cgroup.as_ref(),
                    leader_terminal_observed,
                );
        }
    }

    if runtime.stage10_deadline_expired() && (!leader_terminal_observed || !descendants_quiescent) {
        runtime
            .shutdown_cleanup_failed
            .store(true, Ordering::Release);
        if leader_identity_stable {
            signal_owned_tree(runtime, &process_group, cgroup.as_ref(), Signal::SIGKILL);
        }
        if let Some(cgroup) = cgroup.as_ref() {
            cgroup.kill_all();
        }
        let _ = child.start_kill();
    }

    let cgroup_cleanup = match cgroup {
        Some(cgroup) => cgroup.finish_cleanup(runtime, request_deadline).await,
        None => CgroupCleanup::not_applicable(),
    };
    if leader_identity_stable {
        descendants_quiescent =
            owned_descendants_quiescent(&process_group, None, leader_terminal_observed)
                || cgroup_cleanup.empty == Some(true);
    }
    if descendants_quiescent {
        record_lifecycle(runtime, "descendant_cleanup_finished");
    }

    record_lifecycle(runtime, "leader_identity_released_for_reap");
    let _released_process_group = process_group.release_for_reap();
    let status = match child.try_wait() {
        Ok(Some(status)) => Some(status),
        Ok(None) if leader_terminal_observed => {
            let budget = runtime.effective_cleanup_remaining(request_deadline);
            match budget {
                Some(budget) => tokio::time::timeout(budget, child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok),
                None => None,
            }
        }
        Ok(None) | Err(_) => None,
    };
    if status.is_some() {
        record_lifecycle(runtime, "leader_reaped");
    }

    OwnedProcessCleanup {
        status,
        direct_child_reaped: status.is_some(),
        process_group_empty: leader_identity_stable && descendants_quiescent,
        cgroup_cleanup,
    }
}

fn refresh_leader_observation(
    runtime: &ExecutionRuntime,
    process_group: &StableProcessGroup,
    observed: &mut bool,
    stable: &mut bool,
) {
    if *observed || !*stable {
        return;
    }
    match runtime.leader_observer.observe(process_group.leader_pid()) {
        Ok(LeaderObservationStatus::Terminal) => {
            *observed = true;
            record_lifecycle(runtime, "leader_terminal_observed");
        }
        Ok(LeaderObservationStatus::Pending | LeaderObservationStatus::Interrupted) => {}
        Err(_) => *stable = false,
    }
}

impl LeaderObserver for WaitIdLeaderObserver {
    fn observe(&self, pid: i32) -> std::io::Result<LeaderObservationStatus> {
        // SAFETY: waitid initializes `siginfo` for the selected direct child. WNOWAIT preserves
        // the waitable leader and therefore pins its PID/process-group identity until final cleanup.
        let (result, siginfo) = unsafe {
            let mut siginfo: nix::libc::siginfo_t = std::mem::zeroed();
            let result = nix::libc::waitid(
                nix::libc::P_PID,
                pid as nix::libc::id_t,
                &raw mut siginfo,
                nix::libc::WEXITED | nix::libc::WNOHANG | nix::libc::WNOWAIT,
            );
            (result, siginfo)
        };
        let observed_pid = if result == 0 {
            // SAFETY: a successful waitid with WEXITED makes the SIGCHLD sender field readable.
            unsafe { siginfo.si_pid() }
        } else {
            0
        };
        let error = (result != 0)
            .then(std::io::Error::last_os_error)
            .and_then(|error| error.raw_os_error());
        normalize_waitid_attempt(result, observed_pid, error)
    }
}

fn normalize_waitid_attempt(
    result: i32,
    observed_pid: i32,
    error: Option<i32>,
) -> std::io::Result<LeaderObservationStatus> {
    if result == 0 {
        return Ok(if observed_pid == 0 {
            LeaderObservationStatus::Pending
        } else {
            LeaderObservationStatus::Terminal
        });
    }
    let error = error.unwrap_or(nix::libc::EIO);
    if error == nix::libc::EINTR {
        Ok(LeaderObservationStatus::Interrupted)
    } else {
        Err(std::io::Error::from_raw_os_error(error))
    }
}

async fn wait_for_cleanup_tick(
    runtime: &ExecutionRuntime,
    request_deadline: crate::ports::clock::MonotonicInstant,
    phase_deadline: Option<tokio::time::Instant>,
) -> bool {
    let changed = runtime.shutdown_deadline_changed.notified();
    tokio::pin!(changed);
    changed.as_mut().enable();
    let Some(effective_remaining) = runtime.effective_cleanup_remaining(request_deadline) else {
        return false;
    };
    let phase_remaining = phase_deadline.map_or(effective_remaining, |deadline| {
        deadline.saturating_duration_since(tokio::time::Instant::now())
    });
    let budget = effective_remaining
        .min(phase_remaining)
        .min(GROUP_POLL_INTERVAL);
    if budget.is_zero() {
        return false;
    }
    tokio::select! {
        () = tokio::time::sleep(budget) => true,
        () = &mut changed => true,
    }
}

fn signal_owned_tree(
    runtime: &ExecutionRuntime,
    process_group: &StableProcessGroup,
    cgroup: Option<&ExecutionCgroup>,
    signal: Signal,
) {
    record_lifecycle(runtime, "process_group_signalled");
    process_group.signal(signal);
    if let Some(cgroup) = cgroup {
        cgroup.signal_members(signal);
    }
}

fn owned_descendants_quiescent(
    process_group: &StableProcessGroup,
    cgroup: Option<&ExecutionCgroup>,
    leader_terminal_observed: bool,
) -> bool {
    if !leader_terminal_observed {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        cgroup.is_some_and(ExecutionCgroup::is_empty)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = cgroup;
        macos_process_group_contains_only_owned_leader(process_group.pgid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (process_group, cgroup);
        false
    }
}

#[cfg(target_os = "macos")]
fn macos_process_group_contains_only_owned_leader(pgid: i32) -> bool {
    const PROC_PGRP_ONLY: u32 = 2;
    let pid_size = std::mem::size_of::<nix::libc::pid_t>();
    // SAFETY: a null buffer asks libproc for the current byte requirement for this one PGID.
    let required =
        unsafe { nix::libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, std::ptr::null_mut(), 0) };
    if required < 0 {
        return false;
    }
    let mut slots = usize::try_from(required)
        .unwrap_or(0)
        .div_ceil(pid_size)
        .saturating_add(16)
        .max(16);
    loop {
        let mut pids = vec![0 as nix::libc::pid_t; slots];
        let byte_capacity = pids.len().saturating_mul(pid_size);
        let Ok(byte_capacity_i32) = i32::try_from(byte_capacity) else {
            return false;
        };
        // SAFETY: the buffer is writable for exactly `byte_capacity_i32` bytes.
        let written = unsafe {
            nix::libc::proc_listpids(
                PROC_PGRP_ONLY,
                pgid as u32,
                pids.as_mut_ptr().cast(),
                byte_capacity_i32,
            )
        };
        if written < 0 {
            return false;
        }
        let Ok(written) = usize::try_from(written) else {
            return false;
        };
        if written >= byte_capacity {
            slots = slots.saturating_mul(2);
            continue;
        }
        pids.truncate(written / pid_size);
        return pids.into_iter().all(|pid| pid == 0 || pid == pgid);
    }
}

#[cfg(test)]
fn record_lifecycle(runtime: &ExecutionRuntime, event: &'static str) {
    lock(&runtime.lifecycle_events).push(event);
}

#[cfg(not(test))]
const fn record_lifecycle(_runtime: &ExecutionRuntime, _event: &'static str) {}

fn remaining(
    clock: &dyn Clock,
    deadline: crate::ports::clock::MonotonicInstant,
) -> Option<Duration> {
    deadline
        .checked_duration_since(clock.monotonic_now())
        .filter(|remaining| !remaining.is_zero())
}

fn spawn_failure_result(
    request: &ExecutionRequest,
    resolved_cwd: crate::domain::ResolvedPathEvidence,
    kind: WorkstationErrorKind,
) -> ExecutionResult {
    ExecutionResult {
        operation_id: request.operation_id,
        execution_id: request.execution_id,
        start_observed: false,
        requested_cwd: request.requested_cwd.clone(),
        resolved_cwd,
        effective_privilege: request.effective_privilege,
        command_sha256: Sha256Digest::hash_bytes(request.command.as_bytes()),
        result_kind: ExecutionResultKind::SpawnFailed,
        exit_code: None,
        terminating_signal: None,
        timed_out: false,
        cancelled: false,
        duration: MonotonicDuration::from_millis(0),
        stdout: None,
        stderr: None,
        cleanup: ExecutionCleanupEvidence {
            direct_child_reaped: true,
            stdout_drain_joined: true,
            stderr_drain_joined: true,
            process_group_empty: true,
            cgroup_empty: None,
            cgroup_removed: None,
        },
        error: Some(WorkstationError::new(kind)),
        certainty: Certainty::Definite,
    }
}

fn pre_spawn_terminal_result(
    request: &ExecutionRequest,
    resolved_cwd: crate::domain::ResolvedPathEvidence,
    cause: TerminalCause,
    duration: Duration,
) -> ExecutionResult {
    debug_assert!(matches!(
        cause,
        TerminalCause::Cancellation | TerminalCause::Shutdown | TerminalCause::Timeout
    ));
    let timed_out = cause == TerminalCause::Timeout;
    ExecutionResult {
        operation_id: request.operation_id,
        execution_id: request.execution_id,
        start_observed: false,
        requested_cwd: request.requested_cwd.clone(),
        resolved_cwd,
        effective_privilege: request.effective_privilege,
        command_sha256: Sha256Digest::hash_bytes(request.command.as_bytes()),
        result_kind: if timed_out {
            ExecutionResultKind::TimedOut
        } else {
            ExecutionResultKind::Cancelled
        },
        exit_code: None,
        terminating_signal: None,
        timed_out,
        cancelled: !timed_out,
        duration: MonotonicDuration::from_duration(duration),
        stdout: None,
        stderr: None,
        cleanup: ExecutionCleanupEvidence {
            direct_child_reaped: true,
            stdout_drain_joined: true,
            stderr_drain_joined: true,
            process_group_empty: true,
            cgroup_empty: None,
            cgroup_removed: None,
        },
        error: None,
        certainty: Certainty::Definite,
    }
}

fn panic_failure_result(request: &ExecutionRequest) -> ExecutionResult {
    let resolved = crate::domain::ResolvedPathEvidence::try_new(
        request.workstation_id,
        request.expected_generation,
        request.workspace_id,
        request.requested_cwd.clone(),
        "/",
    )
    .expect("root is valid redacted fallback evidence");
    let mut result = spawn_failure_result(request, resolved, WorkstationErrorKind::CleanupFailed);
    result.result_kind = ExecutionResultKind::CleanupFailed;
    result.error = Some(WorkstationError::uncertain(
        WorkstationErrorKind::CleanupFailed,
    ));
    result.certainty = Certainty::OutcomeUnknown;
    result.cleanup.process_group_empty = false;
    result
}

#[derive(Clone, Copy)]
struct CgroupCleanup {
    empty: Option<bool>,
    removed: Option<bool>,
}

impl CgroupCleanup {
    const fn not_applicable() -> Self {
        Self {
            empty: None,
            removed: None,
        }
    }

    const fn confirmed(self) -> bool {
        !matches!(self.empty, Some(false)) && !matches!(self.removed, Some(false))
    }

    const fn evidence(
        self,
        direct_child_reaped: bool,
        stdout_drain_joined: bool,
        stderr_drain_joined: bool,
        process_group_empty: bool,
    ) -> ExecutionCleanupEvidence {
        ExecutionCleanupEvidence {
            direct_child_reaped,
            stdout_drain_joined,
            stderr_drain_joined,
            process_group_empty,
            cgroup_empty: self.empty,
            cgroup_removed: self.removed,
        }
    }
}

struct ExecutionCgroup {
    path: PathBuf,
    procs_cstring: CString,
}

impl ExecutionCgroup {
    fn create(
        configured_root: Option<&Path>,
        execution_id: ExecutionId,
    ) -> Result<Option<Self>, WorkstationErrorKind> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (configured_root, execution_id);
            Ok(None)
        }
        #[cfg(target_os = "linux")]
        {
            let root = configured_root.ok_or(WorkstationErrorKind::UnsupportedCapability)?;
            let path = root.join(execution_id.to_string());
            std::fs::create_dir(&path).map_err(|_| WorkstationErrorKind::SpawnFailed)?;
            let procs = path.join("cgroup.procs");
            let procs_cstring = CString::new(procs.as_os_str().as_bytes())
                .map_err(|_| WorkstationErrorKind::SpawnFailed)?;
            Ok(Some(Self {
                path,
                procs_cstring,
            }))
        }
    }

    fn signal_members(&self, signal: Signal) {
        let Ok(procs) = std::fs::read_to_string(self.path.join("cgroup.procs")) else {
            return;
        };
        for pid in procs.lines().filter_map(|line| line.parse::<i32>().ok()) {
            let _ = kill(Pid::from_raw(pid), signal);
        }
    }

    fn kill_all(&self) {
        let _ = std::fs::write(self.path.join("cgroup.kill"), b"1\n");
    }

    fn is_empty(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            cgroup_populated(&self.path) == Some(false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }

    async fn finish_cleanup(
        self,
        runtime: &ExecutionRuntime,
        request_deadline: crate::ports::clock::MonotonicInstant,
    ) -> CgroupCleanup {
        if !self.is_empty() {
            self.kill_all();
        }
        while !self.is_empty() {
            let Some(budget) = runtime.effective_cleanup_remaining(request_deadline) else {
                break;
            };
            tokio::time::sleep(budget.min(GROUP_POLL_INTERVAL)).await;
        }
        let empty = self.is_empty();
        let removed = empty && std::fs::remove_dir(&self.path).is_ok();
        #[cfg(feature = "test-failpoints")]
        stage13_marker("after_cleanup");
        CgroupCleanup {
            empty: Some(empty),
            removed: Some(removed),
        }
    }

    fn cleanup_after_spawn_failure(self) -> CgroupCleanup {
        let empty = self.is_empty();
        let removed = empty && std::fs::remove_dir(&self.path).is_ok();
        CgroupCleanup {
            empty: Some(empty),
            removed: Some(removed),
        }
    }
}

#[cfg(target_os = "linux")]
fn cgroup_populated(path: &Path) -> Option<bool> {
    let events = std::fs::read_to_string(path.join("cgroup.events")).ok()?;
    events.lines().find_map(|line| {
        line.strip_prefix("populated ")
            .and_then(|value| match value {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            })
    })
}

pub(super) fn probe_cgroup_root(configured_root: Option<&Path>) -> Option<PathBuf> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = configured_root;
        None
    }
    #[cfg(target_os = "linux")]
    {
        let root = configured_root?;
        if !root.is_absolute()
            || root == Path::new("/")
            || root == Path::new("/sys/fs/cgroup")
            || !root.join("cgroup.controllers").is_file()
        {
            return None;
        }
        let canonical = std::fs::canonicalize(root).ok()?;
        if canonical == Path::new("/sys/fs/cgroup") || !canonical.starts_with("/sys/fs/cgroup/") {
            return None;
        }
        let probe = canonical.join(format!(".craxii-stage13-probe-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&probe).ok()?;
        let valid = probe.join("cgroup.procs").is_file()
            && probe.join("cgroup.events").is_file()
            && probe.join("cgroup.kill").is_file()
            && cgroup_populated(&probe) == Some(false)
            && std::fs::remove_dir(&probe).is_ok();
        valid.then_some(canonical)
    }
}

pub(super) fn probe_admin(administrative_enabled: bool, cgroup_root: Option<&Path>) -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (administrative_enabled, cgroup_root);
        false
    }
    #[cfg(target_os = "linux")]
    {
        if !administrative_enabled || cgroup_root.is_none() || !Path::new(SUDO_PATH).is_file() {
            return false;
        }
        let execution_id = ExecutionId::generate();
        let Ok(Some(cgroup)) = ExecutionCgroup::create(cgroup_root, execution_id) else {
            return false;
        };
        let procs = cgroup.procs_cstring.clone();
        let mut command = std::process::Command::new(SUDO_PATH);
        command.env_clear().args([
                "-n",
                ENV_PATH,
                "-i",
                "HOME=/root",
                "USER=root",
                "LOGNAME=root",
                "SHELL=/bin/bash",
                "LANG=C.UTF-8",
                concat!(
                    "PATH=",
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                ),
                BASH_PATH,
                "--noprofile",
                "--norc",
                "-o",
                "pipefail",
                "-c",
            ])
            .arg(format!(
                concat!(
                    "test \"$(id -u)\" = 0 && ",
                    "test \"$HOME\" = /root && test \"$USER\" = root && ",
                    "test \"$LOGNAME\" = root && test \"$SHELL\" = /bin/bash && ",
                    "test \"$LANG\" = C.UTF-8 && ",
                    "test \"$PATH\" = /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin && ",
                    "test -z \"${{OPENAI_API_KEY-}}\" && test -z \"${{AWS_SECRET_ACCESS_KEY-}}\" && ",
                    "grep -Fq '/{}' /proc/self/cgroup"
                ),
                execution_id
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: the already prepared cgroup.procs pathname is opened and written using only
        // async-signal-safe libc calls before the reviewed sudo executable starts.
        unsafe {
            command.pre_exec(move || {
                let fd =
                    nix::libc::open(procs.as_ptr(), nix::libc::O_WRONLY | nix::libc::O_CLOEXEC);
                if fd == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                let attached = nix::libc::write(fd, b"0\n".as_ptr().cast(), 2) == 2;
                let closed = nix::libc::close(fd) == 0;
                if !attached || !closed {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let status = command.status();
        let empty = cgroup.is_empty();
        let removed = empty && std::fs::remove_dir(&cgroup.path).is_ok();
        status.is_ok_and(|status| status.success()) && empty && removed
    }
}

#[cfg(feature = "test-failpoints")]
fn stage13_marker(name: &str) {
    if std::env::var("CRAXII_TEST_ABORT_AT_STAGE13_MARKER")
        .ok()
        .as_deref()
        == Some(name)
    {
        std::process::abort();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        LogicalPathReference, OperationId, WorkId, WorkspaceId, WorkstationGeneration,
        WorkstationId,
    };
    use crate::ports::clock::MonotonicInstant;
    use crate::ports::workstation::{
        ExecutionCapturePolicy, ExecutionCleanupPolicy, ExecutionStdinPolicy,
    };
    use crate::ports::workstation_preparation::{
        PreparedCwdEvidence, PreparedCwdObjectIdentity, PreparedCwdObjectType,
    };

    fn request() -> ExecutionRequest {
        let workstation_id = WorkstationId::generate();
        let generation = WorkstationGeneration::try_new(1).unwrap();
        let workspace_id = WorkspaceId::generate();
        let requested_cwd = LogicalPathReference::absolute("/").unwrap();
        ExecutionRequest {
            operation_id: OperationId::generate(),
            execution_id: ExecutionId::generate(),
            work_id: WorkId::generate(),
            workstation_id,
            expected_generation: generation,
            workspace_id,
            command: "true".to_owned(),
            requested_cwd: requested_cwd.clone(),
            prepared_cwd: PreparedCwdEvidence::new(
                crate::domain::ResolvedPathEvidence::try_new(
                    workstation_id,
                    generation,
                    workspace_id,
                    requested_cwd,
                    "/",
                )
                .unwrap(),
                PreparedCwdObjectIdentity::try_new(1, 1, PreparedCwdObjectType::Directory).unwrap(),
            ),
            effective_privilege: PrivilegeMode::User,
            stdin: ExecutionStdinPolicy::Closed,
            timeout: MonotonicDuration::from_millis(1_000),
            deadline: MonotonicInstant::from_elapsed(Duration::from_secs(2)),
            capture: ExecutionCapturePolicy {
                stdout_max_bytes: 1,
                stderr_max_bytes: 1,
            },
            cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
        }
    }

    #[test]
    fn first_terminal_cause_wins_cancel_timeout_and_natural_exit_races() {
        for (winner, loser) in [
            (TerminalCause::Cancellation, TerminalCause::Natural),
            (TerminalCause::Natural, TerminalCause::Cancellation),
            (TerminalCause::Timeout, TerminalCause::Natural),
            (TerminalCause::Natural, TerminalCause::Timeout),
            (TerminalCause::Shutdown, TerminalCause::Natural),
        ] {
            let entry = ExecutionEntry::new(request());
            assert_eq!(entry.latch(winner), LatchResult::Won);
            assert_eq!(entry.latch(winner), LatchResult::Same);
            assert_eq!(entry.latch(loser), LatchResult::Lost);
            assert_eq!(entry.cause(), winner);
        }
    }

    #[test]
    fn waitid_eintr_is_one_cooperative_nonterminal_observation() {
        assert_eq!(
            normalize_waitid_attempt(-1, 0, Some(nix::libc::EINTR)).unwrap(),
            LeaderObservationStatus::Interrupted
        );
        assert_eq!(
            normalize_waitid_attempt(0, 0, None).unwrap(),
            LeaderObservationStatus::Pending
        );
        assert_eq!(
            normalize_waitid_attempt(0, 42, None).unwrap(),
            LeaderObservationStatus::Terminal
        );
        assert_eq!(
            normalize_waitid_attempt(-1, 0, Some(nix::libc::ECHILD))
                .unwrap_err()
                .raw_os_error(),
            Some(nix::libc::ECHILD)
        );
    }

    #[test]
    fn launcher_argv_and_user_admin_environment_are_exact() {
        let request = request();
        let work_id = request.work_id.to_string();
        let workspace_id = request.workspace_id.to_string();
        assert_eq!(
            child_environment(&request, PrivilegeMode::User),
            vec![
                ("HOME", "/home/craxii".to_owned()),
                ("USER", "craxii".to_owned()),
                ("LOGNAME", "craxii".to_owned()),
                ("SHELL", "/bin/bash".to_owned()),
                ("LANG", "C.UTF-8".to_owned()),
                ("PATH", USER_PATH.to_owned()),
                ("CRAXII_WORK_ID", work_id.clone()),
                ("CRAXII_WORKSPACE_ID", workspace_id.clone()),
            ]
        );
        assert_eq!(
            child_environment(&request, PrivilegeMode::Administrative),
            vec![
                ("HOME", "/root".to_owned()),
                ("USER", "root".to_owned()),
                ("LOGNAME", "root".to_owned()),
                ("SHELL", "/bin/bash".to_owned()),
                ("LANG", "C.UTF-8".to_owned()),
                ("PATH", ADMIN_PATH.to_owned()),
                ("CRAXII_WORK_ID", work_id),
                ("CRAXII_WORKSPACE_ID", workspace_id),
            ]
        );

        let command = build_command(Path::new(BASH_PATH), &request);
        assert_eq!(command.as_std().get_program(), BASH_PATH);
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["--noprofile", "--norc", "-o", "pipefail", "-c", "true"]
        );
        assert_eq!(command.as_std().get_envs().count(), 8);
    }
}
