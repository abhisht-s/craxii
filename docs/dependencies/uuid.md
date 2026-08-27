# `uuid` dependency decision

- Package or tool: `uuid` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Canonical domain identity values.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: UUID parsing/formatting and production UUIDv7 generation for
  distinct durable/public domain identifiers.
- Why the standard library or current approved dependencies are insufficient: The
  Rust standard library has no UUID representation, RFC variant/version validation,
  or operating-system-backed UUIDv7 generation. A repository-owned implementation
  would add security-sensitive randomness, bit-layout, and parser maintenance.
- Permitted layer or scope: Private storage and validation inside canonical domain ID
  newtypes. The underlying `Uuid`, unchecked constructors, UUID ordering, provider
  IDs, PIDs, paths, and crate Serde implementation must not enter public domain or
  application contracts.
- Alternatives considered: Hand-written UUID parsing/generation, opaque strings, and
  random UUIDv4. Hand-written UUID code duplicates a mature primitive; opaque strings
  weaken canonical/version boundaries; UUIDv4 contradicts the frozen UUIDv7 contract.
- Maintenance and security posture: Resolved `uuid 1.26.0` declares MSRV `1.85.0`
  and builds under Rust `1.98.0`. Lockfile updates remain deliberate upstream,
  advisory, license, feature, platform, randomness, and unsafe-surface reviews.
- Exact license evidence: crates.io/Cargo package metadata declares
  `Apache-2.0 OR MIT`; the packaged crate contains `LICENSE-APACHE` and
  `LICENSE-MIT`. The repository cargo-deny policy accepts both licenses without an
  exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved graph on 2026-08-27. The repository advisory
  ignore list is empty.
- Feature choice and intended API: Default features and the crate's `serde` feature
  are disabled. Exactly `std` and `v7` are enabled. `v7` activates `rng` and
  target-appropriate `getrandom`; Craxii manually implements strict canonical string
  Serde and uses UUID time/lexical structure for inspection only, never ordering.
- Actual transitive graph: On the current Apple ARM target and the supported Ubuntu
  target, `uuid 1.26.0` resolves `getrandom 0.4.3`, `cfg-if 1.0.4`, and
  `libc 0.2.189`. The all-target resolver also records `r-efi 6.0.0` for an opt-in
  UEFI entropy backend. The lockfile additionally contains target/weak-feature
  WebAssembly resolution packages (`wasm-bindgen 0.2.127`, `js-sys 0.3.104`, and
  their proc-macro/support graph), but none is active in the approved native feature
  graph.
- Build-script, native, and system implications: `uuid` declares no build script.
  `getrandom` has a Rust-only build script that detects memory-sanitizer cfg and does
  not invoke a native compiler. `libc`, platform APIs, and operating-system entropy
  facilities are used where target-appropriate; no separately installed native or
  system library is required. The active native `uuid` subtree declares no Cargo
  `links`; lock-only `wasm-bindgen-shared` declares `links = "wasm_bindgen"` for its
  inactive WebAssembly graph.
- Unsafe and platform implications: `uuid` contains reviewed internal unsafe byte
  formatting/transmutation operations. `getrandom` contains target-specific unsafe
  FFI, syscall/intrinsic, and initialized-buffer operations and obtains entropy from
  the host operating system (for example, `getentropy` on macOS and `getrandom` with
  documented fallback behavior on Linux). Craxii adds no unsafe wrapper code and
  treats generation uniqueness as probabilistic, not ordering or causality proof.
- Secrets, parsing, and persistence implications: IDs are non-secret. Untrusted text
  is accepted only through strict lowercase 36-character hyphenated UUIDv7 parsing;
  arbitrary UUID spellings are rejected. The dependency performs no persistence;
  later adapters own storage codecs.
- Removal or migration cost: High after IDs become durable/public. Replacement must
  preserve exact canonical parsing, formatting, UUIDv7 generation, all type-safety
  boundaries, and stored/protocol fixtures without changing identity values.
- Approved version requirement: Compatible `1.26` line with default features
  disabled and exactly `std` plus `v7` enabled.
- Resolved and tested version: `1.26.0` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
