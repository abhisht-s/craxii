use std::ffi::OsString;
use std::fmt::{Debug, Formatter};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::fcntl::{Flock, FlockArg};
use nix::sys::statfs;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqliteLockingMode, SqlitePoolOptions,
    SqliteSynchronous,
};
use sqlx::{ConnectOptions, Connection, SqliteConnection, SqlitePool};
use tokio::sync::Mutex;

use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::schema::{DatabaseDisposition, MAX_SUPPORTED_SCHEMA_VERSION, MIGRATOR, classify_schema};

const DATABASE_DIRECTORY: &str = "db";
const LOCK_DIRECTORY: &str = "locks";
const DATABASE_FILENAME: &str = "craxii.sqlite3";
const LOCK_FILENAME: &str = "craxii.lock";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_POOL_CONNECTIONS: u32 = 4;
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(5);
const BUSY_TIMEOUT: Duration = Duration::from_millis(5_000);

/// Sanitized result from `PRAGMA wal_checkpoint(PASSIVE)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointReport {
    busy: u64,
    log_frames: u64,
    checkpointed_frames: u64,
}

impl CheckpointReport {
    #[must_use]
    pub const fn busy(self) -> u64 {
        self.busy
    }

    #[must_use]
    pub const fn log_frames(self) -> u64 {
        self.log_frames
    }

    #[must_use]
    pub const fn checkpointed_frames(self) -> u64 {
        self.checkpointed_frames
    }
}

#[derive(Clone)]
pub struct SqliteRuntime {
    pub(super) inner: Arc<SqliteRuntimeInner>,
}

pub(super) struct SqliteRuntimeInner {
    pub(super) pool: SqlitePool,
    #[allow(dead_code)] // Consumed by the Stage 5 transaction primitive and Stage 6 named writes.
    pub(super) write_coordinator: Arc<Mutex<()>>,
}

impl Debug for SqliteRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteRuntime")
            .field("pool_size", &self.inner.pool.size())
            .finish_non_exhaustive()
    }
}

impl SqliteRuntime {
    /// Lightweight storage probe; this does not imply product readiness.
    pub async fn probe(&self) -> Result<(), SqliteAdapterError> {
        let started = Instant::now();
        let mut connection = self.acquire().await?;
        let value = sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        if value != 1 {
            return Err(SqliteAdapterError::new(
                SqliteFailureKind::InternalInvariant,
            ));
        }
        tracing::debug!(
            target: "craxii::sqlite",
            operation = "probe",
            outcome = "ok",
            duration_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
        );
        Ok(())
    }

    /// Runs a bounded passive checkpoint and returns counters only.
    pub async fn checkpoint_passive(&self) -> Result<CheckpointReport, SqliteAdapterError> {
        let started = Instant::now();
        let mut connection = self.acquire().await?;
        let (busy, log_frames, checkpointed_frames) =
            sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(PASSIVE)")
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
        let report = CheckpointReport {
            busy: nonnegative_counter(busy)?,
            log_frames: nonnegative_counter(log_frames)?,
            checkpointed_frames: nonnegative_counter(checkpointed_frames)?,
        };
        tracing::info!(
            target: "craxii::sqlite",
            operation = "checkpoint_passive",
            outcome = "ok",
            busy = report.busy,
            log_frames = report.log_frames,
            checkpointed_frames = report.checkpointed_frames,
            duration_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
        );
        Ok(report)
    }

    pub async fn close(&self) {
        self.inner.pool.close().await;
    }

    pub(super) async fn acquire(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, SqliteAdapterError> {
        let started = Instant::now();
        let result = self.inner.pool.acquire().await;
        let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        match result {
            Ok(connection) => {
                tracing::debug!(
                    target: "craxii::sqlite",
                    operation = "pool_acquire",
                    outcome = "ok",
                    duration_micros = elapsed
                );
                Ok(connection)
            }
            Err(error) => {
                let classified = SqliteAdapterError::from_sqlx(error);
                tracing::warn!(
                    target: "craxii::sqlite",
                    operation = "pool_acquire",
                    outcome = "error",
                    category = ?classified.kind(),
                    sqlite_code = ?classified.sqlite_code(),
                    duration_micros = elapsed
                );
                Err(classified)
            }
        }
    }
}

fn nonnegative_counter(value: i64) -> Result<u64, SqliteAdapterError> {
    u64::try_from(value).map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InternalInvariant))
}

/// Bootstrap-owned guard retaining both the pool and exclusive process lock.
pub struct SqliteRuntimeGuard {
    runtime: SqliteRuntime,
    process_lock: Flock<File>,
    disposition: DatabaseDisposition,
}

impl Debug for SqliteRuntimeGuard {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteRuntimeGuard")
            .field("disposition", &self.disposition)
            .finish_non_exhaustive()
    }
}

