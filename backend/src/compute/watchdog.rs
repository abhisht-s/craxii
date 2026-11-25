//! Independent root supervisor state, fixed file transport and restart-safe deadlines.
use super::{
    ComputeResult,
    authorization::{
        Binding, BootstrapAuthority, ClockSample, Deadline, RunningAuthority, micros, text,
    },
    denied,
};
use crate::distribution_contracts::{
    canonical::canonical_bytes,
    raw_json::{CONTROL_LIMITS, parse_bounded},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub contract_version: u8,
    pub binding: Binding,
    pub drain_ms: u64,
    pub stop_ms: u64,
    pub skew_ms: u64,
}
impl Policy {
    pub fn parse(bytes: &[u8]) -> ComputeResult<Self> {
        let v = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        let p: Self = serde_json::from_value(v).map_err(|_| denied())?;
        p.binding.validate()?;
        if p.contract_version != 1
            || p.drain_ms == 0
            || p.drain_ms > 45000
            || p.stop_ms == 0
            || p.stop_ms > 60000
            || p.skew_ms == 0
            || p.skew_ms > 5000
        {
            return Err(denied());
        }
        Ok(p)
    }
    pub fn margins_us(&self) -> ComputeResult<u64> {
        self.drain_ms
            .checked_add(self.stop_ms)
            .and_then(|v| v.checked_add(self.skew_ms))
            .and_then(|v| v.checked_mul(1000))
            .ok_or_else(denied)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub contract_version: u8,
    pub binding: Binding,
    pub deadline: Deadline,
    pub grant_id: Option<String>,
    pub grant_digest: Option<String>,
    pub accepted_issued_us: String,
    pub consumed_permits: Vec<String>,
    pub authorization: Value,
    pub shutdown_requested: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Running,
    BootstrapOnly,
    Drain,
    Poweroff,
}
impl Receipt {
    pub fn previous(bytes: &[u8]) -> ComputeResult<Self> {
        let v = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        let r: Self = serde_json::from_value(v).map_err(|_| denied())?;
        let policy = Policy {
            contract_version: 1,
            binding: r.binding.clone(),
            drain_ms: 1,
            stop_ms: 1,
            skew_ms: 1,
        };
        let clock = ClockSample {
            boot_id: r.deadline.boot_id.clone(),
            ticks_us: r.deadline.sampled_ticks_us,
            utc_us: r.deadline.sampled_utc_us.parse().map_err(|_| denied())?,
            trusted: true,
        };
        Self::parse_inner(bytes, &policy, &clock, true)
    }
    pub fn consume_bootstrap(
        bytes: &[u8],
        policy: &Policy,
        clock: &ClockSample,
        previous: Option<&Receipt>,
    ) -> ComputeResult<Self> {
        let a = BootstrapAuthority::verify(bytes, &policy.binding, clock)?;
        let permit = text(&a.permit, "permit_id")?.to_owned();
        let mut used = previous
            .map(|r| r.consumed_permits.clone())
            .unwrap_or_default();
        if used.len() >= 1024
            || used.contains(&permit)
            || previous.is_some_and(|r| r.deadline.boot_id == clock.boot_id)
        {
            return Err(denied());
        }
        if let Some(old) = previous
            && (old.binding.customer_id != policy.binding.customer_id
                || old.binding.logical_agent_id != policy.binding.logical_agent_id
                || policy
                    .binding
                    .generation
                    .parse::<u64>()
                    .map_err(|_| denied())?
                    < old
                        .binding
                        .generation
                        .parse::<u64>()
                        .map_err(|_| denied())?
                || policy
                    .binding
                    .authorization_epoch
                    .parse::<u64>()
                    .map_err(|_| denied())?
                    < old
                        .binding
                        .authorization_epoch
                        .parse::<u64>()
                        .map_err(|_| denied())?
                || policy.binding.generation == old.binding.generation
                    && policy.binding.installation_id != old.binding.installation_id
                || old.binding.power_operation_id == policy.binding.power_operation_id
                || micros(&a.permit, "issued_at")?
                    <= old
                        .accepted_issued_us
                        .parse::<i128>()
                        .map_err(|_| denied())?)
        {
            return Err(denied());
        }
        used.push(permit);
        Ok(Self {
            contract_version: 2,
            binding: policy.binding.clone(),
            deadline: a.deadline,
            grant_id: None,
            grant_digest: None,
            accepted_issued_us: micros(&a.permit, "issued_at")?.to_string(),
            consumed_permits: used,
            authorization: parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?,
            shutdown_requested: false,
        })
    }
    pub fn parse(bytes: &[u8], policy: &Policy, c: &ClockSample) -> ComputeResult<Self> {
        Self::parse_inner(bytes, policy, c, false)
    }
    fn parse_inner(
        bytes: &[u8],
        policy: &Policy,
        c: &ClockSample,
        history: bool,
    ) -> ComputeResult<Self> {
        let v = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        if v["contract_version"] == 2
            && (!v["deadline"]["ticks_cutoff_us"].is_string()
                || !v["deadline"]["sampled_ticks_us"].is_string())
        {
            return Err(denied());
        }
        let r: Self = serde_json::from_value(v).map_err(|_| denied())?;
        let legacy = r.contract_version == 1 && r.deadline.projection_offset_us.is_none();
        if !(r.contract_version == 2 && r.deadline.projection_offset_us.is_some()
            || history && legacy)
            || r.binding != policy.binding
            || r.consumed_permits.len() > 1024
            || r.deadline.boot_id != c.boot_id
        {
            return Err(denied());
        }
        let bytes = canonical_bytes(&r.authorization).map_err(|_| denied())?;
        // Validate cryptography/context even when expired, using the original saved sample.
        let original = ClockSample {
            boot_id: r.deadline.boot_id.clone(),
            ticks_us: r.deadline.sampled_ticks_us,
            utc_us: r.deadline.sampled_utc_us.parse().map_err(|_| denied())?,
            trusted: true,
        };
        let signed = if let Some(id) = &r.grant_id {
            let a = RunningAuthority::verify(&bytes, &policy.binding, &original)?;
            if a.grant_id()? != id || r.grant_digest.as_ref() != Some(&a.digest) {
                return Err(denied());
            }
            a.deadline
        } else {
            let a = BootstrapAuthority::verify(&bytes, &policy.binding, &original)?;
            if !r
                .consumed_permits
                .iter()
                .any(|s| Some(s.as_str()) == a.permit["permit_id"].as_str())
            {
                return Err(denied());
            }
            a.deadline
        };
        let mut signed = signed;
        if legacy {
            // v1 stored expires_at even when max_elapsed_ms imposed an earlier bound.
            let payload = if r.grant_id.is_some() {
                &r.authorization["body"]["grant"]
            } else {
                &r.authorization["permit"]
            };
            signed.utc_cutoff_us = micros(payload, "expires_at")?.to_string();
        } else {
            // Verify the stored projection cannot be weaker than the signed acceptance sample.
            let offset: i128 = r
                .deadline
                .projection_offset_us
                .as_ref()
                .ok_or_else(denied)?
                .parse()
                .map_err(|_| denied())?;
            let original_offset = original.utc_us - i128::from(original.ticks_us);
            if offset < original_offset {
                return Err(denied());
            }
            signed.inherit_projection(&r.deadline)?;
        }
        if r.accepted_issued_us
            != micros(
                if r.grant_id.is_some() {
                    &r.authorization["body"]["grant"]
                } else {
                    &r.authorization["permit"]
                },
                "issued_at",
            )?
            .to_string()
            || r.deadline.utc_cutoff_us != signed.utc_cutoff_us
            || r.deadline.ticks_cutoff_us > signed.ticks_cutoff_us
        {
            return Err(denied());
        }
        Ok(r)
    }
    pub fn accept(
        &mut self,
        bytes: &[u8],
        policy: &Policy,
        c: &ClockSample,
    ) -> ComputeResult<bool> {
        if self.action(policy, c)? != Action::Running
            && self.action(policy, c)? != Action::BootstrapOnly
            || self.deadline.draining
            || self.shutdown_requested
        {
            return Err(denied());
        }
        let mut a = RunningAuthority::verify(bytes, &policy.binding, c)?;
        a.deadline.inherit_projection(&self.deadline)?;
        if self.grant_id.as_deref() == Some(a.grant_id()?) {
            if self.grant_digest.as_ref() != Some(&a.digest) {
                return Err(denied());
            }
            return Ok(false);
        }
        let issued = micros(a.grant(), "issued_at")?;
        if issued
            <= self
                .accepted_issued_us
                .parse::<i128>()
                .map_err(|_| denied())?
            || self.grant_id.is_some()
                && a.deadline
                    .utc_cutoff_us
                    .parse::<i128>()
                    .map_err(|_| denied())?
                    < self
                        .deadline
                        .utc_cutoff_us
                        .parse::<i128>()
                        .map_err(|_| denied())?
        {
            return Err(denied());
        }
        self.grant_id = Some(a.grant_id()?.to_owned());
        self.grant_digest = Some(a.digest);
        self.deadline = a.deadline;
        self.accepted_issued_us = issued.to_string();
        self.authorization = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        Ok(true)
    }
    pub fn action(&self, p: &Policy, c: &ClockSample) -> ComputeResult<Action> {
        let remaining = match self.deadline.remaining_us(c) {
            Ok(v) => v,
            Err(_) => return Ok(Action::Poweroff),
        };
        if self.shutdown_requested
            || remaining
                <= p.stop_ms
                    .checked_add(p.skew_ms)
                    .and_then(|v| v.checked_mul(1000))
                    .ok_or_else(denied)?
        {
            return Ok(Action::Poweroff);
        }
        if self.deadline.draining || remaining <= p.margins_us()? {
            return Ok(Action::Drain);
        }
        if self.grant_id.is_some() && c.trusted {
            Ok(Action::Running)
        } else {
            Ok(Action::BootstrapOnly)
        }
    }
    pub fn worker_authority(&self, p: &Policy, c: &ClockSample) -> ComputeResult<RunningAuthority> {
        if self.action(p, c)? != Action::Running {
            return Err(denied());
        }
        let mut a = RunningAuthority::verify(
            &canonical_bytes(&self.authorization).map_err(|_| denied())?,
            &p.binding,
            c,
        )?;
        a.restore_deadline(&self.deadline, c)?;
        Ok(a)
    }
}
/// No-follow each component, bounded regular single-link input, exact owning UID and no
/// model/group-writable parent. The expected worker UID may write only its own handoff.
#[cfg(unix)]
pub fn protected_read(path: &Path, owner: u32) -> ComputeResult<Vec<u8>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(denied());
        }
        current.push(component);
        let m = fs::symlink_metadata(&current).map_err(|_| denied())?;
        if m.file_type().is_symlink() || m.mode() & 0o022 != 0 || m.uid() != 0 && m.uid() != owner {
            return Err(denied());
        }
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| denied())?;
    let m = file.metadata().map_err(|_| denied())?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != owner
        || m.mode() & 0o027 != 0
        || m.len() > 16384
    {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| denied())?;
    if bytes.len() > 16384 {
        return Err(denied());
    }
    Ok(bytes)
}
#[cfg(unix)]
pub fn atomic_write(path: &Path, bytes: &[u8]) -> ComputeResult<()> {
    atomic_write_owned(path, bytes, 0)
}
#[cfg(unix)]
pub fn atomic_write_owned(path: &Path, bytes: &[u8], owner: u32) -> ComputeResult<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let parent_meta =
        fs::symlink_metadata(path.parent().ok_or_else(denied)?).map_err(|_| denied())?;
    if !parent_meta.is_dir() || parent_meta.uid() != owner || parent_meta.mode() & 0o022 != 0 {
        return Err(denied());
    }
    if bytes.len() > 16384 {
        return Err(denied());
    }
    let parent = path.parent().ok_or_else(denied)?;
    let name = path
        .file_name()
        .ok_or_else(denied)?
        .to_str()
        .ok_or_else(denied)?;
    let temp = parent.join(format!(".{name}.pending"));
    // A crash can leave a temp file; never adopt it as authority. Remove only our fixed,
    // root-owned protected temp after validating the parent in the daemon.
    if temp.exists() {
        protected_read(&temp, owner)?;
        fs::remove_file(&temp).map_err(|_| denied())?;
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o640)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(&temp)
        .map_err(|_| denied())?;
    f.write_all(bytes).map_err(|_| denied())?;
    f.sync_all().map_err(|_| denied())?;
    fs::rename(&temp, path).map_err(|_| denied())?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| denied())
}
#[cfg(target_os = "linux")]
pub fn clock_sample() -> ComputeResult<ClockSample> {
    let mut t = nix::libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { nix::libc::clock_gettime(nix::libc::CLOCK_BOOTTIME, &mut t) } != 0
        || t.tv_sec < 0
        || t.tv_nsec < 0
    {
        return Err(denied());
    }
    let ticks_us = u64::try_from(t.tv_sec)
        .map_err(|_| denied())?
        .checked_mul(1000000)
        .and_then(|v| v.checked_add(t.tv_nsec as u64 / 1000))
        .ok_or_else(denied)?;
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| denied())?
        .trim()
        .to_owned();
    crate::distribution_contracts::primitives::uuid(&boot_id, None).map_err(|_| denied())?;
    let utc_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| denied())?
        .as_micros() as i128
        + 62_135_596_800_000_000i128;
    let trusted = fs::symlink_metadata("/run/systemd/timesync/synchronized").is_ok_and(|m| {
        use std::os::unix::fs::MetadataExt;
        m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0
    });
    Ok(ClockSample {
        boot_id,
        ticks_us,
        utc_us,
        trusted,
    })
}
