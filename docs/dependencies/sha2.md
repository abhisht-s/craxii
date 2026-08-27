# `sha2` dependency decision

- Package or tool: `sha2` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Bootstrap configuration compatibility metadata and canonical
  domain digests.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Stable SHA-256 hashing for the non-secret configuration
  fingerprint, canonical domain SHA-256 values, and later canonical content hashing,
  using the `Digest` trait and `Sha256` type.
- Why the standard library or current approved dependencies are insufficient:
  Rust's `DefaultHasher` is not a stable cross-version fingerprint contract, and the
  standard library has no SHA-256 implementation. A repository-owned cryptographic
  hash implementation would be inappropriate.
- Permitted layer and architecture boundary: Bootstrap configuration fingerprinting,
  canonical domain SHA-256 values, and later canonical content hashing. Hash-library
  traits and output types remain private implementation details, and the digest is
  not an authentication primitive.
- Alternatives considered: `DefaultHasher`, a hand-written SHA-256 implementation,
  and a broader crypto framework. The first is unstable by contract; the latter two
  add either unsafe maintenance risk or unnecessary surface.
- Maintenance and security posture: `0.11.0` was the latest stable release found by
  the 2026-08-27 audit, declares MSRV `1.85`, and passes the locked Rust `1.98.0`
  build. It is the current sensible line for a new project with no existing
  fingerprint implementation or migration constraint. Future updates remain
  deliberate lockfile, upstream-change, advisory, and license reviews.
- Exact license evidence: crates.io/Cargo package metadata declares
  `MIT OR Apache-2.0`; the packaged crate contains `LICENSE-MIT` and
  `LICENSE-APACHE`. The repository cargo-deny license check accepted the resolved
  crate without a clarification or exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` with
  `cargo-deny 0.20.2` reported `advisories ok` for the repaired application graph on
  2026-08-27. The repository advisory ignore list is empty.
- Feature choice and intended API: Default features are disabled. The default
  `alloc` and `oid` features are unnecessary for fixed SHA-256 hashing through
  `Digest` and `Sha256`, so neither is enabled; no additional `sha2` feature is
  enabled.
- Actual transitive graph: `sha2 0.11.0` resolves `cfg-if 1.0.4`,
  `cpufeatures 0.3.0`, and `digest 0.11.3`. Digest's required block API resolves
  `block-buffer 0.12.1` and `crypto-common 0.2.2`; both converge on
  `hybrid-array 0.4.14`, which resolves `typenum 1.20.1`. On relevant CPU/OS
  targets, including the current Apple ARM target, `cpufeatures` resolves
  `libc 0.2.189`. `generic-array` and `version_check` are absent.
- Build-script, native, and system implications: The direct crate declares
  `build = false`. In its resolved subtree, `libc` is the only package declaring a
  custom build target. No resolved package declares Cargo `links`, and no external
  native or separately installed system library is required. `cpufeatures` uses the
  platform C ABI through `libc` for CPU-feature detection on applicable targets;
  that target-specific FFI is distinct from an external library dependency.
- Unsafe and platform-intrinsics implications: The direct crate contains
  target-specific unsafe implementations and architecture intrinsics or assembly for
  accelerated SHA-2 on x86/x86_64, AArch64, RISC-V, LoongArch, and WASM, with CPU
  feature selection and software fallbacks. `cpufeatures` also contains unsafe CPU
  queries and target-specific OS FFI. Disabling default features does not remove
  these optimized backends, so the absence of a native system library must not be
  described as an absence of unsafe code.
- Secrets, parsing, and persistence implications: Canonical non-secret configuration,
  domain bytes, and later canonical content representations may be hashed. Loaded
  credential material must never enter the configuration fingerprint. SHA-256 values
  are evidence/content identity metadata, not password hashing, signing, or encryption;
  persistence representation remains owned by later adapters.
- Removal or migration cost: High once canonical digests are stored or exchanged.
  Migration requires an explicit algorithm/version transition plus configuration,
  digest, content-hash, and compatibility fixtures.
- Approved version requirement: Compatible `0.11` line with default features
  disabled and no additional features.
- Resolved and tested version: `0.11.0` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
