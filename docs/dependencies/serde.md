# `serde` dependency decision

- Package or tool: `serde` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Bootstrap configuration decoding.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Data-model deserialization and derive support for private,
  typed bootstrap configuration structures.
- Why the standard library or current approved dependencies are insufficient:
  Rust's standard library has no general data-model deserialization framework.
  Hand-writing a parallel visitor and derive system would add a large, security-sensitive
  parsing surface without improving Craxii's contracts.
- Permitted layer or scope: Bootstrap and adapter-local configuration representations
  only. Serde traits, attributes, and helper types must not enter domain or application
  APIs.
- Alternatives considered: Manual field decoding and a higher-level configuration
  framework. Manual decoding duplicates a mature primitive, while a configuration
  framework would own layering, source-merging, and defaults beyond V0 needs.
- Maintenance and security posture: `serde` is a narrowly scoped ecosystem primitive.
  Registry metadata on 2026-08-27 reported stable `1.0.229` with Rust version `1.56`;
  it is compatible with Rust `1.98.0`. Updates remain deliberate lockfile reviews.
- Exact license evidence: crates.io/Cargo package metadata declares
  `MIT OR Apache-2.0`; the packaged crate contains `LICENSE-MIT` and
  `LICENSE-APACHE`. The repository cargo-deny license check accepted the resolved
  crate without a clarification or exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved application graph on 2026-08-27. The repository
  advisory ignore list is empty.
- Unsafe, native, system, and build-script implications: `serde` `1.0.229` has a
  Rust build script that queries `rustc`, emits configuration flags, and writes a
  generated private module under Cargo's output directory; it invokes no native
  compiler or system library. The crate contains two reviewed `from_utf8_unchecked`
  uses in formatting/serialization internals. The enabled `derive` feature adds a
  proc-macro toolchain, but the resolved application graph has no package with a
  native `links` declaration.
- Secrets, parsing, and persistence implications: This dependency will decode the
  non-secret TOML configuration and logical credential references at the bootstrap
  trust boundary. It must never receive secret material for fingerprinting and has
  no persistence role. Unknown-key rejection and semantic validation remain
  Craxii-owned configuration behavior.
- Removal or migration cost: Moderate. Replace derives and deserialization adapters,
  preserve every accepted/rejected configuration fixture, and prove the public
  validated configuration contract unchanged.
- Approved version requirement: Compatible `1.0` line with default features and
  the `derive` feature enabled.
- Resolved and tested version: `1.0.229` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
