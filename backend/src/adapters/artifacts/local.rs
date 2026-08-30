use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use nix::sys::statfs;
use sha2::{Digest, Sha256};

use crate::domain::{
    ArtifactId, ArtifactStorageKey, CanonicalByteCount, Sha256Digest, UtcTimestamp,
};
use crate::ports::artifact_store::{
    ArtifactCapture, ArtifactObjectReference, ArtifactOrphan, ArtifactOrphanReport, ArtifactStore,
    ArtifactStoreError, ArtifactStoreErrorKind, BeginArtifactCapture, FinalizedArtifact,
};

const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const TEMP_DIRECTORY: &str = "tmp";
const DIGEST_DIRECTORY: &str = "sha256";

/// Hardened local content-addressed artifact store.
pub struct LocalArtifactStore {
    root: PathBuf,
    temp: PathBuf,
    sha256: PathBuf,
}

impl std::fmt::Debug for LocalArtifactStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalArtifactStore")
            .field("root", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl LocalArtifactStore {
    /// Initializes and verifies the configured root after SQLite migration/integrity succeeds.
    pub fn initialize(root: &Path) -> Result<Self, ArtifactStoreError> {
        if !root.is_absolute() || root.parent().is_none() {
            return Err(error(ArtifactStoreErrorKind::UnsafeRoot));
        }
        let root_created = ensure_private_directory(root)?;
        verify_supported_filesystem(root)?;
        let temp = root.join(TEMP_DIRECTORY);
        let sha256 = root.join(DIGEST_DIRECTORY);
        let temp_created = ensure_private_directory(&temp)?;
        let sha256_created = ensure_private_directory(&sha256)?;
        if root_created {
            sync_directory(
                root.parent()
                    .ok_or_else(|| error(ArtifactStoreErrorKind::UnsafeRoot))?,
            )?;
        }
        if temp_created || sha256_created {
            sync_directory(root)?;
        }
        let temp_device = fs::metadata(&temp)
            .map_err(|_| error(ArtifactStoreErrorKind::UnsafeRoot))?
            .dev();
        let digest_device = fs::metadata(&sha256)
            .map_err(|_| error(ArtifactStoreErrorKind::UnsafeRoot))?
            .dev();
        if temp_device != digest_device {
            return Err(error(ArtifactStoreErrorKind::UnsafeRoot));
        }
        Ok(Self {
            root: root.to_owned(),
            temp,
            sha256,
        })
    }

    fn final_path(&self, key: &ArtifactStorageKey) -> Result<PathBuf, ArtifactStoreError> {
        let canonical = ArtifactStorageKey::parse_canonical(key.as_str())
            .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?;
        let value = canonical.as_str();
        Ok(self.sha256.join(&value[7..9]).join(&value[10..]))
    }

    fn verified_bytes(
        &self,
        artifact: &ArtifactObjectReference,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        if artifact.storage_key() != &ArtifactStorageKey::from_digest(artifact.sha256()) {
            return Err(error(ArtifactStoreErrorKind::Integrity));
        }
        let path = self.final_path(artifact.storage_key())?;
        let mut file = open_existing_file(&path, ArtifactStoreErrorKind::Integrity)?;
        verify_open_file(&file, artifact.captured_byte_count().get())?;
        let capacity = usize::try_from(artifact.captured_byte_count().get())
            .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
        let mut bytes = Vec::with_capacity(capacity);
        file.read_to_end(&mut bytes)
            .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
        if bytes.len() != capacity || Sha256Digest::hash_bytes(&bytes) != artifact.sha256() {
            return Err(error(ArtifactStoreErrorKind::Integrity));
        }
        Ok(bytes)
    }
}

impl ArtifactStore for LocalArtifactStore {
    fn begin_capture(
        &self,
        request: BeginArtifactCapture,
    ) -> Result<Box<dyn ArtifactCapture>, ArtifactStoreError> {
        verify_secure_directory(&self.root)?;
        verify_secure_directory(&self.temp)?;
        verify_secure_directory(&self.sha256)?;
        let temp_path = self.temp.join(format!("{}.partial", request.artifact_id));
        let file = create_new_private_file(&temp_path)?;
        verify_open_file(&file, 0)?;
        Ok(Box::new(LocalArtifactCapture {
            artifact_id: request.artifact_id,
            hard_limit: request.hard_capture_limit.get(),
            observed: 0,
            captured: 0,
            hasher: Sha256::new(),
            file: Some(file),
            temp_path,
            temp_directory: self.temp.clone(),
            digest_directory: self.sha256.clone(),
        }))
    }

    fn verify(&self, artifact: &ArtifactObjectReference) -> Result<(), ArtifactStoreError> {
        self.verified_bytes(artifact).map(|_| ())
    }

    fn read_verified(
        &self,
        artifact: &ArtifactObjectReference,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        self.verified_bytes(artifact)
    }

    fn scan_orphans(
        &self,
        referenced: &BTreeSet<ArtifactStorageKey>,
        observed_at: UtcTimestamp,
    ) -> Result<ArtifactOrphanReport, ArtifactStoreError> {
        verify_secure_directory(&self.root)?;
        verify_secure_directory(&self.temp)?;
        verify_secure_directory(&self.sha256)?;
        let mut orphans = Vec::new();
        for entry in sorted_entries(&self.temp)? {
            let file_name = entry
                .file_name()
                .into_string()
                .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
            let id = file_name
                .strip_suffix(".partial")
                .ok_or_else(|| error(ArtifactStoreErrorKind::Integrity))?;
            let artifact_id = ArtifactId::parse_canonical(id)
                .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
            verify_file_metadata(&metadata, None)?;
            orphans.push(ArtifactOrphan::TempPartial {
                artifact_id,
                age_seconds: age_seconds(&metadata, observed_at)?,
            });
        }

        let mut referenced_final_count = 0_u64;
        for shard in sorted_entries(&self.sha256)? {
            let shard_name = shard
                .file_name()
                .into_string()
                .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
            if shard_name.len() != 2
                || shard_name
                    .bytes()
                    .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
            {
                return Err(error(ArtifactStoreErrorKind::Integrity));
            }
            verify_secure_directory(&shard.path())?;
            for entry in sorted_entries(&shard.path())? {
                let digest_text = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
                let digest = Sha256Digest::parse_canonical(&digest_text)
                    .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
                if digest_text[..2] != shard_name {
                    return Err(error(ArtifactStoreErrorKind::Integrity));
                }
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
                verify_file_metadata(&metadata, None)?;
                verify_path_digest(&entry.path(), digest, metadata.len())?;
                let storage_key = ArtifactStorageKey::from_digest(digest);
                if referenced.contains(&storage_key) {
                    referenced_final_count = referenced_final_count
                        .checked_add(1)
                        .ok_or_else(|| error(ArtifactStoreErrorKind::Integrity))?;
                } else {
                    orphans.push(ArtifactOrphan::FinalUnreferenced {
                        storage_key,
                        age_seconds: age_seconds(&metadata, observed_at)?,
                    });
                }
            }
        }
        Ok(ArtifactOrphanReport {
            referenced_final_count,
            orphans,
        })
    }
}

struct LocalArtifactCapture {
    artifact_id: ArtifactId,
    hard_limit: u64,
    observed: u64,
    captured: u64,
    hasher: Sha256,
    file: Option<File>,
    temp_path: PathBuf,
    temp_directory: PathBuf,
    digest_directory: PathBuf,
}

impl ArtifactCapture for LocalArtifactCapture {
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), ArtifactStoreError> {
        let chunk_length = u64::try_from(chunk.len())
            .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?;
        self.observed = self
            .observed
            .checked_add(chunk_length)
            .filter(|value| *value <= CanonicalByteCount::MAX)
            .ok_or_else(|| error(ArtifactStoreErrorKind::InvalidRequest))?;
        let remaining = self.hard_limit.saturating_sub(self.captured);
        let retained_length = usize::try_from(remaining.min(chunk_length))
            .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?;
        if retained_length != 0 {
            let retained = &chunk[..retained_length];
            self.file
                .as_mut()
                .ok_or_else(|| error(ArtifactStoreErrorKind::Storage))?
                .write_all(retained)
                .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
            self.hasher.update(retained);
            self.captured = self
                .captured
                .checked_add(retained_length as u64)
                .ok_or_else(|| error(ArtifactStoreErrorKind::InvalidRequest))?;
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<FinalizedArtifact, ArtifactStoreError> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| error(ArtifactStoreErrorKind::Storage))?;
        file.flush()
            .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
        file.sync_all()
            .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
        verify_open_file(&file, self.captured)?;
        drop(file);

        let digest = Sha256Digest::from_bytes(self.hasher.finalize().into());
        let storage_key = ArtifactStorageKey::from_digest(digest);
        let shard = self.digest_directory.join(&storage_key.as_str()[7..9]);
        let shard_created = ensure_private_directory(&shard)?;
        if shard_created {
            sync_directory(&self.digest_directory)?;
        }
        let final_path = shard.join(&storage_key.as_str()[10..]);
        match publish_no_replace(&self.temp_path, &final_path) {
            Ok(()) => sync_directory(&shard)?,
            Err(PublishError::AlreadyExists) => {
                verify_path_digest(&final_path, digest, self.captured)
                    .map_err(|_| error(ArtifactStoreErrorKind::Collision))?;
                let temp_metadata = fs::symlink_metadata(&self.temp_path)
                    .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
                verify_file_metadata(&temp_metadata, Some(self.captured))?;
                fs::remove_file(&self.temp_path)
                    .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
                sync_directory(&self.temp_directory)?;
            }
            Err(PublishError::Storage) => {
                return Err(error(ArtifactStoreErrorKind::Storage));
            }
        }
        #[cfg(feature = "test-failpoints")]
        crate::test_failpoints::reach(
            crate::test_failpoints::PhysicalHook::AfterArtifactRenameBeforeDbCommit,
        );
        verify_path_digest(&final_path, digest, self.captured)?;
        Ok(FinalizedArtifact::from_durable_publication(
            self.artifact_id,
            storage_key,
            digest,
            CanonicalByteCount::try_new(self.captured)
                .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?,
            CanonicalByteCount::try_new(self.observed)
                .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?,
            self.observed > self.captured,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublishError {
    AlreadyExists,
    Storage,
}

#[cfg(target_os = "macos")]
fn publish_no_replace(source: &Path, target: &Path) -> Result<(), PublishError> {
    let source = path_cstring(source)?;
    let target = path_cstring(target)?;
    // SAFETY: both pointers remain valid for the call and flags request atomic exclusive rename.
    let result =
        unsafe { nix::libc::renamex_np(source.as_ptr(), target.as_ptr(), nix::libc::RENAME_EXCL) };
    classify_publish_result(result)
}

#[cfg(target_os = "linux")]
fn publish_no_replace(source: &Path, target: &Path) -> Result<(), PublishError> {
    let source = path_cstring(source)?;
    let target = path_cstring(target)?;
    // SAFETY: both pointers remain valid for the syscall and AT_FDCWD resolves the explicit paths.
    let result = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_renameat2,
            nix::libc::AT_FDCWD,
            source.as_ptr(),
            nix::libc::AT_FDCWD,
            target.as_ptr(),
            nix::libc::RENAME_NOREPLACE,
        ) as i32
    };
    classify_publish_result(result)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn publish_no_replace(_source: &Path, _target: &Path) -> Result<(), PublishError> {
    Err(PublishError::Storage)
}

fn path_cstring(path: &Path) -> Result<CString, PublishError> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| PublishError::Storage)
}

fn classify_publish_result(result: i32) -> Result<(), PublishError> {
    if result == 0 {
        return Ok(());
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == nix::libc::EEXIST => Err(PublishError::AlreadyExists),
        _ => Err(PublishError::Storage),
    }
}

fn error(kind: ArtifactStoreErrorKind) -> ArtifactStoreError {
    ArtifactStoreError::new(kind)
}

fn ensure_private_directory(path: &Path) -> Result<bool, ArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            verify_secure_directory(path)?;
            Ok(false)
        }
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(DIRECTORY_MODE);
            if let Err(create_error) = builder.create(path) {
                if create_error.kind() == std::io::ErrorKind::AlreadyExists {
                    verify_secure_directory(path)?;
                    return Ok(false);
                }
                return Err(error(ArtifactStoreErrorKind::UnsafeRoot));
            }
            fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE))
                .map_err(|_| error(ArtifactStoreErrorKind::UnsafeRoot))?;
            verify_secure_directory(path)?;
            Ok(true)
        }
        Err(_) => Err(error(ArtifactStoreErrorKind::UnsafeRoot)),
    }
}

