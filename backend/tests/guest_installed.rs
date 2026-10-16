//! Explicitly ignored, credential-free installed Linux qualification. No fixture backend is
//! selected in the product: these tests call the standard installed constructors.
#![cfg(all(target_os = "linux", feature = "test-failpoints", debug_assertions))]
use craxii_server::{
    adapters::{
        artifacts::LocalArtifactStore,
        local_workstation::{LocalWorkstation, LocalWorkstationOptions},
        system_clock::SystemClock,
    },
    distribution::startup::{DistributionConfig, DistributionRuntime},
    domain::*,
    guest_boundary::{QuiescenceState, storage},
    ports::{
        clock::Clock, state_store::BootstrapStateStore, workstation::*, workstation_preparation::*,
    },
    worker_auth::{
        key_store::KeyStore,
        state::{Disposition, IdentityState},
    },
};
use std::os::unix::fs::PermissionsExt;
use std::{path::Path, sync::Arc, time::Duration};
fn fixture() {
    assert_eq!(
        std::fs::read_to_string("/etc/craxii-v001-fixture")
            .unwrap()
            .trim(),
        "craxii-v001-guest-boundary-disposable"
    );
    assert_eq!(
        std::fs::read_to_string("/etc/craxii-v001-fixture-instance")
            .unwrap()
            .trim(),
        "13131313-1313-4313-8313-131313131314"
    );
    assert_eq!(std::env::consts::ARCH, "x86_64");
}
fn config() -> DistributionConfig {
    let mut config = DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://127.0.0.1:19443".into(),
        state_root: storage::STATE.into(),
        artifact_root: storage::ARTIFACTS.into(),
        workspace_root: storage::WORKSPACE.into(),
        profile_public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        power_operation_id: "11111111-1111-4111-8111-111111111111".into(),
        release_id: "qualification-only".into(),
        release_sha256: "1".repeat(64),
    };
    let policy = craxii_server::compute::watchdog::Policy::parse(
        &craxii_server::compute::watchdog::protected_read(
            Path::new("/etc/craxii/compute-watchdog.json"),
            0,
        )
        .unwrap(),
    )
    .unwrap();
    config.power_operation_id = policy.binding.power_operation_id;
    config
}
#[test]
#[ignore = "owned disposable installed guest only; before guard readiness"]
fn seed_synthetic_identity() {
    fixture();
    let guest = storage::load_installed().unwrap();
    let uid = unsafe { nix::libc::geteuid() };
    let model = unsafe { (*nix::libc::getpwnam(c"craxii".as_ptr())).pw_uid };
    let key = KeyStore::open(uid, model, true).unwrap();
    let state = IdentityState {
        version: 1,
        installation_id: guest.text("installation_id").into(),
        customer_id: guest.text("customer_id").into(),
        logical_agent_id: guest.text("logical_agent_id").into(),
        generation: guest.text("generation").into(),
        key_id: key.key_id(),
        highest_epoch: "1".into(),
        binding_id: None,
        pending_signed: None,
        disposition: Disposition::Unbound,
    };
    state.persist(&key).unwrap();
    let p = Path::new(storage::IDENTITY).join("distribution.json");
    if !p.exists() {
        std::fs::write(&p, serde_json::to_vec(&config()).unwrap()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}
async fn request(
    w: &LocalWorkstation,
    clock: &SystemClock,
    command: &str,
    timeout: u64,
) -> ExecutionRequest {
    let cwd = LogicalPathReference::absolute(storage::WORKSPACE).unwrap();
    let prepared = w
        .prepare(WorkstationPreparationRequest {
            operation_id: OperationId::generate(),
            workstation_id: w.workstation_id(),
            expected_generation: w.generation(),
            workspace_id: w.workspace_id(),
            requested_cwd: cwd.clone(),
            required_capability: RequiredWorkstationCapability::ForegroundExecute,
            effective_privilege: PrivilegeMode::User,
        })
        .await
        .unwrap();
    ExecutionRequest {
        operation_id: OperationId::generate(),
        execution_id: ExecutionId::generate(),
        work_id: WorkId::generate(),
        workstation_id: w.workstation_id(),
        expected_generation: w.generation(),
        workspace_id: w.workspace_id(),
        command: command.into(),
        requested_cwd: cwd,
        prepared_cwd: prepared.prepared_cwd,
        effective_privilege: PrivilegeMode::User,
        stdin: ExecutionStdinPolicy::Closed,
        timeout: MonotonicDuration::from_millis(timeout),
        deadline: clock
            .monotonic_now()
            .checked_add(Duration::from_millis(timeout + 15000))
            .unwrap(),
        capture: ExecutionCapturePolicy {
            stdout_max_bytes: 8192,
            stderr_max_bytes: 8192,
        },
        cleanup: ExecutionCleanupPolicy::ProcessGroupAndCgroup,
    }
}
fn scratch_empty() {
    for base in ["/tmp", "/var/tmp", "/dev/shm"] {
        // The root-owned parent is readable to the worker group, but not to tool UID.
        assert_eq!(
            std::fs::read_dir(Path::new(base).join("craxii-tools"))
                .unwrap()
                .count(),
            0
        );
    }
}
async fn installed() -> (DistributionRuntime, LocalWorkstation, Arc<SystemClock>) {
    fixture();
    storage::require_worker_principal().unwrap();
    let runtime = DistributionRuntime::open(config()).await.unwrap();
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    let snapshot = runtime.store.load_bootstrap_snapshot().await.unwrap();
    let clock = Arc::new(SystemClock::new());
    let w = LocalWorkstation::isolated_unmetered_installed(
        &snapshot.workstation,
        &snapshot.workspace,
        LocalWorkstationOptions {
            default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
            configured_workspace_root: Path::new(storage::WORKSPACE).into(),
            read_hard_limit: HARD_FILE_READ_MAX_BYTES,
            artifact_store: Arc::new(
                LocalArtifactStore::initialize(Path::new(storage::ARTIFACTS)).unwrap(),
            ),
            administrative_enabled: false,
            user_switch_launcher: Some("/opt/craxii/current/craxii-workstation-launcher".into()),
            credential_free_direct_execution: false,
            delegated_cgroup_root: Some(storage::TOOLS_CGROUP.into()),
            clock: clock.clone(),
        },
    )
    .unwrap();
    (runtime, w, clock)
}

#[tokio::test]
#[ignore = "owned disposable installed guest and actual systemd delegation only"]
async fn installed_runtime_launcher_and_cleanup() {
    let (runtime, w, clock) = installed().await;
    use std::os::fd::{AsRawFd, FromRawFd};
    let canary = Path::new(storage::STATE).join("locks/qualification-fd");
    std::fs::write(&canary, b"synthetic-only").unwrap();
    let file = std::fs::File::open(&canary).unwrap();
    let raw = unsafe { nix::libc::fcntl(file.as_raw_fd(), nix::libc::F_DUPFD, 200) };
    assert!(raw >= 200);
    let inherited = unsafe { std::fs::File::from_raw_fd(raw) };
    let command = format!(
        "python3 /opt/craxii/qualification/hostile.py {} {}",
        std::process::id(),
        inherited.as_raw_fd()
    );
    let result = w
        .execute(request(&w, &clock, &command, 15000).await)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "hostile probe refused or failed");
    assert!(result.cleanup.confirmed());
    scratch_empty();
    // Parent exit cannot conceal a setsid descendant. Cleanup includes grandchildren and scratch.
    let mut req = request(&w, &clock, "true", 10000).await;
    let marker = Path::new(storage::WORKSPACE)
        .join(format!("qualification-descendant-{}", req.execution_id));
    let command = |marker: &Path, terminal: &str| {
        format!(
            "setsid /bin/bash --noprofile --norc -c 'trap \"\" TERM; sleep 60 & echo $! > \"{}\"; wait' >/dev/null 2>&1 & while ! test -e \"{}\"; do sleep .01; done; {terminal}",
            marker.display(),
            marker.display()
        )
    };
    req.command = command(&marker, "exit 0");
    let result = w.execute(req).await.unwrap();
    assert_eq!(result.result_kind, ExecutionResultKind::Exited);
    assert!(marker.is_file());
    assert!(result.cleanup.confirmed());
    scratch_empty();
    let mut req = request(&w, &clock, "true", 2000).await;
    let marker = Path::new(storage::WORKSPACE)
        .join(format!("qualification-descendant-{}", req.execution_id));
    req.command = command(&marker, "exec sleep 60");
    let result = w.execute(req).await.unwrap();
    assert_eq!(result.result_kind, ExecutionResultKind::TimedOut);
    assert!(marker.is_file());
    assert!(result.cleanup.confirmed());
    scratch_empty();
    let mut req = request(&w, &clock, "true", 15000).await;
    let marker = Path::new(storage::WORKSPACE)
        .join(format!("qualification-descendant-{}", req.execution_id));
    req.command = command(&marker, "exec sleep 60");
    let id = req.execution_id;
    let running = tokio::spawn({
        let w = w.clone();
        async move { w.execute(req).await.unwrap() }
    });
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    while !marker.is_file() {
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(storage::model_process_count().unwrap() >= 3);
    assert_eq!(
        runtime
            .quiescence_with_execution(w.execution_observation())
            .await
            .unwrap()
            .state(),
        QuiescenceState::Busy
    );
    let cancel = w
        .cancel_execution(ExecutionCancellationRequest {
            operation_id: OperationId::generate(),
            execution_id: id,
            workstation_id: w.workstation_id(),
            expected_generation: w.generation(),
        })
        .await
        .unwrap();
    assert_eq!(cancel.state, ExecutionCancellationState::Confirmed);
    let result = running.await.unwrap();
    assert_eq!(result.result_kind, ExecutionResultKind::Cancelled);
    assert!(result.cleanup.confirmed());
    scratch_empty();
    assert_eq!(storage::tool_descendant_count().unwrap(), 0);
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    let result = w
        .execute(
            request(
                &w,
                &clock,
                "python3 -c 'import sys;sys.stdout.write(\"x\"*1048576);sys.stderr.write(\"y\"*1048576)'",
                10000,
            )
            .await,
        )
        .await
        .unwrap();
    assert!(result.cleanup.confirmed());
    let out = result.stdout.unwrap();
    assert_eq!(out.observed_bytes, 1048576);
    assert_eq!(out.captured_bytes, 8192);
    assert!(out.truncated);
    let err = result.stderr.unwrap();
    assert_eq!(err.observed_bytes, 1048576);
    assert_eq!(err.captured_bytes, 8192);
    assert!(err.truncated);
    for (mode, file, key) in [
        ("pids", "pids.events", "max"),
        ("memory", "memory.events", "oom_kill"),
        ("cpu", "cpu.stat", "nr_throttled"),
    ] {
        let command =
            format!("python3 /opt/craxii/qualification/resources.py {mode} & wait; sleep 60");
        let req = request(&w, &clock, &command, 60000).await;
        let id = req.execution_id;
        let running = tokio::spawn({
            let w = w.clone();
            async move { w.execute(req).await.unwrap() }
        });
        let end = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let path = Path::new(storage::TOOLS_CGROUP)
                .join(id.to_string())
                .join(file);
            let hit = std::fs::read_to_string(path).ok().is_some_and(|s| {
                s.lines().any(|l| {
                    l.split_once(' ')
                        .is_some_and(|(k, v)| k == key && v.parse::<u64>().unwrap_or(0) > 0)
                })
            });
            if hit {
                break;
            }
            assert!(
                tokio::time::Instant::now() < end,
                "installed resource event absent: {mode}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let cancel = w
            .cancel_execution(ExecutionCancellationRequest {
                operation_id: OperationId::generate(),
                execution_id: id,
                workstation_id: w.workstation_id(),
                expected_generation: w.generation(),
            })
            .await
            .unwrap();
        assert_eq!(cancel.state, ExecutionCancellationState::Confirmed);
        assert!(running.await.unwrap().cleanup.confirmed());
        scratch_empty();
        println!("PASS installed-resource-{mode}");
    }
    // Concurrent executions have the same UID. The second shell must still be denied the
    // first shell's private scratch by Landlock, with EACCES rather than an unrelated error.
    let req = request(
        &w,
        &clock,
        "echo harmless > \"$TMPDIR/cross-canary\" && exec 9< \"$TMPDIR/cross-canary\" && exec sleep 60",
        30000,
    )
    .await;
    let id = req.execution_id;
    let running = tokio::spawn({
        let w = w.clone();
        async move { w.execute(req).await.unwrap() }
    });
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let path = Path::new(storage::TOOLS_CGROUP)
            .join(id.to_string())
            .join("cgroup.procs");
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|v| v.lines().next().and_then(|p| p.parse::<u32>().ok()))
            && std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()
                .as_deref()
                == Some("sleep\n")
        {
            break pid;
        }
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let command = format!(
        "python3 -c 'import os,errno; paths=[\"/tmp/craxii-tools/{id}/cross-canary\",\"/proc/{pid}/root/tmp/craxii-tools/{id}/cross-canary\",\"/proc/{pid}/fd/9\"];\nfor p in paths:\n try: fd=os.open(p,os.O_RDONLY)\n except OSError as e: assert e.errno in (errno.EACCES,errno.EPERM) or (p.startswith(\"/proc/\") and e.errno==errno.ENOENT and not os.path.exists(\"/proc/{pid}\"))\n else: os.close(fd);raise AssertionError()'"
    );
    let result = w
        .execute(request(&w, &clock, &command, 10000).await)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.cleanup.confirmed());
    // hidepid may return ENOENT for the entire live target process. The trusted observer
    // confirms that target still exists, distinguishing kernel hiding from a dead PID.
    assert_eq!(
        std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap(),
        "sleep\n"
    );
    w.cancel_execution(ExecutionCancellationRequest {
        operation_id: OperationId::generate(),
        execution_id: id,
        workstation_id: w.workstation_id(),
        expected_generation: w.generation(),
    })
    .await
    .unwrap();
    assert!(running.await.unwrap().cleanup.confirmed());
    scratch_empty();
    assert_eq!(
        runtime
            .quiescence_with_execution(w.execution_observation())
            .await
            .unwrap()
            .state(),
        QuiescenceState::LocallyQuiescent
    );
    runtime.shutdown().await.unwrap();
    println!("PASS installed-runtime-hostile-normal-timeout-cancel-descendants-scratch-quiescence");
}

#[tokio::test]
#[ignore = "owned installed guest; root logging and abstract-socket positive controls required"]
async fn installed_ipc_boundary() {
    let (runtime, w, clock) = installed().await;
    // Actual trusted Rust service HTTPS, with certificate+hostname validation and synthetic
    // authentication. The owned collector rejects missing auth. No production trust override.
    let http = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .add_root_certificate(
            reqwest::Certificate::from_pem(
                &std::fs::read("/opt/craxii/qualification/network-ca.pem").unwrap(),
            )
            .unwrap(),
        )
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert_eq!(
        http.get("https://127.0.0.1:19444/")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    use std::os::fd::{AsRawFd, FromRawFd};
    let socket = std::os::unix::net::UnixDatagram::unbound().unwrap();
    let raw = unsafe { nix::libc::fcntl(socket.as_raw_fd(), nix::libc::F_DUPFD, 300) };
    assert!(raw >= 300);
    let inherited = unsafe { std::fs::File::from_raw_fd(raw) };
    // Execute twice with an actual cancelled process tree in between. No per-worker policy state
    // or first-launch-only setup may control enforcement. The external fixture also restarts units.
    for round in 0..2 {
        assert_eq!(
            http.get("https://127.0.0.1:19444/")
                .bearer_auth("synthetic-remediation02-worker")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "synthetic-worker-https"
        );
        let result = w
            .execute(
                request(
                    &w,
                    &clock,
                    &format!(
                        "python3 /opt/craxii/qualification/ipc.py {}",
                        inherited.as_raw_fd()
                    ),
                    15000,
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0), "installed IPC probe failed");
        assert!(result.cleanup.confirmed());
        scratch_empty();
        if round == 0 {
            let req = request(&w, &clock, "exec sleep 60", 30000).await;
            let id = req.execution_id;
            let running = tokio::spawn({
                let w = w.clone();
                async move { w.execute(req).await.unwrap() }
            });
            let end = tokio::time::Instant::now() + Duration::from_secs(10);
            while storage::model_process_count().unwrap() == 0 {
                assert!(tokio::time::Instant::now() < end);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let cancel = w
                .cancel_execution(ExecutionCancellationRequest {
                    operation_id: OperationId::generate(),
                    execution_id: id,
                    workstation_id: w.workstation_id(),
                    expected_generation: w.generation(),
                })
                .await
                .unwrap();
            assert_eq!(cancel.state, ExecutionCancellationState::Confirmed);
            assert!(running.await.unwrap().cleanup.confirmed());
        }
    }
    let canary = format!("qualification-ipc-workspace-{}", ExecutionId::generate());
    let result = w
        .execute(
            request(
                &w,
                &clock,
                &format!("test \"$PWD\" = /srv/craxii/workspaces/primary && printf synthetic > {canary} && test \"$(cat {canary})\" = synthetic && rm {canary}"),
                10000,
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.cleanup.confirmed());
    scratch_empty();
    assert_eq!(
        runtime
            .quiescence_with_execution(w.execution_observation())
            .await
            .unwrap()
            .state(),
        QuiescenceState::LocallyQuiescent
    );
    runtime.shutdown().await.unwrap();
    println!("PASS installed-ipc-denial-descendants-cancel-replacement-workspace-quiescence");
}

#[tokio::test]
#[ignore = "owned installed guest; root controller removes policy after worker startup"]
async fn installed_missing_network_refuses_tool() {
    let (runtime, w, clock) = installed().await;
    let canary = format!("qualification-missing-network-{}", ExecutionId::generate());
    std::fs::write("/var/lib/craxii/remediation02-network-ready", &canary).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !Path::new("/run/remediation02-network-removed").exists() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let result = w
        .execute(request(&w, &clock, &format!("printf unsafe > {canary}"), 10000).await)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(1));
    assert!(result.cleanup.confirmed());
    assert!(!Path::new(storage::WORKSPACE).join(&canary).exists());
    assert_eq!(storage::model_process_count().unwrap(), 0);
    scratch_empty();
    runtime.shutdown().await.unwrap();
    println!("PASS installed-launch-refused-after-active-policy-removal");
}

#[tokio::test]
#[ignore = "owned installed guest; root fixture narrows tmpfs for bounded ENOSPC"]
async fn installed_scratch_pressure_and_cleanup_uncertainty() {
    let (runtime, w, clock) = installed().await;
    assert!(std::env::var_os("CRAXII_SYNTHETIC_SERVICE_SECRET").is_some());
    let mut req = request(&w, &clock, "true", 20000).await;
    req.command = format!(
        "python3 /opt/craxii/qualification/resources.py scratch-hold {}",
        req.execution_id
    );
    let marker =
        Path::new(storage::WORKSPACE).join(format!("qualification-pressure-{}", req.execution_id));
    let running = tokio::spawn({
        let w = w.clone();
        async move { w.execute(req).await.unwrap() }
    });
    let end = tokio::time::Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    storage::pressure().unwrap();
    let mut temp = std::mem::MaybeUninit::<nix::libc::statvfs>::uninit();
    assert_eq!(
        unsafe { nix::libc::statvfs(c"/tmp".as_ptr(), temp.as_mut_ptr()) },
        0
    );
    assert_eq!(unsafe { temp.assume_init() }.f_bavail, 0);
    // A durable trusted heartbeat is written while untrusted scratch is actually full.
    use craxii_server::ports::state_store::{HeartbeatRuntimeRequest, RuntimeStateStore};
    let runtime_id = runtime.hello()["body"]["runtime_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    runtime
        .store
        .heartbeat_runtime(HeartbeatRuntimeRequest {
            runtime_instance_id: runtime_id,
            observed_at: UtcTimestamp::from_offset_datetime(time::OffsetDateTime::now_utc())
                .unwrap(),
        })
        .await
        .unwrap();
    let result = running.await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.cleanup.confirmed());
    scratch_empty();
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        "persistent-workspace-canary"
    );
    // An adversarial scratch mode change is conservatively unsafe; it must not disappear
    // when the terminal entry leaves the registry or authorize another dispatch.
    let result = w
        .execute(request(&w, &clock, "python3 -c 'import os; fd=os.open(os.environ[\"TMPDIR\"],os.O_RDONLY);\nfor _ in range(1400):\n os.mkdir(\"d\",dir_fd=fd); new=os.open(\"d\",os.O_RDONLY|os.O_DIRECTORY,dir_fd=fd);os.close(fd);fd=new\nos.symlink(\"/var/lib/craxii\",\"outside\",dir_fd=fd);os.close(fd)' && chmod 000 \"$TMPDIR\"; exit 0", 10000).await)
        .await
        .unwrap();
    assert_eq!(result.result_kind, ExecutionResultKind::CleanupFailed);
    assert_eq!(result.certainty, Certainty::OutcomeUnknown);
    assert!(!result.cleanup.confirmed());
    assert!(w.execution_observation().pending_cleanup > 0);
    assert_eq!(
        runtime
            .quiescence_with_execution(w.execution_observation())
            .await
            .unwrap()
            .state(),
        QuiescenceState::Busy
    );
    assert!(
        w.execute(request(&w, &clock, "true", 1000).await)
            .await
            .is_err()
    );
    assert_eq!(storage::model_process_count().unwrap(), 0);
    runtime.shutdown().await.unwrap();
    println!("PASS scratch-ENOSPC-trusted-write-workspace-preservation-unsafe-cleanup-sticky");
}

#[tokio::test]
#[ignore = "owned installed guest; external root fixture kills the worker process"]
async fn installed_worker_interruption() {
    let (runtime, w, clock) = installed().await;
    let req=request(&w,&clock,"setsid /bin/bash --noprofile --norc -c 'trap \"\" TERM; (sleep 45; echo late > qualification-interruption-late) & touch qualification-interruption-ready; wait' >/dev/null 2>&1 & exec sleep 60",60000).await;
    let id = req.execution_id;
    std::fs::write(
        Path::new(storage::STATE).join("locks/qualification-interruption-id"),
        id.to_string(),
    )
    .unwrap();
    let result = w.execute(req).await.unwrap();
    let _ = runtime;
    panic!(
        "external root interruption did not occur: {:?}",
        result.result_kind
    );
}

#[tokio::test]
#[ignore = "owned installed guest; root controller injects mount/proc/cgroup visibility faults"]
async fn installed_observation_faults() {
    let (runtime, w, _) = installed().await;
    let root = Path::new("/run/craxii-worker/qualification");
    async fn phase(root: &Path, phase: &str, control: &str) {
        std::fs::write(root.join("phase"), phase).unwrap();
        let end = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if std::fs::read_to_string(root.join("control"))
                .ok()
                .as_deref()
                == Some(control)
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < end,
                "root observation controller timed out"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    phase(root, "await_mount", "mount").await;
    match runtime
        .quiescence_with_execution(w.execution_observation())
        .await
    {
        Ok(q) => assert_eq!(q.state(), QuiescenceState::Unknown),
        Err(_) => {
            panic!("invalid mount obscured the integrated observation instead of reporting unknown")
        }
    }
    phase(root, "mount_checked", "restored").await;
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    phase(root, "await_incomplete", "incomplete").await;
    let q = runtime.quiescence_observation().await.unwrap();
    assert!(q.descendants.is_none());
    assert!(!q.visibility_complete);
    assert_eq!(q.state(), QuiescenceState::Unknown);
    phase(root, "incomplete_checked", "complete").await;
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    phase(root, "await_rogue", "rogue").await;
    let q = runtime.quiescence_observation().await.unwrap();
    assert!(q.descendants.unwrap() > 0);
    assert_eq!(q.state(), QuiescenceState::Busy);
    phase(root, "rogue_checked", "none").await;
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    let empty = Path::new(storage::TOOLS_CGROUP).join(ExecutionId::generate().to_string());
    std::fs::create_dir(&empty).unwrap();
    assert!(
        runtime
            .quiescence_observation()
            .await
            .unwrap()
            .pending_cleanup
            > 0
    );
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::Busy
    );
    std::fs::remove_dir(empty).unwrap();
    assert_eq!(
        runtime.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    std::fs::write(root.join("phase"), "done").unwrap();
    runtime.shutdown().await.unwrap();
    println!("PASS actual-mount-incomplete-visibility-off-cgroup-UID-empty-cgroup-observations");
}

#[tokio::test]
#[ignore = "owned installed guest; verify absolute cleanup budget and conservative uncertainty"]
async fn installed_absolute_cleanup_deadline() {
    let (runtime, w, clock) = installed().await;
    let mut req = request(&w, &clock, "true", 20000).await;
    req.command="python3 -c 'import os; p=os.environ[\"TMPDIR\"];\nfor i in range(8000):\n fd=os.open(p+\"/\"+str(i),os.O_CREAT|os.O_WRONLY,0o600);os.close(fd)' && exec sleep 60".into();
    req.deadline = clock
        .monotonic_now()
        .checked_add(Duration::from_millis(1500))
        .unwrap();
    let started = std::time::Instant::now();
    let result = w.execute(req).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "cleanup exceeded the absolute caller deadline"
    );
    assert!(result.start_observed);
    assert!(!result.cleanup.confirmed());
    assert_eq!(result.certainty, Certainty::OutcomeUnknown);
    assert_eq!(
        runtime
            .quiescence_with_execution(w.execution_observation())
            .await
            .unwrap()
            .state(),
        QuiescenceState::Busy
    );
    runtime.shutdown().await.unwrap();
    println!("PASS absolute-deadline-no-unbudgeted-scratch-cleanup-unknown-busy");
}

#[tokio::test]
#[ignore = "owned installed guest; actual privilege-drop file reader"]
async fn installed_reader_denies_trusted_state() {
    let (runtime, w, clock) = installed().await;
    let tag = ExecutionId::generate();
    let canary = format!("{}/qualification-reader-{tag}", storage::WORKSPACE);
    let link = format!("{}/qualification-reader-link-{tag}", storage::WORKSPACE);
    let command = format!(
        "printf harmless-reader-canary > {canary}; ln -s /var/lib/craxii/db/craxii.sqlite3 {link}"
    );
    let result = w
        .execute(request(&w, &clock, &command, 10000).await)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.cleanup.confirmed());
    for path in [
        canary.as_str(),
        "/var/lib/craxii-worker/identity/worker.pk8",
        "/var/lib/craxii/db/craxii.sqlite3",
        "/etc/craxii/guest-bootstrap.json",
        link.as_str(),
    ] {
        let result = w
            .read_file(FileReadRequest {
                operation_id: OperationId::generate(),
                workstation_id: w.workstation_id(),
                expected_generation: w.generation(),
                workspace_id: w.workspace_id(),
                path: LogicalPathReference::absolute(path).unwrap(),
                max_bytes: HARD_FILE_READ_MAX_BYTES,
                deadline: clock
                    .monotonic_now()
                    .checked_add(Duration::from_secs(10))
                    .unwrap(),
            })
            .await;
        if path == canary {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.unwrap_err().kind(),
                WorkstationErrorKind::PermissionDenied
            );
        }
    }
    runtime.shutdown().await.unwrap();
    println!("PASS actual-reader-workspace-control-trusted-key-journal-config-symlink-denial");
}

