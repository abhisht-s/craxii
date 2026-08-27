# `serde_json` dependency decision

- Package or tool: `serde_json` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: SQLite persistence adapters plus redaction, telemetry, and startup tests.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Strict versioned JSON encoding/decoding for adapter-private durable row
  codecs, plus structural JSON assertions in tests.
- Why the standard library or current approved dependencies are insufficient:
  the standard library has no JSON parser, and substring checks cannot prove the
  structured shape, reject unknown fields, or reconstruct the exact V1 message-content DTO.
- Permitted layer or scope: Production use is confined to `adapters/sqlite` persistence DTOs and
  existing adapter-owned tracing functionality. It is not a domain dependency or a public wire
  contract.
- Alternatives considered: Ad hoc JSON parsing and snapshot-test frameworks. The
  first is fragile and the second is substantially broader than the focused gates.
- Feature choice: Default features are enabled; no additional features are enabled.
- Maintenance and security posture: Production codecs parse SQLite text after SQL `json_valid`
  checks and then validate strict private DTOs with unknown fields denied. No raw
  provider/process/path/internal-detail material is admitted to terminal-detail persistence.
- Exact license evidence: `serde_json` declares `MIT OR Apache-2.0`; the repository
  cargo-deny license policy accepts the resolved crate without an exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved production graph on 2026-08-28. The repository
  advisory ignore list is empty.
- MSRV: The resolved crate declares Rust 1.71; Craxii requires Rust 1.98.
- Actual graph impact: `serde_json 1.0.151` resolves `itoa 1.0.18`, `memchr 2.8.3`,
  `serde_core 1.0.229`, and `zmij 1.0.23`. The same exact package was already lockfile-converged in
  the production graph through JSON trace formatting, so promotion adds no package or duplicate.
- Unsafe, native, system, and build-script implications: `serde_json` uses internal
  unsafe UTF-8, pointer-offset, and serialization fast paths; `memchr` and `zmij`
  use reviewed unsafe optimized search/numeric formatting. `serde_json` has a build
  script selecting arithmetic width from Cargo target variables, and `zmij` invokes
  `rustc --version` to select supported intrinsics. Neither invokes a native compiler
  or system library, and no resolved package declares Cargo `links`.
- Secrets, parsing, and persistence implications: The production role is durable adapter-only JSON
  for message content and safe terminal detail. `InternalDetail`, raw credentials, provider wire
  objects, SQL text, and physical paths are excluded. SQLite's built-in `json_valid` is sufficient
  for SQL shape gating; no SQLite JSON extension or extra SQLx feature is required.
- Domain and wire boundary: Domain types remain storage-neutral and do not import `serde_json`.
  Public protocol JSON remains owned by its later protocol stage; storage DTO versioning does not
  become a public wire dependency.
- Removal or migration cost: Moderate for persistence codecs; replace the private JSON DTO codec
  while preserving exact stored V1 bytes and corruption rejection.
- Approved version requirement: Compatible `1.0` line with default features and no
  additional features.
- Resolved and tested version: `1.0.151` from crates.io.
- Review date: 2026-08-28.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-28. Codex recorded the decision and is not the approver.
