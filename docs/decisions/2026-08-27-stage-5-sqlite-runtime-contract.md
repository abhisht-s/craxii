# Stage 5 SQLite runtime contract decision

## Date

2026-08-27

## Status

Accepted

## Context / problem

Stage 5 must establish one trustworthy file-backed SQLite lifecycle before any Craxii domain table
exists. Later persistence must not leak SQLx upward, weaken crash durability, permit two owners, or
turn transactions into arbitrary application work scopes.

## Decision

### Dependencies and build

- Use SQLx 0.9 with exactly `sqlite-bundled`, `runtime-tokio`, `migrate`, and `macros`; Tokio 1.53
  with exactly `macros`, `rt-multi-thread`, `sync`, and `time`; and nix 0.31 with exactly `fs`.
- SQLx owns SQLite. `libsqlite3-sys` compiles the bundled amalgamation, so production has no system
  SQLite, bindgen-at-build-time, OpenSSL, TLS, SQLCipher, or extension-loading dependency.
- The top-level SQLx `any` feature is off. SQLx 0.9's proc-macro implementation nevertheless
  compiles its internal `sqlx-core/any` feature; no Any, MySQL, or PostgreSQL driver is active in
  Craxii's normal/runtime graph. Keeping embedded `migrate!` is preferred to runtime migration-file
  discovery, and this transitive macro detail is recorded rather than misrepresented.
- The locked graph adds the OSI-approved Zlib license through `foldhash`; Zlib is added to the
  repository allowlist as a general accepted license, without a crate-specific exception.
- Reject rusqlite because the architecture already selects async SQLx, raw SQLite FFI because it
  expands unsafe/native ownership, and handwritten async/locking primitives because Tokio and nix
  provide the reviewed seams.

### State Store and SQLx containment

- `StateStore` exposes one dependency-neutral method per durable business intent, guarded expected
  state/version/runtime/current-attempt inputs, and committed version/event-range receipts. It has
  no generic CRUD, query, callback, transaction, connection, pool, row, SQL, or path surface.
- SQLx names and types remain under `backend/src/adapters/sqlite`; bootstrap owns only the adapter
  facade and retains its lifetime guard. Application depends inward on ports and domain only.
  Transactions never escape the SQLite adapter.

### Paths, objects, and filesystems

- Derive the database as `<state_root>/db/craxii.sqlite3` and the lifetime lock as
  `<state_root>/locks/craxii.lock`; SQLite URLs, URI parameters, and production `:memory:` mode are
  forbidden.
- `state_root` must preexist. Stage 5 may create only `db/` and `locks/`, each mode `0700`; new
  database and lock files are mode `0600`. Production provisioning owns `state_root`, its user, and
  ownership; runtime performs no `chown`.
- Fail closed for missing/non-directory/symlink leaves, unexpected file types, group/world-accessible
  state objects, and reliably observable multi-link database/lock files.
- Linux allows ext2/3/4, XFS, Btrfs, tmpfs, and overlayfs. macOS allows APFS and HFS/HFS+. Known
  remote types and every unknown type are rejected. This classifies the mounted filesystem only and
  does not prove EBS backing.

### SQLite connections and durability

- Every connection sets and verifies `journal_mode=WAL`, `synchronous=FULL`, `foreign_keys=ON`,
  `busy_timeout=5000`, `temp_store=MEMORY`, `wal_autocheckpoint=1000`,
  `locking_mode=NORMAL`, `trusted_schema=OFF`, `recursive_triggers=OFF`, `secure_delete=OFF`,
  `mmap_size=0`, `cache_size=-2000`, `journal_size_limit=-1`, and `fullfsync=ON`.
- `fullfsync` is verified where effective. The value is still requested and inspected on Linux, but
  no Apple durability claim is inferred from Linux's no-op behavior.
- The validated pool maximum is `1..=4`, default four; minimum equals maximum for eager validation;
  acquire timeout is five seconds; idle timeout and maximum lifetime are disabled. Replacement
  connections run the same initialization and verification.
- One Tokio `WriteCoordinator` serializes Craxii writes. Lock order is coordinator, pool
  connection, then `BEGIN IMMEDIATE`. Transactions explicitly commit or roll back; drop starts a
  rollback. There is no savepoint/nested or generic retry surface.
