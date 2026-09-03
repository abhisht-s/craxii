# `sqlx` dependency decision

- Package or tool: `sqlx` from crates.io.
- Dependency kind and owner: Direct normal dependency owned by the SQLite persistence adapter.
- Purpose: Async SQLite connections, bounded pool, fixed queries, and embedded forward migration
  metadata.
- Declaration: `version = "0.9"`, default features disabled, exactly `sqlite-bundled`,
  `runtime-tokio`, `migrate`, and `macros` enabled.
- Resolved version and MSRV: `sqlx 0.9.0`, MSRV `1.94.0`; verified with repository Rust `1.98.0`.
- License and source: `MIT OR Apache-2.0`, crates.io registry. Cargo-deny policy accepts both.
- Transitive license implication: `hashbrown` selects `foldhash 0.2.0`, licensed under the
  OSI-approved Zlib license. The repository explicitly admits Zlib in the cargo-deny allowlist; there is no
  per-crate license exception or confidence override.
- Feature rationale: bundled SQLite removes a system-library dependency; Tokio supplies the chosen
  runtime; `migrate` embeds the checked-in migration set; `macros` supplies `migrate!`. The
  top-level `sqlx/any`, JSON, time/UUID codecs, TLS, load extension, deserialize, regexp, and
  unlock-notify features remain disabled. SQLx 0.9's proc-macro crate unconditionally compiles an
  internal `sqlx-core/any` feature for macro expansion; no Any driver or MySQL/PostgreSQL package is
  active in Craxii's normal/runtime graph.
- Native/build implications: resolves `sqlx-core/sqlx-sqlite/sqlx-macros 0.9.0` and
  `libsqlite3-sys 0.37.0`; bundled mode invokes `cc 1.4.4` for SQLite `3.51.3` and uses packaged
  bindings. No build-time bindgen, system SQLite, OpenSSL, TLS, or SQLCipher is active.
- Unsafe implications: SQLite is C/FFI code and SQLx's SQLite worker crosses reviewed bindings.
  Craxii adds no unsafe wrapper and confines SQLx to `adapters/sqlite`.
- Graph note: SQLx runtime support transitively enables the Tokio facilities its connection/pool
  implementation needs. Cargo.lock records optional MySQL/PostgreSQL packages for SQLx's published
  feature universe, but neither package has a reverse dependency in the selected normal/build/dev
  graph. `pkg-config` and `vcpkg` are present as `libsqlite3-sys` build dependencies, but the active
  bundled branch compiles the packaged amalgamation and does not select system SQLite.
- Advisories: `cargo deny --locked check advisories -D warnings` must pass on the resolved graph;
  no ignore is approved.
- Alternatives rejected: rusqlite conflicts with the selected async adapter; system SQLite makes
  deployments host-dependent; raw FFI expands unsafe ownership; a hand-rolled migration runner
  duplicates SQLx metadata semantics.
- Removal/replacement path: Replace only behind the dependency-neutral StateStore facade while
  preserving migration history, file/WAL behavior, error normalization, and crash tests.
- Review date and approval: 2026-08-27, approved by the repository/project owner.