impl SqliteRuntimeGuard {
    pub async fn start(
        state_root: &Path,
        pool_connections: u64,
    ) -> Result<Self, SqliteAdapterError> {
        Self::start_with_timeout(state_root, pool_connections, ACQUIRE_TIMEOUT).await
    }

    async fn start_with_timeout(
        state_root: &Path,
        pool_connections: u64,
        acquire_timeout: Duration,
    ) -> Result<Self, SqliteAdapterError> {
        let pool_connections = u32::try_from(pool_connections)
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InternalInvariant))?;
        if !(1..=MAX_POOL_CONNECTIONS).contains(&pool_connections) {
            return Err(SqliteAdapterError::new(
                SqliteFailureKind::InternalInvariant,
            ));
        }

        let paths = StatePaths::prepare(state_root)?;
        let options = connection_options(&paths.database);
        let mut bootstrap = options
            .clone()
            .connect()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        verify_pragmas(&mut bootstrap)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        tracing::info!(
            target: "craxii::sqlite",
            operation = "database_open",
            outcome = "ok",
            journal_mode = "wal"
        );

        let process_lock = acquire_process_lock(paths.lock_file)?;

        let pool = SqlitePoolOptions::new()
            .max_connections(pool_connections)
            .min_connections(pool_connections)
            .acquire_timeout(acquire_timeout)
            .idle_timeout(None)
            .max_lifetime(None)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    let started = Instant::now();
                    let result = verify_pragmas(connection).await;
                    tracing::debug!(
                        target: "craxii::sqlite",
                        operation = "pool_connection_init",
                        outcome = if result.is_ok() { "ok" } else { "error" },
                        duration_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
                    );
                    result
                })
            })
            .connect_with(options)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;

        let preflight_started = Instant::now();
        run_integrity_checks(&mut bootstrap).await?;
        let preflight = classify_schema(&mut bootstrap).await?;
        trace_integrity("preflight", preflight_started.elapsed(), preflight);
        match preflight {
            DatabaseDisposition::Empty
            | DatabaseDisposition::MigratedUninitialized
            | DatabaseDisposition::Current => {}
            DatabaseDisposition::NewerSchema => {
                return Err(SqliteAdapterError::new(SqliteFailureKind::NewerSchema));
            }
            DatabaseDisposition::Corrupt => {
                return Err(SqliteAdapterError::new(SqliteFailureKind::Corrupt));
            }
            DatabaseDisposition::Inconsistent => {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
        }

        let migration_started = Instant::now();
        MIGRATOR
            .run(&mut bootstrap)
            .await
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        tracing::info!(
            target: "craxii::sqlite",
            operation = "migrate",
            current_version = MAX_SUPPORTED_SCHEMA_VERSION,
            max_supported_version = MAX_SUPPORTED_SCHEMA_VERSION,
            applied_count = if preflight == DatabaseDisposition::Current { 0_u64 } else { 1_u64 },
            duration_micros = u64::try_from(migration_started.elapsed().as_micros()).unwrap_or(u64::MAX)
        );

        let postflight_started = Instant::now();
        run_integrity_checks(&mut bootstrap).await?;
        let disposition = classify_schema(&mut bootstrap).await?;
        trace_integrity("postflight", postflight_started.elapsed(), disposition);
        if disposition != DatabaseDisposition::Current {
            return Err(SqliteAdapterError::new(match disposition {
                DatabaseDisposition::NewerSchema => SqliteFailureKind::NewerSchema,
                DatabaseDisposition::Corrupt => SqliteFailureKind::Corrupt,
                _ => SqliteFailureKind::InconsistentSchema,
            }));
        }

        verify_state_file(&paths.database)?;
        verify_optional_sidecar(&paths.database, "-wal")?;
        verify_optional_sidecar(&paths.database, "-shm")?;
        bootstrap
            .close()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;

        tracing::info!(
            target: "craxii::sqlite",
            operation = "database_disposition",
            disposition = disposition.as_str(),
            max_supported_version = MAX_SUPPORTED_SCHEMA_VERSION
        );

        Ok(Self {
            runtime: SqliteRuntime {
                inner: Arc::new(SqliteRuntimeInner {
                    pool,
                    write_coordinator: Arc::new(Mutex::new(())),
                }),
            },
            process_lock,
            disposition,
        })
    }

    #[must_use]
    pub const fn runtime(&self) -> &SqliteRuntime {
        &self.runtime
    }

    #[must_use]
    pub const fn disposition(&self) -> DatabaseDisposition {
        self.disposition
    }

    pub async fn shutdown(self) {
        self.runtime.close().await;
        drop(self.process_lock);
    }
}

fn trace_integrity(phase: &'static str, duration: Duration, disposition: DatabaseDisposition) {
    tracing::info!(
        target: "craxii::sqlite",
        operation = "integrity_check",
        phase,
        outcome = "ok",
        disposition = disposition.as_str(),
        duration_micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    );
}

