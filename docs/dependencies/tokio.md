# `tokio` dependency decision

- Package or tool: `tokio` from crates.io.
- Dependency kind and owner: Direct normal dependency owned by bootstrap/runtime coordination and
  LocalWorkstation process supervision.
- Purpose: Main async runtime, SQLx runtime, the in-process write mutex, timeouts, owned tasks,
  composition-edge graceful-shutdown signals, and owned asynchronous child processes.
- Declaration: `version = "1.53"`, default features disabled, exactly `io-util`, `macros`, `net`,
  `process`, `rt-multi-thread`, `signal`, `sync`, and `time` enabled.
- Resolved version and MSRV: `tokio 1.53.1`, MSRV `1.71`; verified with Rust `1.98.0`.
- License and source: MIT, crates.io registry; accepted by cargo-deny.
- Feature rationale: macros supports the binary/test runtime; multi-thread runtime supports SQLx and
  owned tasks; signal supplies only composition-edge Ctrl-C/SIGTERM handling; sync supplies
  `Mutex`, `Notify`, `watch`, `mpsc`, `broadcast`, and `oneshot`; time supplies heartbeat, fallback
  scans, and bounded waits; `net` and `io-util` supply listener/socket and owned I/O
  primitives; `process` supplies child handles, pipes, waits, and kill-on-drop defense.
  Direct `full` and fs features remain disabled.
- Proc-macro/runtime graph: resolves `tokio-macros 2.7.2`, `pin-project-lite`, and the platform
  runtime dependencies activated transitively by SQLx. The signal feature additionally activates
  the already governed platform signal registry/libc path. The proc macro uses the standard
  proc-macro2/quote/syn build graph.
- Build/native/unsafe implications: Tokio has no native build script for the selected direct
  features; platform event/runtime support uses reviewed target-specific unsafe and libc/mio code
  when transitively active. Process support adds reviewed target-specific Unix/Windows child-process
  plumbing but no native build script or external system library.
- Advisories: the locked cargo-deny advisory check must pass with no ignore.
- Alternatives rejected: a second async runtime is incompatible with SQLx/runtime ownership;
  handwritten executor/mutex/timer code is not justified; synchronous blocking would complicate
  later network/process ownership.
- Removal/replacement path: A runtime replacement must first replace SQLx runtime integration and
  every owned-task/timer/synchronization seam without changing StateStore semantics.
- Review date and approval: 2026-08-29, expanded and explicitly approved by the
  repository/project owner for the current async runtime and process-execution use.
