# `tokio` dependency decision

- Package or tool: `tokio` from crates.io.
- Dependency kind and owner: Direct normal dependency owned by bootstrap/runtime coordination.
- Purpose: Main async runtime, SQLx runtime, the in-process write mutex, timeouts, and async tests.
- Declaration: `version = "1.53"`, default features disabled, exactly `macros`,
  `rt-multi-thread`, `sync`, and `time` enabled.
- Resolved version and MSRV: `tokio 1.53.1`, MSRV `1.71`; verified with Rust `1.98.0`.
- License and source: MIT, crates.io registry; accepted by cargo-deny.
- Feature rationale: macros supports the binary/test runtime; multi-thread runtime supports SQLx and
  later owned tasks; sync supplies `Mutex`; time supplies bounded waits. Direct `full`, fs, process,
  signal, and explicit networking features are not enabled for convenience.
- Proc-macro/runtime graph: resolves `tokio-macros 2.7.2`, `pin-project-lite`, and the platform
  runtime dependencies activated transitively by SQLx. The proc macro uses the standard
  proc-macro2/quote/syn build graph.
- Build/native/unsafe implications: Tokio has no native build script for the selected direct
  features; platform event/runtime support uses reviewed target-specific unsafe and libc/mio code
  when transitively active. No external system library is required.
- Advisories: the locked cargo-deny advisory check must pass with no ignore.
- Alternatives rejected: a second async runtime is incompatible with SQLx/runtime ownership;
  handwritten executor/mutex/timer code is not justified; synchronous blocking would complicate
  later network/process ownership.
- Removal/replacement path: A runtime replacement must first replace SQLx runtime integration and
  every owned-task/timer/synchronization seam without changing StateStore semantics.
- Review date and approval: 2026-08-27, approved by the repository/project owner.