- Transactions are short, bounded, and SQLite-only. Provider/network/workstation/process calls,
  filesystem content reads, artifact rename, client delivery, and unrelated sleeps/waits never run
  inside them.
- A successful FULL/WAL commit is treated as durable across process crash and conforming local
  storage crash boundaries. It does not protect against filesystem/storage failure, EBS volume
  loss, account or Availability Zone loss, or missing backup.

### Migration, classification, and integrity

- `MAX_SUPPORTED_SCHEMA_VERSION` remains zero. Stage 5 embeds an empty SQLx migration set and may
  create only SQLx-owned `_sqlx_migrations` metadata; there is no `user_version`, Craxii schema
  table, fake migration 0000, or domain object. Stage 6 owns migration 0001 and the canonical schema.
- Preflight classifies `empty`, `migrated_uninitialized`, `newer_schema`, `corrupt`, or
  `inconsistent`. Positive applied versions are newer; failed/malformed/contradictory metadata or
  unexpected version-zero objects are inconsistent. Only the first two may proceed.
- Run `quick_check`, `foreign_key_check`, WAL verification, and compatibility inspection before
  migration mutation, then repeat the applicable checks afterward. Never auto-repair.

### Ownership, startup, errors, and operations

- A nonblocking Unix advisory exclusive lock is held on the private lock file for the runtime
  guard's lifetime. A PID file is not the exclusion mechanism.
- Bootstrap validates config/metadata/health/telemetry, derives safe state paths, opens and verifies
  SQLite, acquires the lifetime lock, eagerly opens the pool, performs preflight, runs the empty
  migration harness, performs postflight, and returns a guard owning pool plus lock. Successful
  Stage 5 startup stays `live_unready`.
- Outward startup failures use fixed categories for database lifecycle, newer schema, already-owned
  state, and corruption/inconsistency. Adapter Display/Debug/source chains never expose raw paths,
  SQL, binds, SQLx/SQLite messages, content, or secrets. Numeric SQLite codes remain sanitized
  diagnostics and are never mislabeled as OS errno.
- Trace only sanitized operation/category/duration/count information. A lightweight `SELECT 1`
  probe cannot make readiness true. Passive checkpoint reporting is observational; graceful pool
  close is sufficient and shutdown does not force `TRUNCATE`.

### Testing and deferrals

- Tests use private file-backed roots and test-only SQL tables. Child processes prove lifetime
  locking, clean and abrupt committed reopen, and absence of an abruptly abandoned transaction.
  Pure filesystem classification, every PRAGMA, replacement connections, pool sizes, readers,
  coordinator serialization, busy/acquire behavior, transactions, migration classifications,
  integrity, checkpoint fields, and redaction are permanent tests.
- Stage 6 owns domain schema; Stage 7 owns bootstrap/journal persistence; Stage 18 owns the complete
  crash-failpoint matrix; Stage 29 owns backup/restore; Stage 32 owns deployment verification.

## Rationale

Bundled SQLite makes the tested engine the deployed engine. Early writer acquisition plus one local
coordinator makes contention explicit without weakening cross-process correctness. Named Store
intents preserve domain meaning while allowing the adapter to enforce atomicity and constraints.

## Consequences / tradeoffs

- The native C build increases compile time and the locked graph; SQLx macros also add build-time
  parsing dependencies.
- Fail-closed filesystem and schema checks reject unfamiliar but potentially usable environments.
- Four eager connections consume more handles but expose configuration failures before service.
- FULL synchronization favors durability over marginal throughput; passive checkpointing is not a
  backup or data-loss boundary.

## Rollback / change path

- Patch updates remain lockfile/governance changes. Replacing SQLx/SQLite requires preserving the
  StateStore intents, migration history, durability, error, and crash tests.
- A new local filesystem type requires a reviewed platform allowlist amendment. Remote databases
  require a new adapter rather than weakening SQLite assumptions.
- Schema version changes start with Stage 6 forward migrations. Pool/PRAGMA changes require an
  architecture amendment and crash/concurrency evidence; existing durable data is never silently
  downgraded.