struct StatePaths {
    database: PathBuf,
    lock_file: File,
}

impl StatePaths {
    fn prepare(state_root: &Path) -> Result<Self, SqliteAdapterError> {
        verify_secure_directory(state_root)?;
        verify_supported_filesystem(state_root)?;

        let database_directory = state_root.join(DATABASE_DIRECTORY);
        let lock_directory = state_root.join(LOCK_DIRECTORY);
        ensure_private_directory(&database_directory)?;
        ensure_private_directory(&lock_directory)?;

        let database = database_directory.join(DATABASE_FILENAME);
        if database.exists() {
            verify_state_file(&database)?;
        } else {
            let file = open_private_file(&database)?;
            verify_open_file(&file)?;
            drop(file);
        }

        let lock_path = lock_directory.join(LOCK_FILENAME);
        if lock_path.exists() {
            verify_state_file(&lock_path)?;
        }
        let lock_file = open_private_file(&lock_path)?;
        verify_open_file(&lock_file)?;

        Ok(Self {
            database,
            lock_file,
        })
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), SqliteAdapterError> {
    match fs::symlink_metadata(path) {
        Ok(_) => verify_secure_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(DIRECTORY_MODE);
            builder
                .create(path)
                .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?;
            fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE))
                .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?;
            verify_secure_directory(path)
        }
        Err(_) => Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath)),
    }
}

fn verify_secure_directory(path: &Path) -> Result<(), SqliteAdapterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath));
    }
    Ok(())
}

fn open_private_file(path: &Path) -> Result<File, SqliteAdapterError> {
    match private_file_options(true).open(path) {
        Ok(file) => {
            file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
                .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?;
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            private_file_options(false)
                .open(path)
                .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))
        }
        Err(_) => Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath)),
    }
}

fn private_file_options(create_new: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create_new)
        .mode(FILE_MODE)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    options
}

fn verify_open_file(file: &File) -> Result<(), SqliteAdapterError> {
    verify_file_metadata(
        &file
            .metadata()
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?,
    )
}

fn verify_state_file(path: &Path) -> Result<(), SqliteAdapterError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath))?;
    if metadata.file_type().is_symlink() {
        return Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath));
    }
    verify_file_metadata(&metadata)
}

fn verify_file_metadata(metadata: &fs::Metadata) -> Result<(), SqliteAdapterError> {
    if !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath));
    }
    Ok(())
}

fn verify_optional_sidecar(database: &Path, suffix: &str) -> Result<(), SqliteAdapterError> {
    let mut name: OsString = database.as_os_str().to_owned();
    name.push(suffix);
    let path = PathBuf::from(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => verify_file_metadata(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(SqliteAdapterError::new(SqliteFailureKind::UnsafeStatePath)),
    }
}

fn acquire_process_lock(file: File) -> Result<Flock<File>, SqliteAdapterError> {
    Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| {
        if errno == nix::errno::Errno::EWOULDBLOCK || errno == nix::errno::Errno::EAGAIN {
            SqliteAdapterError::new(SqliteFailureKind::AlreadyOwned)
        } else {
            SqliteAdapterError::new(SqliteFailureKind::Storage)
        }
    })
}

fn verify_supported_filesystem(path: &Path) -> Result<(), SqliteAdapterError> {
    let supported = filesystem_is_supported(path)?;
    if supported {
        Ok(())
    } else {
        Err(SqliteAdapterError::new(
            SqliteFailureKind::UnsupportedFilesystem,
        ))
    }
}

#[cfg(target_os = "linux")]
fn filesystem_is_supported(path: &Path) -> Result<bool, SqliteAdapterError> {
    let status = statfs::statfs(path)
        .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsupportedFilesystem))?;
    Ok(is_supported_linux_filesystem(
        status.filesystem_type().0 as i64,
    ))
}

#[cfg(target_os = "macos")]
fn filesystem_is_supported(path: &Path) -> Result<bool, SqliteAdapterError> {
    let status = statfs::statfs(path)
        .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::UnsupportedFilesystem))?;
    Ok(is_supported_macos_filesystem(status.filesystem_type_name()))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn filesystem_is_supported(_path: &Path) -> Result<bool, SqliteAdapterError> {
    Ok(false)
}

#[cfg(any(target_os = "linux", test))]
const fn is_supported_linux_filesystem(magic: i64) -> bool {
    matches!(
        magic,
        0xEF53 | 0x5846_5342 | 0x9123_683E | 0x0102_1994 | 0x794C_7630
    )
}

fn is_supported_macos_filesystem(name: &str) -> bool {
    name.eq_ignore_ascii_case("apfs") || name.eq_ignore_ascii_case("hfs")
}

