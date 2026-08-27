# `cargo-deny` dependency decision

- Package or tool: `cargo-deny` from crates.io, installed with Cargo in user space.
- Dependency kind: Exact repository verification and supply-chain tool; not a Craxii
  runtime Cargo dependency.
- Owning subsystem: Repository dependency governance.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Locked-graph advisory, license, duplicate/bans, and dependency
  source enforcement.
- Why the standard library or current approved dependencies are insufficient:
  Cargo and the Rust standard library do not jointly evaluate RustSec advisories,
  SPDX license policy, duplicate versions, and registry/Git provenance as one
  mandatory gate.
- Permitted layer or scope: User-space repository verification through
  `scripts/verify` and `deny.toml` only. It is not declared in `backend/Cargo.toml`, is
  not shipped with the server, and must not influence application semantics.
- Alternatives considered: `cargo-audit`, `cargo-license`, custom lockfile parsing,
  and overlapping combinations. Separate tools leave policy integration gaps and
  duplicate work; custom parsing would recreate mature advisory and SPDX machinery.
- Maintenance and security posture: crates.io reported `0.20.2` as the latest stable
  release on 2026-08-27 with Rust version `1.88.0`, compatible with the unchanged
  project toolchain `1.98.0`. The executable is exact-pinned and installed with
  `cargo install cargo-deny --version 0.20.2 --locked`; version changes require human
  review of schema, policy behavior, and the tool's locked graph.
- Exact license evidence: crates.io/Cargo package metadata declares
  `MIT OR Apache-2.0`; the packaged crate contains `LICENSE-MIT` and
  `LICENSE-APACHE`. Running cargo-deny `0.20.2` against its packaged locked runtime
  graph with its included policy and `--exclude-dev` reported `licenses ok` and
  `sources ok` without ignored advisories.
- Advisory result: Running cargo-deny `0.20.2` against its packaged `Cargo.lock` with
  its included policy, `--locked`, `--exclude-dev`, and `-D warnings` reported
  `advisories ok` on 2026-08-27. Its packaged advisory ignore list is empty.
- Unsafe, native, system, and build-script implications: The direct tool declares
  `build = false` and contains unsafe code for memory mapping and validated internal
  byte representations. Its locked tool graph contains many Rust build scripts and
  compiled `ring` plus `zstd-sys`; installation therefore used the local C/assembly
  compiler toolchain for bundled native code. No sudo, Homebrew mutation, shell-profile
  edit, or separately installed system library was required.
- Secrets, parsing, and persistence implications: The tool parses Cargo manifests,
  lockfiles, license text, `deny.toml`, registry/index metadata, and advisory data. It
  must not receive application credentials. Its user Cargo caches and advisory/index
  cache are tooling state, not Craxii product persistence.
- Removal or migration cost: Moderate. A replacement must preserve the exact installed
  version check, explicit network/advisory failure behavior, SPDX license policy,
  duplicate inspection, registry-only provenance, and full `scripts/verify` gate.
- Approved version requirement: Exact `=0.20.2` repository-tool version.
- Resolved and tested version: `cargo-deny 0.20.2`, installed at the existing user
  Cargo binary location and executed with Rust `1.98.0`.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
