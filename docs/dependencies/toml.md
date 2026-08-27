# `toml` dependency decision

- Package or tool: `toml` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Bootstrap non-secret configuration parser.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Standards-compliant TOML parsing and Serde-backed decoding for
  the versioned non-secret configuration file.
- Why the standard library or current approved dependencies are insufficient:
  Rust's standard library and Serde do not parse TOML syntax. A manual parser would
  be brittle at a startup trust boundary and would conflict with the requirement to
  reject malformed and unknown configuration deterministically.
- Permitted layer and architecture boundary: Bootstrap configuration parsing only.
  Raw TOML values and TOML-specific errors must not cross into domain or application
  contracts.
- Alternatives considered: Manual parsing, generic string maps, `config`, and
  `figment`. Manual parsing and string maps lose standards compliance and type
  ownership; larger configuration frameworks add source merging and policy that V0
  deliberately keeps in Craxii.
- Maintenance and security posture: `1.1.4+spec-1.1.0` was the latest stable release
  found by the 2026-08-27 audit, declares MSRV `1.85`, and passes the locked Rust
  `1.98.0` build. The compatible `1.1` line is preferred for a new project because it
  is the current stable line, typed configuration code does not yet exist, and there
  is no migration compatibility cost that would justify beginning on an older line.
  Future updates remain deliberate lockfile, upstream-change, advisory, and license
  reviews.
- Exact license evidence: crates.io/Cargo package metadata declares
  `MIT OR Apache-2.0`; the packaged crate contains `LICENSE-MIT` and
  `LICENSE-APACHE`. The repository cargo-deny license check accepted the resolved
  crate without a clarification or exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` with
  `cargo-deny 0.20.2` reported `advisories ok` for the repaired application graph on
  2026-08-27. The repository advisory ignore list is empty.
- Unsafe, native, system, and build-script implications: The direct crate declares
  `build = false` and `#![forbid(unsafe_code)]`, declares no Cargo `links` value, and
  requires no external system library. Its resolved subtree is `serde_core 1.0.229`,
  `serde_spanned 1.1.1`, `toml_datetime 1.1.1+spec-1.1.0`,
  `toml_parser 1.1.3+spec-1.1.0`, and `winnow 1.0.4`; the shared `serde_core`
  package declares a Rust build script. This direct-crate result is not a claim that
  every transitive crate in the complete application graph is free of unsafe code.
- Transitive implications: Disabling default features excludes display/serialization
  support and `toml_writer`. The `parse` feature brings in `toml_parser` and
  `winnow`; the `serde` feature brings in the Serde-backed date, span, value, and
  typed-decoding surface. Both TOML paths use the single resolved `winnow 1.0.4`, so
  the older `winnow 0.7` duplicate is absent.
- Secrets, parsing, and persistence implications: It parses an operator-controlled,
  non-secret file. File size limits, unknown-key rejection, typed validation,
  redacted errors, and the separation of logical credential references from secret
  values remain Craxii responsibilities. It performs no persistence.
- Intended API and feature choice: `toml::from_str` is sufficient because Craxii
  needs one in-memory TOML string decoded directly into an owned Serde type; it does
  not need document editing, formatting, source merging, or display support. The
  required features are exactly `parse` and `serde`, with default features disabled.
- Removal or migration cost: Effectively zero now because typed configuration and
  its fixtures have not been implemented. Once accepted parsing behavior and
  fingerprint fixtures exist, replacement would need to preserve rejection,
  redaction, versioning, and fingerprint contracts.
- Approved version requirement: Compatible `1.1` line, default features disabled,
  with exactly `parse` and `serde` enabled.
- Resolved and tested version: `1.1.4+spec-1.1.0` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