fn verify_secure_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| error(ArtifactStoreErrorKind::UnsafeRoot))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o777 != DIRECTORY_MODE
    {
        return Err(error(ArtifactStoreErrorKind::UnsafeRoot));
    }
    Ok(())
}

fn create_new_private_file(path: &Path) -> Result<File, ArtifactStoreError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    options
        .open(path)
        .map_err(|_| error(ArtifactStoreErrorKind::Storage))
}

fn open_existing_file(
    path: &Path,
    failure: ArtifactStoreErrorKind,
) -> Result<File, ArtifactStoreError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    options.open(path).map_err(|_| error(failure))
}

fn verify_open_file(file: &File, expected_size: u64) -> Result<(), ArtifactStoreError> {
    let metadata = file
        .metadata()
        .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?;
    verify_file_metadata(&metadata, Some(expected_size))
}

fn verify_file_metadata(
    metadata: &fs::Metadata,
    expected_size: Option<u64>,
) -> Result<(), ArtifactStoreError> {
    if !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o777 != FILE_MODE
        || metadata.nlink() != 1
        || expected_size.is_some_and(|size| metadata.len() != size)
    {
        return Err(error(ArtifactStoreErrorKind::Integrity));
    }
    Ok(())
}

fn verify_path_digest(
    path: &Path,
    expected_digest: Sha256Digest,
    expected_size: u64,
) -> Result<(), ArtifactStoreError> {
    let mut file = open_existing_file(path, ArtifactStoreErrorKind::Integrity)?;
    verify_open_file(&file, expected_size)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = Sha256Digest::from_bytes(hasher.finalize().into());
    if actual != expected_digest {
        return Err(error(ArtifactStoreErrorKind::Integrity));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW | nix::libc::O_DIRECTORY);
    let directory = options
        .open(path)
        .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
    directory
        .sync_all()
        .map_err(|_| error(ArtifactStoreErrorKind::Storage))
}

fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, ArtifactStoreError> {
    let mut entries = fs::read_dir(path)
        .map_err(|_| error(ArtifactStoreErrorKind::Storage))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| error(ArtifactStoreErrorKind::Storage))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn age_seconds(
    metadata: &fs::Metadata,
    observed_at: UtcTimestamp,
) -> Result<u64, ArtifactStoreError> {
    let modified = metadata
        .modified()
        .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error(ArtifactStoreErrorKind::Integrity))?
        .as_secs();
    let observed = u64::try_from(observed_at.to_offset_datetime().unix_timestamp())
        .map_err(|_| error(ArtifactStoreErrorKind::InvalidRequest))?;
    Ok(observed.saturating_sub(modified))
}

fn verify_supported_filesystem(path: &Path) -> Result<(), ArtifactStoreError> {
    if filesystem_is_supported(path)? {
        Ok(())
    } else {
        Err(error(ArtifactStoreErrorKind::UnsupportedFilesystem))
    }
}

#[cfg(target_os = "linux")]
fn filesystem_is_supported(path: &Path) -> Result<bool, ArtifactStoreError> {
    let status =
        statfs::statfs(path).map_err(|_| error(ArtifactStoreErrorKind::UnsupportedFilesystem))?;
    Ok(matches!(
        status.filesystem_type().0 as i64,
        0xEF53 | 0x5846_5342 | 0x9123_683E | 0x0102_1994 | 0x794C_7630
    ))
}

