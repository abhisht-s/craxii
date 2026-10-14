use super::{AuthError, AuthResult};
use crate::distribution_contracts::{
    canonical::sha256,
    signing::{SignaturePurpose, signature_input},
};
use aws_lc_rs::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};
/// The signer and directory descriptors belong only to the trusted service UID.
/// The model UID must differ; chmod provides no same-UID containment.
pub struct KeyStore {
    directory: File,
    _lock: File,
    key: Ed25519KeyPair,
    service_uid: u32,
}
impl std::fmt::Debug for KeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyStore([REDACTED])")
    }
}
fn name(s: &str) -> AuthResult<CString> {
    CString::new(s).map_err(|_| AuthError::UnsafeStorage)
}
fn open_at(parent: &File, child: &str, flags: i32, mode: u32) -> AuthResult<File> {
    let n = name(child)?;
    // SAFETY: live directory descriptor, NUL-terminated fixed/path component and checked return value.
    let fd = unsafe {
        nix::libc::openat(
            parent.as_raw_fd(),
            n.as_ptr(),
            flags | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(AuthError::UnsafeStorage);
    }
    // SAFETY: openat produced a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn regular(file: &File, uid: u32, max: u64) -> AuthResult<()> {
    let m = file.metadata().map_err(|_| AuthError::UnsafeStorage)?;
    if !m.is_file()
        || m.uid() != uid
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
        || m.len() > max
    {
        return Err(AuthError::UnsafeStorage);
    }
    Ok(())
}
fn directory(path: &Path, uid: u32) -> AuthResult<File> {
    if !path.is_absolute() {
        return Err(AuthError::UnsafeStorage);
    }
    let mut parent = File::open("/").map_err(|_| AuthError::UnsafeStorage)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(c) => {
                let child = c.to_str().ok_or(AuthError::UnsafeStorage)?;
                parent = open_at(
                    &parent,
                    child,
                    nix::libc::O_RDONLY | nix::libc::O_DIRECTORY,
                    0,
                )?;
                let m = parent.metadata().map_err(|_| AuthError::UnsafeStorage)?;
                if (m.uid() != 0 && m.uid() != uid) || m.mode() & 0o022 != 0 {
                    return Err(AuthError::UnsafeStorage);
                }
            }
            _ => return Err(AuthError::UnsafeStorage),
        }
    }
    let m = parent.metadata().map_err(|_| AuthError::UnsafeStorage)?;
    if m.uid() != uid || m.mode() & 0o777 != 0o700 {
        return Err(AuthError::UnsafeStorage);
    }
    Ok(parent)
}
impl KeyStore {
    /// Production location is fixed on OS-root storage, outside customer snapshot/workspaces.
    pub fn open(service_uid: u32, model_uid: u32, first_enrollment: bool) -> AuthResult<Self> {
        Self::open_path(
            Path::new("/var/lib/craxii-worker/identity"),
            service_uid,
            model_uid,
            first_enrollment,
        )
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub fn isolated_fixture(
        path: &Path,
        service_uid: u32,
        model_uid: u32,
        first_enrollment: bool,
    ) -> AuthResult<Self> {
        Self::open_path(path, service_uid, model_uid, first_enrollment)
    }
    fn open_path(
        path: &Path,
        uid: u32,
        model_uid: u32,
        first_enrollment: bool,
    ) -> AuthResult<Self> {
        // SAFETY: geteuid has no preconditions or side effects.
        if uid == model_uid || unsafe { nix::libc::geteuid() } != uid {
            return Err(AuthError::UnsafeStorage);
        }
        let dir = directory(path, uid)?;
        let lock = open_at(
            &dir,
            "identity.lock",
            nix::libc::O_RDWR | nix::libc::O_CREAT,
            0o600,
        )?;
        regular(&lock, uid, 0)?;
        // SAFETY: owned regular file descriptor. Nonblocking lock prevents duplicate writers.
        if unsafe { nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) }
            != 0
        {
            return Err(AuthError::UnsafeStorage);
        }
        let mut file = match open_at(&dir, "worker.pk8", nix::libc::O_RDONLY, 0) {
            Ok(f) => f,
            Err(_) => {
                // Existing sentinel or state, even after incomplete publication, means explicit recovery.
                if !first_enrollment
                    || open_at(&dir, "identity.created", nix::libc::O_RDONLY, 0).is_ok()
                    || open_at(&dir, "identity.json", nix::libc::O_RDONLY, 0).is_ok()
                {
                    return Err(AuthError::KeyUnavailable);
                }
                let marker = open_at(
                    &dir,
                    "identity.created",
                    nix::libc::O_WRONLY | nix::libc::O_CREAT | nix::libc::O_EXCL,
                    0o600,
                )?;
                regular(&marker, uid, 0)?;
                marker.sync_all().map_err(|_| AuthError::UnsafeStorage)?;
                dir.sync_all().map_err(|_| AuthError::UnsafeStorage)?;
                let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                    .map_err(|_| AuthError::KeyUnavailable)?;
                // Exclusive no-replace publication. A partial file after crash is deliberately unrecoverable,
                // never replaced; the marker prevents generating a new identity after key loss.
                let mut generated = open_at(
                    &dir,
                    "worker.pending",
                    nix::libc::O_WRONLY | nix::libc::O_CREAT | nix::libc::O_EXCL,
                    0o600,
                )?;
                generated
                    .write_all(pkcs8.as_ref())
                    .map_err(|_| AuthError::KeyUnavailable)?;
                generated
                    .sync_all()
                    .map_err(|_| AuthError::KeyUnavailable)?;
                let pending = name("worker.pending")?;
                let final_name = name("worker.pk8")?;
                // SAFETY: fixed names in the held protected directory. linkat never overwrites a key.
                if unsafe {
                    nix::libc::linkat(
                        dir.as_raw_fd(),
                        pending.as_ptr(),
                        dir.as_raw_fd(),
                        final_name.as_ptr(),
                        0,
                    )
                } != 0
                {
                    return Err(AuthError::KeyUnavailable);
                }
                // SAFETY: removes only the new temporary link, preserving the published key.
                if unsafe { nix::libc::unlinkat(dir.as_raw_fd(), pending.as_ptr(), 0) } != 0 {
                    return Err(AuthError::KeyUnavailable);
                }
                dir.sync_all().map_err(|_| AuthError::KeyUnavailable)?;
                open_at(&dir, "worker.pk8", nix::libc::O_RDONLY, 0)?
            }
        };
        regular(&file, uid, 512)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| AuthError::KeyUnavailable)?;
        let key = Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| AuthError::KeyUnavailable)?;
        Ok(Self {
            directory: dir,
            _lock: lock,
            key,
            service_uid: uid,
        })
    }
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref())
    }
    pub fn key_id(&self) -> String {
        sha256(self.key.public_key().as_ref())
    }
    // No arbitrary public signer or model IPC. The client validates server challenge context first.
    pub(super) fn sign(&self, purpose: SignaturePurpose, signed: &Value) -> AuthResult<String> {
        let bytes = signature_input(purpose, signed).map_err(|_| AuthError::InvalidContext)?;
        Ok(URL_SAFE_NO_PAD.encode(self.key.sign(&bytes).as_ref()))
    }
    pub(super) fn read_state(&self) -> AuthResult<Option<Vec<u8>>> {
        let state_name = name("identity.json")?;
        let mut stat = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
        // SAFETY: valid descriptor/name and writable stat storage; no symlink traversal.
        if unsafe {
            nix::libc::fstatat(
                self.directory.as_raw_fd(),
                state_name.as_ptr(),
                stat.as_mut_ptr(),
                nix::libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            if std::io::Error::last_os_error().raw_os_error() == Some(nix::libc::ENOENT) {
                return Ok(None);
            }
            return Err(AuthError::UnsafeStorage);
        }
        let mut f = open_at(&self.directory, "identity.json", nix::libc::O_RDONLY, 0)?;
        regular(&f, self.service_uid, 16384)?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)
            .map_err(|_| AuthError::UnsafeStorage)?;
        Ok(Some(bytes))
    }
    pub(super) fn save_state(&self, bytes: &[u8]) -> AuthResult<()> {
        if bytes.len() > 16384 {
            return Err(AuthError::InvalidContext);
        }
        let mut f = open_at(
            &self.directory,
            "identity.pending",
            nix::libc::O_WRONLY | nix::libc::O_CREAT | nix::libc::O_EXCL,
            0o600,
        )?;
        regular(&f, self.service_uid, 0)?;
        f.write_all(bytes).map_err(|_| AuthError::UnsafeStorage)?;
        f.sync_all().map_err(|_| AuthError::UnsafeStorage)?;
        let from = name("identity.pending")?;
        let to = name("identity.json")?;
        // SAFETY: fixed names in held protected directory; rename replaces only recovery state, never key.
        if unsafe {
            nix::libc::renameat(
                self.directory.as_raw_fd(),
                from.as_ptr(),
                self.directory.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(AuthError::UnsafeStorage);
        }
        self.directory
            .sync_all()
            .map_err(|_| AuthError::UnsafeStorage)
    }
}
