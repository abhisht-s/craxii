#[cfg(target_os = "linux")]
fn main() {
    if run().is_err() {
        fixed_poweroff();
        std::process::exit(1);
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(1);
}
#[cfg(target_os = "linux")]
fn fixed_poweroff() {
    // Fixed local OS operation only. No user input, shell, cloud operation or wait on a child.
    fixed_request(&["--no-block", "poweroff"]);
}
#[cfg(target_os = "linux")]
fn fixed_request(arguments: &[&str]) {
    // Wait only for the bounded local queue acknowledgement. Exiting immediately after
    // spawn lets systemd kill this child with its service cgroup before it queues poweroff.
    if let Ok(mut child) = std::process::Command::new("/usr/bin/systemctl")
        .args(arguments)
        .env_clear()
        .spawn()
    {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(1000);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Err(_) => break,
                Ok(None) => {}
            }
            if std::time::Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
#[cfg(target_os = "linux")]
fn run() -> craxii_server::compute::ComputeResult<()> {
    use craxii_server::compute::{
        ComputeError,
        watchdog::{self, Action, Policy, Receipt},
    };
    use std::{
        fs::OpenOptions,
        os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
        path::Path,
        time::Duration,
    };
    let denied = || ComputeError("compute_authorization_denied");
    if unsafe { nix::libc::geteuid() } != 0 || std::env::args_os().len() != 1 {
        return Err(denied());
    }
    let policy = Policy::parse(&watchdog::protected_read(
        Path::new("/etc/craxii/compute-watchdog.json"),
        0,
    )?)?;
    // Protected identity projection is independent of storage initialization. The
    // existing bootstrap independently validates the complete storage contract.
    let guest_bytes = watchdog::protected_read(Path::new("/etc/craxii/guest-bootstrap.json"), 0)?;
    let guest = craxii_server::distribution_contracts::raw_json::parse_bounded(
        &guest_bytes,
        craxii_server::distribution_contracts::raw_json::CONTROL_LIMITS,
    )
    .map_err(|_| denied())?;
    for (field, expected) in [
        ("customer_id", &policy.binding.customer_id),
        ("logical_agent_id", &policy.binding.logical_agent_id),
        ("installation_id", &policy.binding.installation_id),
        ("generation", &policy.binding.generation),
    ] {
        if guest[field].as_str() != Some(expected.as_str()) {
            return Err(denied());
        }
    }
    if guest["provenance"]["kind"] != "trusted_infrastructure" {
        return Err(denied());
    }
    let state = Path::new("/var/lib/craxii-compute/receipt.json");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open("/var/lib/craxii-compute/supervisor.lock")
        .map_err(|_| denied())?;
    if unsafe { nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) } != 0 {
        return Err(denied());
    }
    let clock = watchdog::clock_sample()?;
    let mut receipt = match watchdog::protected_read(state, 0) {
        Ok(bytes) => {
            let previous = Receipt::previous(&bytes)?;
            if previous.deadline.boot_id == clock.boot_id {
                if previous.binding != policy.binding {
                    return Err(denied());
                }
                // Legacy resettable projections cannot be resumed on the same boot.
                Receipt::parse(&bytes, &policy, &clock)?
            } else {
                let permit = watchdog::protected_read(
                    Path::new("/var/lib/craxii-compute/bootstrap.json"),
                    0,
                )?;
                let r = Receipt::consume_bootstrap(&permit, &policy, &clock, Some(&previous))?;
                watchdog::atomic_write(state, &serde_json::to_vec(&r).map_err(|_| denied())?)?;
                r
            }
        }
        Err(_) => {
            // Existing corrupt/inaccessible state must not fall back to an old permit.
            if state.try_exists().map_err(|_| denied())?
                || Path::new("/var/lib/craxii-compute/bootstrap-consumed")
                    .try_exists()
                    .map_err(|_| denied())?
            {
                return Err(denied());
            }
            let permit =
                watchdog::protected_read(Path::new("/var/lib/craxii-compute/bootstrap.json"), 0)?;
            let r = Receipt::consume_bootstrap(&permit, &policy, &clock, None)?;
            watchdog::atomic_write(
                Path::new("/var/lib/craxii-compute/bootstrap-consumed"),
                b"consumed",
            )?;
            watchdog::atomic_write(state, &serde_json::to_vec(&r).map_err(|_| denied())?)?;
            r
        }
    };
    let worker_uid = std::fs::metadata("/var/lib/craxii-worker/compute-inbox")
        .map_err(|_| denied())
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid()
        })?;
    if worker_uid == 0 {
        return Err(denied());
    }
    // Prepare the small frozen compute validator before READY starts the periodic
    // health obligation. Its initialization remains within the bounded startup budget.
    craxii_server::compute::authorization::prepare_verifier()?;
    // Readiness is emitted only after protected funding and cutoff are durable.
    // No model-visible socket or command is accepted.
    if std::env::var_os("NOTIFY_SOCKET").as_deref()
        != Some(std::ffi::OsStr::new("/run/systemd/notify"))
    {
        return Err(denied());
    }
    std::os::unix::net::UnixDatagram::unbound()
        .and_then(|s| s.send_to(b"READY=1", "/run/systemd/notify"))
        .map_err(|_| denied())?;
    loop {
        let clock = watchdog::clock_sample()?;
        if receipt.deadline.observe(&clock).unwrap_or(false) {
            watchdog::atomic_write(state, &serde_json::to_vec(&receipt).map_err(|_| denied())?)?;
        }
        if let Ok(bytes) = watchdog::protected_read(
            Path::new("/var/lib/craxii-worker/compute-inbox/grant.json"),
            worker_uid,
        ) {
            // Invalid input never changes existing authority. A valid renewal after draining
            // cannot reopen admission. Acceptance is durable before the worker can observe it.
            if receipt.accept(&bytes, &policy, &clock).unwrap_or(false) {
                watchdog::atomic_write(
                    state,
                    &serde_json::to_vec(&receipt).map_err(|_| denied())?,
                )?;
            }
        }
        match receipt.action(&policy, &clock)? {
            Action::Running | Action::BootstrapOnly => {}
            Action::Drain => {
                if !receipt.deadline.draining {
                    receipt.deadline.draining = true;
                    watchdog::atomic_write(
                        state,
                        &serde_json::to_vec(&receipt).map_err(|_| denied())?,
                    )?;
                    fixed_request(&["--no-block", "stop", "craxii-distribution.service"]);
                }
            }
            Action::Poweroff => {
                receipt.deadline.draining = true;
                receipt.shutdown_requested = true;
                watchdog::atomic_write(
                    state,
                    &serde_json::to_vec(&receipt).map_err(|_| denied())?,
                )?;
                fixed_poweroff();
                return Ok(());
            }
        }
        std::os::unix::net::UnixDatagram::unbound()
            .and_then(|s| s.send_to(b"WATCHDOG=1", "/run/systemd/notify"))
            .map_err(|_| denied())?;
        std::thread::sleep(Duration::from_millis(100));
    }
}
