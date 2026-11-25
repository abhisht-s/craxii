//! Installed shell scratch. Fixed tmpfs paths, one execution UUID, no arbitrary cleanup path.
use nix::libc;
use std::ffi::CString;
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

const BASES: [&str; 3] = ["/tmp", "/var/tmp", "/dev/shm"];
const READ: u64 = (1 << 0) | (1 << 2) | (1 << 3);
const ALL: u64 = (1 << 15) - 1;
#[repr(C)]
struct Ruleset {
    handled: u64,
}
#[repr(C, packed)]
struct PathRule {
    access: u64,
    parent: i32,
}

pub fn id(value: &std::ffi::OsStr) -> Result<String, ()> {
    let text = value.to_str().ok_or(())?;
    let parsed = uuid::Uuid::try_parse(text).map_err(|_| ())?;
    if parsed.to_string() != text {
        return Err(());
    }
    Ok(text.to_owned())
}
fn paths(id: &str) -> Vec<PathBuf> {
    BASES
        .iter()
        .map(|p| PathBuf::from(p).join("craxii-tools").join(id))
        .collect()
}
fn parent(path: &std::path::Path) -> Result<(), ()> {
    let m = fs::symlink_metadata(path).map_err(|_| ())?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o7777 != 0o1773 {
        return Err(());
    }
    Ok(())
}
pub fn create(id: &str) -> Result<(), ()> {
    let current = fs::read_to_string("/proc/self/cgroup").map_err(|_| ())?;
    if current.trim() != format!("0::/system.slice/craxii-distribution.service/tools/{id}") {
        return Err(());
    }
    for p in paths(id) {
        parent(p.parent().ok_or(())?)?;
        fs::create_dir(&p).map_err(|_| ())?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
    }
    Ok(())
}
pub fn verify_empty(id: &str) -> Result<(), ()> {
    let cgroup = PathBuf::from(craxii_server::guest_boundary::storage::TOOLS_CGROUP).join(id);
    let events = fs::read_to_string(cgroup.join("cgroup.events")).map_err(|_| ())?;
    if !events.lines().any(|line| line == "populated 0") {
        return Err(());
    }
    Ok(())
}
pub fn cleanup(id: &str, uid: u32) -> Result<(), ()> {
    for p in paths(id) {
        parent(p.parent().ok_or(())?)?;
        match fs::symlink_metadata(&p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(()),
            Ok(m) => {
                if !m.is_dir()
                    || m.uid() != uid
                    || m.mode() & 0o7777 != 0o700
                    || m.dev() != fs::metadata(p.parent().ok_or(())?).map_err(|_| ())?.dev()
                {
                    return Err(());
                }
            }
        }
        // Rust's Unix remove_dir_all walks descriptor-relative and does not follow symlinks.
        fs::remove_dir_all(p).map_err(|_| ())?;
    }
    Ok(())
}
fn allow(ruleset: &File, path: &std::path::Path, access: u64) -> Result<(), ()> {
    let name = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| ())?;
    let raw = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(());
    }
    let fd = unsafe { File::from_raw_fd(raw) };
    let rule = PathRule {
        access,
        parent: fd.as_raw_fd(),
    };
    if unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            1,
            &rule,
            0,
        )
    } != 0
    {
        return Err(());
    }
    Ok(())
}
pub fn restrict(id: &str) -> Result<String, ()> {
    // ABI 3 includes cross-directory references and truncation. Missing enforcement holds.
    if unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<Ruleset>(),
            0,
            1,
        )
    } < 3
    {
        return Err(());
    }
    let attr = Ruleset { handled: ALL };
    let raw = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr,
            size_of::<Ruleset>(),
            0,
        )
    };
    if raw < 0 {
        return Err(());
    }
    let ruleset = unsafe { File::from_raw_fd(raw as i32) };
    for p in [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc",
        "/proc",
        "/sys",
        "/opt",
        "/run",
        "/var/lib",
        "/var/log",
        "/var/cache",
        "/var/spool",
    ] {
        let path = std::path::Path::new(p);
        if path.exists() {
            allow(&ruleset, path, READ)?;
        }
    }
    for p in [
        "/dev/null",
        "/dev/zero",
        "/dev/random",
        "/dev/urandom",
        "/dev/tty",
    ] {
        let path = std::path::Path::new(p);
        if path.exists() {
            allow(&ruleset, path, (1 << 1) | (1 << 2))?;
        }
    }
    for p in [
        "/home/craxii",
        craxii_server::guest_boundary::storage::WORKSPACE,
    ] {
        allow(&ruleset, std::path::Path::new(p), ALL)?;
    }
    let scratch = paths(id);
    for p in &scratch {
        allow(&ruleset, p, ALL)?;
    }
    if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0) } != 0 {
        return Err(());
    }
    scratch[0].to_str().map(str::to_owned).ok_or(())
}