#[cfg(target_os = "macos")]
fn filesystem_is_supported(path: &Path) -> Result<bool, ArtifactStoreError> {
    let status =
        statfs::statfs(path).map_err(|_| error(ArtifactStoreErrorKind::UnsupportedFilesystem))?;
    let name = status.filesystem_type_name();
    Ok(name.eq_ignore_ascii_case("apfs") || name.eq_ignore_ascii_case("hfs"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn filesystem_is_supported(_path: &Path) -> Result<bool, ArtifactStoreError> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "craxii-artifact-test-{}-{}",
                std::process::id(),
                uuid::Uuid::now_v7()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store(root: &TestRoot) -> LocalArtifactStore {
        LocalArtifactStore::initialize(&root.0.join("artifacts")).unwrap()
    }

    fn capture(
        store: &LocalArtifactStore,
        id: ArtifactId,
        limit: u64,
        chunks: &[&[u8]],
    ) -> FinalizedArtifact {
        let mut capture = store
            .begin_capture(BeginArtifactCapture {
                artifact_id: id,
                hard_capture_limit: CanonicalByteCount::try_new(limit).unwrap(),
            })
            .unwrap();
        for chunk in chunks {
            capture.write_chunk(chunk).unwrap();
        }
        capture.finalize().unwrap()
    }

    #[test]
    fn capture_empty_text_binary_chunks_exact_limit_and_over_limit() {
        let root = TestRoot::new();
        let store = store(&root);
        for (chunks, limit, expected, observed, truncated) in [
            (vec![b"".as_slice()], 4, b"".to_vec(), 0, false),
            (
                vec![b"utf".as_slice(), b"-8".as_slice()],
                5,
                b"utf-8".to_vec(),
                5,
                false,
            ),
            (vec![&[0, 255, 1][..]], 3, vec![0, 255, 1], 3, false),
            (
                vec![b"abc".as_slice(), b"def".as_slice()],
                4,
                b"abcd".to_vec(),
                6,
                true,
            ),
        ] {
            let finalized = capture(&store, ArtifactId::generate(), limit, &chunks);
            assert_eq!(
                store.read_verified(finalized.object_reference()).unwrap(),
                expected
            );
            assert_eq!(finalized.observed_byte_count().get(), observed);
            assert_eq!(finalized.truncated(), truncated);
            assert_eq!(
                finalized.storage_key(),
                &ArtifactStorageKey::from_digest(finalized.sha256())
            );
        }
        for path in [&store.root, &store.temp, &store.sha256] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn concurrent_equal_bytes_share_one_object_but_keep_distinct_semantic_ids() {
        let root = TestRoot::new();
        let store = Arc::new(store(&root));
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut capture = store
                        .begin_capture(BeginArtifactCapture {
                            artifact_id: ArtifactId::generate(),
                            hard_capture_limit: CanonicalByteCount::try_new(64).unwrap(),
                        })
                        .unwrap();
                    capture.write_chunk(b"same bytes").unwrap();
                    barrier.wait();
                    capture.finalize().unwrap()
                })
            })
            .collect::<Vec<_>>();
        let left = handles[0].thread().id();
        let mut finalized = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_ne!(left, thread::current().id());
        assert_ne!(finalized[0].artifact_id(), finalized[1].artifact_id());
        assert_eq!(finalized[0].storage_key(), finalized[1].storage_key());
        let shard = store
            .sha256
            .join(&finalized[0].storage_key().as_str()[7..9]);
        assert_eq!(fs::read_dir(shard).unwrap().count(), 1);
        assert_eq!(
            store
                .read_verified(finalized.remove(0).object_reference())
                .unwrap(),
            b"same bytes"
        );
    }

    #[test]
    fn collision_symlink_hardlink_mode_and_corruption_fail_closed() {
        let root = TestRoot::new();
        let store = store(&root);
        let expected = Sha256Digest::hash_bytes(b"expected");
        let key = ArtifactStorageKey::from_digest(expected);
        let shard = store.sha256.join(&key.as_str()[7..9]);
        ensure_private_directory(&shard).unwrap();
        let collision = shard.join(&key.as_str()[10..]);
        fs::write(&collision, b"wrong").unwrap();
        fs::set_permissions(&collision, fs::Permissions::from_mode(FILE_MODE)).unwrap();
        let mut pending = store
            .begin_capture(BeginArtifactCapture {
                artifact_id: ArtifactId::generate(),
                hard_capture_limit: CanonicalByteCount::try_new(64).unwrap(),
            })
            .unwrap();
        pending.write_chunk(b"expected").unwrap();
        assert_eq!(
            pending.finalize().unwrap_err().kind(),
            ArtifactStoreErrorKind::Collision
        );
        fs::remove_file(&collision).unwrap();

        let finalized = capture(&store, ArtifactId::generate(), 64, &[b"secure"]);
        let final_path = store.final_path(finalized.storage_key()).unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            store
                .verify(finalized.object_reference())
                .unwrap_err()
                .kind(),
            ArtifactStoreErrorKind::Integrity
        );
        fs::set_permissions(&final_path, fs::Permissions::from_mode(FILE_MODE)).unwrap();
        let hardlink = root.0.join("hardlink");
        fs::hard_link(&final_path, &hardlink).unwrap();
        assert_eq!(
            store
                .verify(finalized.object_reference())
                .unwrap_err()
                .kind(),
            ArtifactStoreErrorKind::Integrity
        );
        fs::remove_file(hardlink).unwrap();
        fs::write(&final_path, b"damage").unwrap();
        fs::set_permissions(&final_path, fs::Permissions::from_mode(FILE_MODE)).unwrap();
        assert_eq!(
            store
                .read_verified(finalized.object_reference())
                .unwrap_err()
                .kind(),
            ArtifactStoreErrorKind::Integrity
        );
    }

    #[test]
    fn roots_and_storage_keys_reject_symlink_and_traversal_attacks() {
        let target = TestRoot::new();
        let parent = TestRoot::new();
        let link = parent.0.join("artifacts");
        std::os::unix::fs::symlink(&target.0, &link).unwrap();
        assert_eq!(
            LocalArtifactStore::initialize(&link).unwrap_err().kind(),
            ArtifactStoreErrorKind::UnsafeRoot
        );
        for invalid in [
            "sha256/aa/../aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256/AA/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256/ab/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(ArtifactStorageKey::parse_canonical(invalid).is_err());
        }
    }

    #[test]
    fn orphan_scan_reports_partials_referenced_and_unreferenced_without_deletion() {
        let root = TestRoot::new();
        let store = store(&root);
        let referenced = capture(&store, ArtifactId::generate(), 64, &[b"referenced"]);
        let orphan = capture(&store, ArtifactId::generate(), 64, &[b"orphan"]);
        let partial_id = ArtifactId::generate();
        let mut partial = store
            .begin_capture(BeginArtifactCapture {
                artifact_id: partial_id,
                hard_capture_limit: CanonicalByteCount::try_new(64).unwrap(),
            })
            .unwrap();
        partial.write_chunk(b"partial").unwrap();
        drop(partial);
        let referenced_keys = BTreeSet::from([referenced.storage_key().clone()]);
        let now = UtcTimestamp::from_offset_datetime(::time::OffsetDateTime::now_utc()).unwrap();
        let report = store.scan_orphans(&referenced_keys, now).unwrap();
        assert_eq!(report.referenced_final_count, 1);
        assert!(report.orphans.iter().any(
            |item| matches!(item, ArtifactOrphan::TempPartial { artifact_id, .. } if *artifact_id == partial_id)
        ));
        assert!(report.orphans.iter().any(
            |item| matches!(item, ArtifactOrphan::FinalUnreferenced { storage_key, .. } if storage_key == orphan.storage_key())
        ));
        assert!(store.final_path(orphan.storage_key()).unwrap().exists());
        assert!(store.temp.join(format!("{partial_id}.partial")).exists());
    }
}