fn connection_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .locking_mode(SqliteLockingMode::Normal)
        .pragma("temp_store", "MEMORY")
        .pragma("wal_autocheckpoint", "1000")
        .pragma("trusted_schema", "OFF")
        .pragma("recursive_triggers", "OFF")
        .pragma("secure_delete", "OFF")
        .pragma("mmap_size", "0")
        .pragma("cache_size", "-2000")
        .pragma("journal_size_limit", "-1")
        .pragma("fullfsync", "ON")
        .disable_statement_logging()
}

async fn verify_pragmas(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?;
    let locking_mode = sqlx::query_scalar::<_, String>("PRAGMA locking_mode")
        .fetch_one(&mut *connection)
        .await?;
    let integers = [
        sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA temp_store")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA wal_autocheckpoint")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA trusted_schema")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA recursive_triggers")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA secure_delete")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA mmap_size")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA cache_size")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA journal_size_limit")
            .fetch_one(&mut *connection)
            .await?,
        sqlx::query_scalar::<_, i64>("PRAGMA fullfsync")
            .fetch_one(&mut *connection)
            .await?,
    ];
    let expected = [2, 1, 5_000, 2, 1_000, 0, 0, 0, 0, -2_000, -1, 1];
    if !journal_mode.eq_ignore_ascii_case("wal")
        || !locking_mode.eq_ignore_ascii_case("normal")
        || integers != expected
    {
        return Err(sqlx::Error::Protocol(
            "SQLite connection PRAGMA mismatch".to_owned(),
        ));
    }
    Ok(())
}

async fn run_integrity_checks(connection: &mut SqliteConnection) -> Result<(), SqliteAdapterError> {
    let quick = sqlx::query_scalar::<_, String>("PRAGMA quick_check")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if quick.as_slice() != ["ok"] {
        return Err(SqliteAdapterError::new(SqliteFailureKind::Corrupt));
    }
    let foreign_key_violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::schema_query)?;
    if !foreign_key_violations.is_empty() {
        return Err(SqliteAdapterError::new(
            SqliteFailureKind::InconsistentSchema,
        ));
    }
    verify_pragmas(connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)
}

