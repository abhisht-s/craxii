//! Read-only installed checks. No canonical directory, database or key is created here.
use super::{
    BoundaryError::{
        DiskIdentityMismatch, IsolationUnavailable, LayoutUnavailable, ResourcePressure,
    },
    BoundaryResult,
    config::GuestConfig,
};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
};
pub const CONFIG: &str = "/etc/craxii/guest-bootstrap.json";
pub const DATA: &str = "/srv/craxii-data";
pub const STATE: &str = "/var/lib/craxii";
pub const ARTIFACTS: &str = "/var/lib/craxii/artifacts";
pub const WORKSPACE: &str = "/srv/craxii/workspaces/primary";
pub const IDENTITY: &str = "/var/lib/craxii-worker/identity";
pub const TOOLS_CGROUP: &str = "/sys/fs/cgroup/system.slice/craxii-distribution.service/tools";
const SNAPSHOT_BYTES: usize = 1_048_576;
#[derive(Clone, Debug)]
pub struct MountObservation {
    pub device: String,
    pub root: String,
    pub target: String,
    pub filesystem: String,
    pub flags: Vec<String>,
}
pub fn parse_mounts(bytes: &[u8]) -> BoundaryResult<Vec<MountObservation>> {
    if bytes.len() > SNAPSHOT_BYTES {
        return Err(LayoutUnavailable);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| LayoutUnavailable)?;
    let mut mounts = Vec::new();
    for line in text.lines() {
        if mounts.len() >= 4096 {
            return Err(LayoutUnavailable);
        }
        let (head, tail) = line.split_once(" - ").ok_or(LayoutUnavailable)?;
        let h: Vec<_> = head.split_whitespace().collect();
        let t: Vec<_> = tail.split_whitespace().collect();
        if h.len() < 6 || t.len() != 3 {
            return Err(LayoutUnavailable);
        }
        // Fixed paths contain no escapes. Reject encoded names rather than guessing their meaning.
        if h[3].contains('\\') || h[4].contains('\\') {
            return Err(LayoutUnavailable);
        }
        mounts.push(MountObservation {
            device: h[2].to_owned(),
            root: h[3].to_owned(),
            target: h[4].to_owned(),
            filesystem: t[0].to_owned(),
            flags: h[5].split(',').map(str::to_owned).collect(),
        });
    }
    Ok(mounts)
}
pub fn verify_mounts(mounts: &[MountObservation], device: &str) -> BoundaryResult<()> {
    let root = mounts
        .iter()
        .filter(|m| m.target == "/")
        .collect::<Vec<_>>();
    if root.len() != 1 || root[0].device == device {
        return Err(DiskIdentityMismatch);
    }
    for (target, fsroot) in [
        (DATA, "/"),
        (STATE, "/state"),
        ("/srv/craxii/workspaces", "/workspaces"),
        ("/home/craxii", "/home/craxii"),
    ] {
        let matches = mounts
            .iter()
            .filter(|m| m.target == target)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(LayoutUnavailable);
        }
        let m = matches[0];
        if m.device != device
            || m.root != fsroot
            || m.filesystem != "ext4"
            || !["rw", "nosuid", "nodev"]
                .iter()
                .all(|f| m.flags.iter().any(|flag| flag == f))
        {
            return Err(LayoutUnavailable);
        }
    }
    // A nested overmount would redirect a required trusted path away from the verified volume.
    for m in mounts {
        if [STATE, "/srv/craxii/workspaces", "/home/craxii", IDENTITY]
            .iter()
            .any(|p| m.target.starts_with(&format!("{p}/")))
        {
            return Err(LayoutUnavailable);
        }
    }
    Ok(())
}
/// Walks from / with O_NOFOLLOW and pins each ancestor descriptor, including the final object.
/// Allowed trusted owners may search, but no ancestor is writable by group/other.
pub fn protected_open(path: &Path, owner: u32, mode: u32, directory: bool) -> BoundaryResult<File> {
    if !path.is_absolute() {
        return Err(LayoutUnavailable);
    }
    let mut fd = File::open("/").map_err(|_| LayoutUnavailable)?;
    let components: Vec<_> = path
        .components()
        .filter(|c| !matches!(c, Component::RootDir))
        .collect();
    if components.is_empty() {
        return Err(LayoutUnavailable);
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(LayoutUnavailable);
        };
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| LayoutUnavailable)?;
        let final_component = index + 1 == components.len();
        let flags = nix::libc::O_RDONLY
            | nix::libc::O_CLOEXEC
            | nix::libc::O_NOFOLLOW
            | nix::libc::O_NONBLOCK
            | if !final_component || directory {
                nix::libc::O_DIRECTORY
            } else {
                0
            };
        let next = unsafe { nix::libc::openat(fd.as_raw_fd(), name.as_ptr(), flags) };
        if next < 0 {
            return Err(LayoutUnavailable);
        }
        fd = unsafe { File::from_raw_fd(next) };
        let m = fd.metadata().map_err(|_| LayoutUnavailable)?;
        if final_component {
            if m.uid() != owner
                || m.mode() & 0o7777 != mode
                || (!directory && (!m.is_file() || m.nlink() != 1))
                || (directory && !m.is_dir())
            {
                return Err(LayoutUnavailable);
            }
        } else if !m.is_dir() || ![0, owner].contains(&m.uid()) || m.mode() & 0o022 != 0 {
            return Err(LayoutUnavailable);
        }
    }
    Ok(fd)
}
pub fn read_protected(path: &str, owner: u32, mode: u32, max: usize) -> BoundaryResult<Vec<u8>> {
    let file = protected_open(Path::new(path), owner, mode, false)?;
    let mut bytes = Vec::new();
    file.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LayoutUnavailable)?;
    if bytes.len() > max {
        return Err(LayoutUnavailable);
    }
    Ok(bytes)
}
#[cfg(target_os = "linux")]
fn bounded(path: &str, max: usize) -> BoundaryResult<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| LayoutUnavailable)?
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LayoutUnavailable)?;
    if bytes.len() > max {
        return Err(LayoutUnavailable);
    }
    String::from_utf8(bytes).map_err(|_| LayoutUnavailable)
}
#[cfg(target_os = "linux")]
fn user(name: &std::ffi::CStr) -> BoundaryResult<(u32, u32)> {
    let pw = unsafe { nix::libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return Err(IsolationUnavailable);
    }
    Ok(unsafe { ((*pw).pw_uid, (*pw).pw_gid) })
}
pub fn load_installed() -> BoundaryResult<GuestConfig> {
    let c = GuestConfig::decode(&read_protected(CONFIG, 0, 0o640, 16384)?)?;
    if c.synthetic() {
        return Err(IsolationUnavailable);
    }
    Ok(c)
}
#[cfg(not(target_os = "linux"))]
pub fn verify_installed(_: &GuestConfig) -> BoundaryResult<()> {
    Err(IsolationUnavailable)
}
#[cfg(target_os = "linux")]
pub fn verify_installed(c: &GuestConfig) -> BoundaryResult<()> {
    if c.synthetic() || std::env::consts::ARCH != "x86_64" {
        return Err(IsolationUnavailable);
    }
    let os = bounded("/etc/os-release", 16384)?;
    if !os.lines().any(|s| s == "ID=ubuntu") || !os.lines().any(|s| s == "VERSION_ID=\"24.04\"") {
        return Err(IsolationUnavailable);
    }
    let (worker, worker_group) = user(c"craxii-server")?;
    let (model, model_group) = user(c"craxii")?;
    if worker == 0
        || model == 0
        || worker_group == 0
        || model_group == 0
        || worker == model
        || worker_group == model_group
    {
        return Err(IsolationUnavailable);
    }
    let uid = unsafe { nix::libc::geteuid() };
    if uid != 0 && uid != worker {
        return Err(IsolationUnavailable);
    }
    let mounts = parse_mounts(bounded("/proc/self/mountinfo", SNAPSHOT_BYTES)?.as_bytes())?;
    let device_path =
        std::fs::canonicalize(format!("/dev/disk/by-uuid/{}", c.text("filesystem_uuid")))
            .map_err(|_| DiskIdentityMismatch)?;
    use std::os::unix::fs::FileTypeExt;
    let m = std::fs::metadata(&device_path).map_err(|_| DiskIdentityMismatch)?;
    if !m.file_type().is_block_device() {
        return Err(DiskIdentityMismatch);
    }
    let major = nix::libc::major(m.rdev());
    let minor = nix::libc::minor(m.rdev());
    let device = format!("{major}:{minor}");
    let mut matching = 0;
    let wanted = c.text("volume_id").replace('-', "");
    for entry in std::fs::read_dir("/sys/class/block").map_err(|_| DiskIdentityMismatch)? {
        let entry = entry.map_err(|_| DiskIdentityMismatch)?;
        if entry.path().join("partition").exists() {
            continue;
        }
        let serial_path = entry.path().join("device/serial");
        if !serial_path.exists() {
            continue;
        }
        let serial = bounded(serial_path.to_str().ok_or(DiskIdentityMismatch)?, 128)?;
        if serial.trim().replace('-', "") == wanted {
            matching += 1;
            if bounded(
                entry
                    .path()
                    .join("dev")
                    .to_str()
                    .ok_or(DiskIdentityMismatch)?,
                128,
            )?
            .trim()
                != device
                || bounded(
                    entry
                        .path()
                        .join("size")
                        .to_str()
                        .ok_or(DiskIdentityMismatch)?,
                    128,
                )?
                .trim()
                    != "167772160"
                || bounded(
                    entry
                        .path()
                        .join("device/model")
                        .to_str()
                        .ok_or(DiskIdentityMismatch)?,
                    128,
                )?
                .trim()
                    != "Amazon Elastic Block Store"
            {
                return Err(DiskIdentityMismatch);
            }
            if std::fs::read_dir(entry.path().join("holders"))
                .map_err(|_| DiskIdentityMismatch)?
                .next()
                .is_some()
            {
                return Err(DiskIdentityMismatch);
            }
        }
    }
    if matching != 1 {
        return Err(DiskIdentityMismatch);
    }
    verify_mounts(&mounts, &device)?;
    let data = protected_open(Path::new(DATA), 0, 0o750, true)?;
    let state = protected_open(Path::new(STATE), worker, 0o700, true)?;
    let workspace = protected_open(Path::new(WORKSPACE), model, 0o750, true)?;
    if unsafe {
        nix::libc::syscall(
            nix::libc::SYS_faccessat2,
            workspace.as_raw_fd(),
            c"".as_ptr(),
            nix::libc::X_OK,
            nix::libc::AT_EMPTY_PATH | nix::libc::AT_EACCESS,
        )
    } != 0
    {
        return Err(LayoutUnavailable);
    }
    let key = protected_open(Path::new(IDENTITY), worker, 0o700, true)?;
    if data.metadata().map_err(|_| LayoutUnavailable)?.dev()
        != state.metadata().map_err(|_| LayoutUnavailable)?.dev()
        || key.metadata().map_err(|_| LayoutUnavailable)?.dev()
            != std::fs::metadata("/").map_err(|_| LayoutUnavailable)?.dev()
    {
        return Err(LayoutUnavailable);
    }
    for p in ["db", "artifacts", "locks"] {
        let path = format!("{STATE}/{p}");
        let f = protected_open(Path::new(&path), worker, 0o700, true)?;
        if f.metadata().map_err(|_| LayoutUnavailable)?.dev()
            != state.metadata().map_err(|_| LayoutUnavailable)?.dev()
        {
            return Err(LayoutUnavailable);
        }
    }
    for filename in [
        "worker.pk8",
        "identity.created",
        "identity.json",
        "distribution.json",
    ] {
        read_protected(&format!("{IDENTITY}/{filename}"), worker, 0o600, 16384)?;
    }
    let identity: serde_json::Value = serde_json::from_slice(&read_protected(
        &format!("{IDENTITY}/identity.json"),
        worker,
        0o600,
        16384,
    )?)
    .map_err(|_| LayoutUnavailable)?;
    for field in [
        "customer_id",
        "logical_agent_id",
        "installation_id",
        "generation",
    ] {
        if identity[field] != c.value()[field] {
            return Err(LayoutUnavailable);
        }
    }
    let marker = crate::distribution_contracts::raw_json::parse_bounded(
        &read_protected(&format!("{DATA}/.craxii-data-lineage.json"), 0, 0o440, 4096)?,
        crate::distribution_contracts::raw_json::Limits {
            bytes: 4096,
            nodes: 256,
            depth: 8,
        },
    )
    .map_err(|_| LayoutUnavailable)?;
    if marker != c.value()["marker"] {
        return Err(LayoutUnavailable);
    }
    // Root pre-start verification records boot-bound evidence for root-only quota/firewall checks.
    let ready: serde_json::Value = serde_json::from_slice(&read_protected(
        "/run/craxii-worker/security-ready.json",
        0,
        0o440,
        4096,
    )?)
    .map_err(|_| IsolationUnavailable)?;
    if ready
        != serde_json::json!({"version":1,"config_sha256":c.digest()?,"boot_id":bounded("/proc/sys/kernel/random/boot_id",128)?.trim(),"storage_phase":"layout_verified"})
    {
        return Err(IsolationUnavailable);
    }
    if bounded("/proc/sys/kernel/yama/ptrace_scope", 128)?.trim() != "2"
        || bounded("/proc/sys/fs/suid_dumpable", 128)?.trim() != "0"
    {
        return Err(IsolationUnavailable);
    }
    for (name, expected) in [
        ("pids.max", "64"),
        ("memory.max", "268435456"),
        ("memory.swap.max", "0"),
        ("cpu.max", "100000 100000"),
    ] {
        if bounded(&format!("{TOOLS_CGROUP}/{name}"), 128)?.trim() != expected {
            return Err(IsolationUnavailable);
        }
    }
    Ok(())
}
pub fn pressure() -> BoundaryResult<()> {
    #[cfg(target_os = "linux")]
    {
        let path = std::ffi::CString::new(STATE).map_err(|_| ResourcePressure)?;
        let mut stat = std::mem::MaybeUninit::<nix::libc::statvfs>::uninit();
        if unsafe { nix::libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(ResourcePressure);
        }
        let stat = unsafe { stat.assume_init() };
        if u128::from(stat.f_bavail) * u128::from(stat.f_frsize) < 4294967296
            || stat.f_favail < 65536
        {
            return Err(ResourcePressure);
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(ResourcePressure)
    }
}
/// All model-UID processes count, including those outside the ordinary execution cgroup.
/// Missing visibility never becomes an empty observation.
pub fn model_process_count() -> BoundaryResult<u64> {
    #[cfg(target_os = "linux")]
    {
        let (model, _) = user(c"craxii")?;
        let mut count = 0;
        let mut rows = 0;
        for entry in std::fs::read_dir("/proc").map_err(|_| super::BoundaryError::CleanupUnknown)? {
            let entry = entry.map_err(|_| super::BoundaryError::CleanupUnknown)?;
            if !entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
            {
                continue;
            }
            rows += 1;
            if rows > 4096 {
                return Err(super::BoundaryError::CleanupUnknown);
            }
            let path = entry.path().join("status");
            let status = match bounded(
                path.to_str().ok_or(super::BoundaryError::CleanupUnknown)?,
                16384,
            ) {
                Ok(s) => s,
                Err(_) => {
                    if !entry.path().exists() {
                        continue;
                    }
                    return Err(super::BoundaryError::CleanupUnknown);
                }
            };
            let uid = status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or(super::BoundaryError::CleanupUnknown)?;
            if uid == model {
                count += 1;
            }
        }
        Ok(count)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(super::BoundaryError::CleanupUnknown)
    }
}
/// Before the first canonical SQLite open, record a one-shot creation intent on the verified
/// mount. A crash after this record but before SQLite creation holds instead of silently creating
/// a new journal on restart. This is a storage sentinel, not a competing execution journal.
pub fn prepare_journal_open(c: &GuestConfig) -> BoundaryResult<()> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let (worker, _) = user(c"craxii-server")?;
        let database = Path::new(STATE).join("db/craxii.sqlite3");
        if database.try_exists().map_err(|_| LayoutUnavailable)? {
            protected_open(&database, worker, 0o600, false)?;
            return Ok(());
        }
        pressure()?;
        if c.text("mode") != "initialize_blank" {
            return Err(LayoutUnavailable);
        }
        let receipt: serde_json::Value = serde_json::from_slice(&read_protected(
            "/run/craxii-worker/security-ready.json",
            0,
            0o440,
            4096,
        )?)
        .map_err(|_| LayoutUnavailable)?;
        if receipt
            != serde_json::json!({"version":1,"config_sha256":c.digest()?,"boot_id":bounded("/proc/sys/kernel/random/boot_id",128)?.trim(),"storage_phase":"layout_verified"})
        {
            return Err(LayoutUnavailable);
        }
        let parent = protected_open(Path::new(STATE), worker, 0o700, true)?;
        let fd = unsafe {
            nix::libc::openat(
                parent.as_raw_fd(),
                c".journal-creation-intent".as_ptr(),
                nix::libc::O_WRONLY
                    | nix::libc::O_CLOEXEC
                    | nix::libc::O_NOFOLLOW
                    | nix::libc::O_CREAT
                    | nix::libc::O_EXCL,
                0o600,
            )
        };
        if fd < 0 {
            return Err(LayoutUnavailable);
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(c.text("marker_sha256").as_bytes())
            .map_err(|_| LayoutUnavailable)?;
        file.sync_all().map_err(|_| LayoutUnavailable)?;
        parent.sync_all().map_err(|_| LayoutUnavailable)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = c;
        Err(IsolationUnavailable)
    }
}
pub fn tool_descendant_count() -> BoundaryResult<u64> {
    #[cfg(target_os = "linux")]
    {
        let count = model_process_count()?;
        let events = bounded(&format!("{TOOLS_CGROUP}/cgroup.events"), 1024)?;
        match events
            .lines()
            .find_map(|line| line.strip_prefix("populated "))
        {
            Some("0") if count == 0 => Ok(0),
            Some("0" | "1") => Ok(count.max(1)),
            _ => Err(super::BoundaryError::CleanupUnknown),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(super::BoundaryError::CleanupUnknown)
    }
}
/// Empty execution cgroups and leftover ephemeral directories still require cleanup.
pub fn pending_execution_cleanup() -> BoundaryResult<u64> {
    let mut count = 0u64;
    for root in [
        TOOLS_CGROUP,
        "/tmp/craxii-tools",
        "/var/tmp/craxii-tools",
        "/dev/shm/craxii-tools",
    ] {
        for entry in std::fs::read_dir(root).map_err(|_| super::BoundaryError::CleanupUnknown)? {
            let entry = entry.map_err(|_| super::BoundaryError::CleanupUnknown)?;
            if count >= 4096 {
                return Err(super::BoundaryError::CleanupUnknown);
            }
            if entry
                .file_type()
                .map_err(|_| super::BoundaryError::CleanupUnknown)?
                .is_dir()
            {
                count += 1;
            } else if root != TOOLS_CGROUP {
                return Err(super::BoundaryError::CleanupUnknown);
            }
        }
    }
    Ok(count)
}
/// Root inspection and trusted service execution are distinct authorities.
pub fn require_worker_principal() -> BoundaryResult<()> {
    #[cfg(target_os = "linux")]
    {
        let (worker, group) = user(c"craxii-server")?;
        if unsafe { nix::libc::getuid() } != worker
            || unsafe { nix::libc::geteuid() } != worker
            || unsafe { nix::libc::getgid() } != group
            || worker == 0
        {
            return Err(IsolationUnavailable);
        }
        let status = bounded("/proc/self/status", 16384)?;
        let value = |key: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .map(str::trim)
        };
        for key in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:"] {
            if value(key).and_then(|v| u64::from_str_radix(v, 16).ok()) != Some(1 << 5) {
                return Err(IsolationUnavailable);
            }
        }
        if value("CapBnd:").and_then(|v| u64::from_str_radix(v, 16).ok())
            != Some((1 << 5) | (1 << 6) | (1 << 7) | (1 << 8))
            || value("NoNewPrivs:") != Some("0")
        {
            return Err(IsolationUnavailable);
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(IsolationUnavailable)
    }
}
/// Make trusted memory non-dumpable before reading any protected key/config bytes.
pub fn harden_worker_process() -> BoundaryResult<()> {
    require_worker_principal()?;
    #[cfg(target_os = "linux")]
    if unsafe { nix::libc::prctl(nix::libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(IsolationUnavailable);
    }
    Ok(())
}