#[tokio::test]
#[ignore = "owned installed guest; mandatory compute permit gate"]
async fn installed_compute_requires_grant() {
    let (runtime, fixture_adapter, clock) = installed().await;
    let snapshot = runtime.store.load_bootstrap_snapshot().await.unwrap();
    let normal = LocalWorkstation::new_installed(
        &snapshot.workstation,
        &snapshot.workspace,
        LocalWorkstationOptions {
            default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
            configured_workspace_root: Path::new(storage::WORKSPACE).into(),
            read_hard_limit: HARD_FILE_READ_MAX_BYTES,
            artifact_store: Arc::new(
                LocalArtifactStore::initialize(Path::new(storage::ARTIFACTS)).unwrap(),
            ),
            administrative_enabled: false,
            user_switch_launcher: Some("/opt/craxii/current/craxii-workstation-launcher".into()),
            credential_free_direct_execution: false,
            delegated_cgroup_root: Some(storage::TOOLS_CGROUP.into()),
            clock: clock.clone(),
        },
    )
    .unwrap();
    let canary = format!("compute-missing-permit-{}", ExecutionId::generate());
    let req = request(&fixture_adapter, &clock, &format!("touch {canary}"), 1000).await;
    assert!(normal.execute(req).await.is_err());
    assert!(!Path::new(storage::WORKSPACE).join(canary).exists());
    assert_eq!(storage::model_process_count().unwrap(), 0);
    runtime.shutdown().await.unwrap();
    println!("PASS installed mandatory compute permit refused before tool effect");
}