#[cfg(test)]
pub(super) mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::adapters::sqlite::transaction::WriteTransaction;
    use crate::domain::ErrorCategory;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "craxii-sqlite-test-{}-{}-{sequence}",
                std::process::id(),
                uuid::Uuid::now_v7()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn runtime(root: &TestRoot, size: u64) -> SqliteRuntimeGuard {
        SqliteRuntimeGuard::start(root.path(), size).await.unwrap()
    }

    #[tokio::test]
    async fn state_root_must_exist_be_directory_private_and_not_a_symlink() {
        let missing = std::env::temp_dir().join(format!("craxii-missing-{}", uuid::Uuid::now_v7()));
        assert_eq!(
            SqliteRuntimeGuard::start(&missing, 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );

        let root = TestRoot::new();
        let file = root.path().join("not-a-directory");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(&file, 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );

        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );
        fs::set_permissions(root.path(), fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();

        let target = TestRoot::new();
        let link_parent = TestRoot::new();
        let link = link_parent.path().join("state-link");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(&link, 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );
    }

    #[tokio::test]
    async fn creates_exact_private_layout_and_rejects_leaf_symlink_and_hard_link() {
        let root = TestRoot::new();
        let guard = runtime(&root, 1).await;
        let database = root.path().join("db/craxii.sqlite3");
        let lock = root.path().join("locks/craxii.lock");
        let mut root_entries = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        root_entries.sort();
        assert_eq!(
            root_entries,
            [OsString::from("db"), OsString::from("locks")]
        );
        for directory in [root.path().join("db"), root.path().join("locks")] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for file in [&database, &lock] {
            let metadata = fs::metadata(file).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
        }
        for sidecar in [
            root.path().join("db/craxii.sqlite3-wal"),
            root.path().join("db/craxii.sqlite3-shm"),
        ] {
            if sidecar.exists() {
                let metadata = fs::metadata(sidecar).unwrap();
                assert_eq!(metadata.permissions().mode() & 0o077, 0);
                assert_eq!(metadata.nlink(), 1);
            }
        }
        assert_eq!(guard.disposition(), DatabaseDisposition::Current);
        guard.shutdown().await;

        let linked = root.path().join("db/linked.sqlite3");
        fs::hard_link(&database, &linked).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );
        fs::remove_file(linked).unwrap();

        fs::remove_file(&database).unwrap();
        let sentinel = root.path().join("sentinel");
        fs::write(&sentinel, b"sentinel").unwrap();
        std::os::unix::fs::symlink(&sentinel, &database).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );

        let lock_root = TestRoot::new();
        let lock_guard = runtime(&lock_root, 1).await;
        lock_guard.shutdown().await;
        let lock = lock_root.path().join("locks/craxii.lock");
        let linked_lock = lock_root.path().join("locks/linked.lock");
        fs::hard_link(&lock, &linked_lock).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(lock_root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );
    }

    #[tokio::test]
    async fn rejects_unsafe_existing_leaf_types_and_permissions() {
        let database_type = TestRoot::new();
        fs::create_dir(database_type.path().join("db")).unwrap();
        fs::set_permissions(
            database_type.path().join("db"),
            fs::Permissions::from_mode(DIRECTORY_MODE),
        )
        .unwrap();
        fs::create_dir(database_type.path().join("db/craxii.sqlite3")).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(database_type.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );

        let database_mode = TestRoot::new();
        fs::create_dir(database_mode.path().join("db")).unwrap();
        fs::set_permissions(
            database_mode.path().join("db"),
            fs::Permissions::from_mode(DIRECTORY_MODE),
        )
        .unwrap();
        let database = database_mode.path().join("db/craxii.sqlite3");
        fs::write(&database, b"").unwrap();
        fs::set_permissions(&database, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(database_mode.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );

        let lock_type = TestRoot::new();
        fs::create_dir(lock_type.path().join("locks")).unwrap();
        fs::set_permissions(
            lock_type.path().join("locks"),
            fs::Permissions::from_mode(DIRECTORY_MODE),
        )
        .unwrap();
        fs::create_dir(lock_type.path().join("locks/craxii.lock")).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(lock_type.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::UnsafeStatePath
        );
    }

    #[test]
    fn filesystem_classifiers_allow_only_frozen_local_classes() {
        for magic in [0xEF53, 0x5846_5342, 0x9123_683E, 0x0102_1994, 0x794C_7630] {
            assert!(is_supported_linux_filesystem(magic));
        }
        for magic in [0x6969, 0x517B, 0x9FA0, 0x00C3_6400, 0x6573_5546, -1] {
            assert!(!is_supported_linux_filesystem(magic));
        }
        assert!(is_supported_macos_filesystem("apfs"));
        assert!(is_supported_macos_filesystem("hfs"));
        assert!(!is_supported_macos_filesystem("nfs"));
        assert!(!is_supported_macos_filesystem("smbfs"));
        assert!(!is_supported_macos_filesystem("fusefs"));
    }

    #[tokio::test]
    async fn actual_temp_filesystem_is_accepted_and_paths_never_enter_errors() {
        let root = TestRoot::new();
        assert!(filesystem_is_supported(root.path()).unwrap());
        let guard = runtime(&root, 1).await;
        guard.shutdown().await;

        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let error = SqliteRuntimeGuard::start(root.path(), 1).await.unwrap_err();
        let path = root.path().to_string_lossy();
        assert!(!error.to_string().contains(path.as_ref()));
        assert!(!format!("{error:?}").contains(path.as_ref()));
    }

    #[tokio::test]
    async fn every_eager_pool_connection_has_every_exact_pragma() {
        for size in [1_u64, 4] {
            let root = TestRoot::new();
            let guard = runtime(&root, size).await;
            let mut connections = Vec::new();
            for _ in 0..size {
                let mut connection = guard.runtime().acquire().await.unwrap();
                verify_pragmas(&mut connection).await.unwrap();
                connections.push(connection);
            }
            assert_eq!(connections.len(), size as usize);
            drop(connections);
            guard.shutdown().await;
        }
    }

    #[tokio::test]
    async fn pool_size_must_be_within_the_validated_one_through_four_range() {
        for size in [0_u64, 5, u64::MAX] {
            let root = TestRoot::new();
            assert_eq!(
                SqliteRuntimeGuard::start(root.path(), size)
                    .await
                    .unwrap_err()
                    .kind(),
                SqliteFailureKind::InternalInvariant
            );
        }
    }

    #[tokio::test]
    async fn replacement_connection_reapplies_and_verifies_pragmas() {
        let root = TestRoot::new();
        let guard = runtime(&root, 1).await;
        let connection = guard.runtime().acquire().await.unwrap();
        connection.close().await.unwrap();
        let mut replacement = guard.runtime().acquire().await.unwrap();
        verify_pragmas(&mut replacement).await.unwrap();
        drop(replacement);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_readers_and_close_reopen_are_file_backed() {
        let root = TestRoot::new();
        let guard = runtime(&root, 4).await;
        let tasks = (0..8)
            .map(|_| {
                let runtime = guard.runtime().clone();
                tokio::spawn(async move { runtime.probe().await })
            })
            .collect::<Vec<_>>();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        guard.shutdown().await;
        let reopened = runtime(&root, 1).await;
        reopened.runtime().probe().await.unwrap();
        reopened.shutdown().await;
    }

    #[tokio::test]
    async fn acquisition_timeout_is_bounded_and_production_constant_is_five_seconds() {
        assert_eq!(ACQUIRE_TIMEOUT, Duration::from_secs(5));
        let root = TestRoot::new();
        let guard =
            SqliteRuntimeGuard::start_with_timeout(root.path(), 1, Duration::from_millis(25))
                .await
                .unwrap();
        let held = guard.runtime().acquire().await.unwrap();
        let error = guard.runtime().acquire().await.unwrap_err();
        assert_eq!(error.kind(), SqliteFailureKind::Storage);
        drop(held);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn transactions_commit_rollback_drop_and_begin_immediate() {
        let root = TestRoot::new();
        let guard = runtime(&root, 2).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "CREATE TABLE stage5_test (id INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE)",
        )
        .execute(&mut *connection)
        .await
        .unwrap();
        drop(connection);

        let mut committed = WriteTransaction::begin(guard.runtime(), "test_commit")
            .await
            .unwrap();
        sqlx::query("INSERT INTO stage5_test (id, value) VALUES (1, 'committed')")
            .execute(committed.connection())
            .await
            .unwrap();
        committed.commit().await.unwrap();

        let mut rolled_back = WriteTransaction::begin(guard.runtime(), "test_rollback")
            .await
            .unwrap();
        sqlx::query("INSERT INTO stage5_test (id, value) VALUES (2, 'rolled-back')")
            .execute(rolled_back.connection())
            .await
            .unwrap();
        rolled_back.rollback().await.unwrap();

        let mut dropped = WriteTransaction::begin(guard.runtime(), "test_drop")
            .await
            .unwrap();
        sqlx::query("INSERT INTO stage5_test (id, value) VALUES (3, 'dropped')")
            .execute(dropped.connection())
            .await
            .unwrap();
        drop(dropped);

        let mut check = guard.runtime().acquire().await.unwrap();
        let rows = sqlx::query_scalar::<_, i64>("SELECT id FROM stage5_test ORDER BY id")
            .fetch_all(&mut *check)
            .await
            .unwrap();
        assert_eq!(rows, [1]);
        drop(check);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn write_coordinator_serializes_and_raw_second_writer_reports_busy() {
        let root = TestRoot::new();
        let guard = runtime(&root, 2).await;
        let first = WriteTransaction::begin(guard.runtime(), "first")
            .await
            .unwrap();
        let runtime = guard.runtime().clone();
        let waiting =
            tokio::spawn(async move { WriteTransaction::begin(&runtime, "second").await });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        first.rollback().await.unwrap();
        waiting.await.unwrap().unwrap().rollback().await.unwrap();

        let mut raw_first = guard.runtime().acquire().await.unwrap();
        sqlx::query("CREATE TABLE busy_test (id INTEGER PRIMARY KEY)")
            .execute(&mut *raw_first)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *raw_first)
            .await
            .unwrap();
        let mut raw_second = connection_options(&root.path().join("db/craxii.sqlite3"))
            .busy_timeout(Duration::from_millis(25))
            .connect()
            .await
            .unwrap();
        let error = sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut raw_second)
            .await
            .unwrap_err();
        assert_eq!(
            SqliteAdapterError::from_sqlx(error).kind(),
            SqliteFailureKind::BusyOrLocked
        );
        let rows = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM busy_test")
            .fetch_one(&mut raw_second)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        sqlx::query("ROLLBACK")
            .execute(&mut *raw_first)
            .await
            .unwrap();
        raw_second.close().await.unwrap();
        drop(raw_first);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn constraint_failures_are_internal_invariants_and_not_os_errno() {
        let root = TestRoot::new();
        let guard = runtime(&root, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("CREATE TABLE parent (id INTEGER PRIMARY KEY)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parent(id), checked INTEGER NOT NULL CHECK (checked > 0), value TEXT UNIQUE)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO parent VALUES (1)")
            .execute(&mut *connection)
            .await
            .unwrap();
        for statement in [
            "INSERT INTO child VALUES (1, 99, 1, 'a')",
            "INSERT INTO child VALUES (2, 1, 0, 'b')",
        ] {
            let error = sqlx::query(statement)
                .execute(&mut *connection)
                .await
                .unwrap_err();
            let classified = SqliteAdapterError::from_sqlx(error);
            assert_eq!(classified.kind(), SqliteFailureKind::InternalInvariant);
            let normalized = classified.normalized();
            assert_eq!(normalized.category(), ErrorCategory::InternalInvariantError);
            assert!(
                !serde_json::to_string(&normalized)
                    .unwrap()
                    .contains("os_errno")
            );
        }
        drop(connection);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn migration_version_one_inventory_is_exact_and_reopen_is_idempotent() {
        let root = TestRoot::new();
        let guard = runtime(&root, 1).await;
        assert_eq!(guard.disposition(), DatabaseDisposition::Current);
        let mut connection = guard.runtime().acquire().await.unwrap();
        let tables = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let expected_tables = std::iter::once("_sqlx_migrations".to_owned())
            .chain(
                crate::adapters::sqlite::schema::PRODUCT_TABLES
                    .iter()
                    .map(|value| (*value).to_owned()),
            )
            .collect::<Vec<_>>();
        assert_eq!(tables, expected_tables);
        for forbidden in [
            "work_item_inputs",
            "journal_events",
            "stream_heads",
            "context_manifests",
            "model_invocations",
            "tool_executions",
            "artifacts",
        ] {
            assert!(!tables.iter().any(|object| object == forbidden));
        }
        for table in crate::adapters::sqlite::schema::PRODUCT_TABLES {
            let count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(&mut *connection)
            .await
            .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        drop(connection);
        guard.shutdown().await;

        let reopened = runtime(&root, 1).await;
        assert_eq!(reopened.disposition(), DatabaseDisposition::Current);
        reopened.shutdown().await;
    }

    #[tokio::test]
    async fn fresh_database_is_empty_before_migration_one_runs() {
        assert_eq!(MAX_SUPPORTED_SCHEMA_VERSION, 1);
        let root = TestRoot::new();
        let paths = StatePaths::prepare(root.path()).unwrap();
        let mut connection = connection_options(&paths.database).connect().await.unwrap();
        verify_pragmas(&mut connection).await.unwrap();
        run_integrity_checks(&mut connection).await.unwrap();
        assert_eq!(
            classify_schema(&mut connection).await.unwrap(),
            DatabaseDisposition::Empty
        );
        connection.close().await.unwrap();
    }

    async fn mutate_database(root: &TestRoot, statement: &'static str) {
        let guard = runtime(root, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn newer_dirty_malformed_and_unexpected_schema_fail_closed() {
        let newer = TestRoot::new();
        mutate_database(&newer, "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (2, 'future', 1, X'000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000', 0)").await;
        assert_eq!(
            SqliteRuntimeGuard::start(newer.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::NewerSchema
        );

        let dirty = TestRoot::new();
        mutate_database(
            &dirty,
            "UPDATE _sqlx_migrations SET success = 0 WHERE version = 1",
        )
        .await;
        assert_eq!(
            SqliteRuntimeGuard::start(dirty.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );

        let unexpected = TestRoot::new();
        mutate_database(
            &unexpected,
            "CREATE TABLE unexpected_stage5_object (id INTEGER)",
        )
        .await;
        assert_eq!(
            SqliteRuntimeGuard::start(unexpected.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );

        let malformed = TestRoot::new();
        let guard = runtime(&malformed, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("ALTER TABLE _sqlx_migrations RENAME TO old_sqlx_migrations")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("DROP TABLE old_sqlx_migrations")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        guard.shutdown().await;
        assert_eq!(
            SqliteRuntimeGuard::start(malformed.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );

        let malformed_shape = TestRoot::new();
        let guard = runtime(&malformed_shape, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("ALTER TABLE _sqlx_migrations RENAME TO old_sqlx_migrations")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE _sqlx_migrations (version TEXT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("DROP TABLE old_sqlx_migrations")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        guard.shutdown().await;
        assert_eq!(
            SqliteRuntimeGuard::start(malformed_shape.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );

        let contradictory = TestRoot::new();
        mutate_database(
            &contradictory,
            "UPDATE _sqlx_migrations SET success = 2 WHERE version = 1",
        )
        .await;
        assert_eq!(
            SqliteRuntimeGuard::start(contradictory.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema
        );
    }

    #[tokio::test]
    async fn corruption_and_foreign_key_check_fail_closed_without_repair() {
        let corrupt = TestRoot::new();
        fs::create_dir(corrupt.path().join("db")).unwrap();
        fs::set_permissions(corrupt.path().join("db"), fs::Permissions::from_mode(0o700)).unwrap();
        let database = corrupt.path().join("db/craxii.sqlite3");
        fs::write(&database, b"not-a-database sentinel").unwrap();
        fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            SqliteRuntimeGuard::start(corrupt.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::Corrupt
        );

        let inconsistent = TestRoot::new();
        let guard = runtime(&inconsistent, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        sqlx::query("CREATE TABLE fk_parent (id INTEGER PRIMARY KEY)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE fk_child (parent_id INTEGER REFERENCES fk_parent(id))")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fk_child VALUES (99)")
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);
        guard.shutdown().await;
        assert!(matches!(
            SqliteRuntimeGuard::start(inconsistent.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::InconsistentSchema | SqliteFailureKind::InternalInvariant
        ));
    }

    #[tokio::test]
    async fn readonly_and_raw_query_failures_are_safely_classified_and_redacted() {
        let readonly = TestRoot::new();
        let guard = runtime(&readonly, 1).await;
        let database = readonly.path().join("db/craxii.sqlite3");
        let mut read_only_connection = connection_options(&database)
            .read_only(true)
            .connect()
            .await
            .unwrap();
        let error = sqlx::query("CREATE TABLE forbidden_readonly_write (id INTEGER)")
            .execute(&mut read_only_connection)
            .await
            .unwrap_err();
        assert_eq!(
            SqliteAdapterError::from_sqlx(error).kind(),
            SqliteFailureKind::Storage
        );
        read_only_connection.close().await.unwrap();
        guard.shutdown().await;

        let raw = TestRoot::new();
        let guard = runtime(&raw, 1).await;
        let mut connection = guard.runtime().acquire().await.unwrap();
        let classified = SqliteAdapterError::from_sqlx(
            sqlx::query("SELECT secret_path_and_sql_sentinel FROM absent_table")
                .execute(&mut *connection)
                .await
                .unwrap_err(),
        );
        for surface in [classified.to_string(), format!("{classified:?}")] {
            assert!(!surface.contains("secret_path_and_sql_sentinel"));
            assert!(!surface.contains("absent_table"));
            assert!(!surface.contains("SELECT"));
        }
        drop(connection);
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn passive_checkpoint_reports_sane_fields() {
        let root = TestRoot::new();
        let guard = runtime(&root, 1).await;
        let report = guard.runtime().checkpoint_passive().await.unwrap();
        assert!(report.checkpointed_frames() <= report.log_frames());
        assert!(report.busy() <= 1);
        guard.shutdown().await;
    }

    fn spawn_child(mode: &str, root: &Path) -> std::process::Child {
        Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("adapters::sqlite::runtime::tests::child_process_helper")
            .arg("--nocapture")
            .env("CRAXII_STAGE5_CHILD_MODE", mode)
            .env("CRAXII_STAGE5_CHILD_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn wait_for_child_ready(child: &mut std::process::Child) {
        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "child exited before ready"
            );
            if line.trim() == "STAGE5_CHILD_READY" {
                break;
            }
        }
    }

    #[test]
    fn child_process_helper() {
        let Ok(mode) = std::env::var("CRAXII_STAGE5_CHILD_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("CRAXII_STAGE5_CHILD_ROOT").unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let guard = SqliteRuntimeGuard::start(&root, 2).await.unwrap();
            if mode == "lock" {
                println!("STAGE5_CHILD_READY");
                std::io::stdout().flush().unwrap();
                let mut byte = [0_u8; 1];
                let _ = std::io::stdin().read(&mut byte);
                return;
            }

            let mut connection = guard.runtime().acquire().await.unwrap();
            sqlx::query("CREATE TABLE IF NOT EXISTS crash_test (id INTEGER PRIMARY KEY)")
                .execute(&mut *connection)
                .await
                .unwrap();
            drop(connection);
            let mut transaction = WriteTransaction::begin(guard.runtime(), "child_crash")
                .await
                .unwrap();
            sqlx::query("INSERT INTO crash_test (id) VALUES (1)")
                .execute(transaction.connection())
                .await
                .unwrap();
            if mode == "committed" {
                transaction.commit().await.unwrap();
            }
            println!("STAGE5_CHILD_READY");
            std::io::stdout().flush().unwrap();
            let mut byte = [0_u8; 1];
            let _ = std::io::stdin().read(&mut byte);
        });
    }

    #[tokio::test]
    async fn lifetime_lock_excludes_second_process_and_releases_after_exit() {
        let root = TestRoot::new();
        let mut child = spawn_child("lock", root.path());
        wait_for_child_ready(&mut child);
        assert_eq!(
            SqliteRuntimeGuard::start(root.path(), 1)
                .await
                .unwrap_err()
                .kind(),
            SqliteFailureKind::AlreadyOwned
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let guard = runtime(&root, 1).await;
        guard.shutdown().await;
    }

    #[tokio::test]
    async fn committed_wal_survives_abrupt_exit_and_uncommitted_row_does_not() {
        for (mode, expected) in [("committed", vec![1_i64]), ("uncommitted", Vec::new())] {
            let root = TestRoot::new();
            let mut child = spawn_child(mode, root.path());
            wait_for_child_ready(&mut child);
            let wal = root.path().join("db/craxii.sqlite3-wal");
            let shm = root.path().join("db/craxii.sqlite3-shm");
            assert!(wal.exists());
            assert!(shm.exists());
            child.kill().unwrap();
            child.wait().unwrap();

            let database = root.path().join("db/craxii.sqlite3");
            let mut connection = connection_options(&database).connect().await.unwrap();
            verify_pragmas(&mut connection).await.unwrap();
            let rows = sqlx::query_scalar::<_, i64>("SELECT id FROM crash_test ORDER BY id")
                .fetch_all(&mut connection)
                .await
                .unwrap();
            assert_eq!(rows, expected);
            connection.close().await.unwrap();
        }
    }
}
