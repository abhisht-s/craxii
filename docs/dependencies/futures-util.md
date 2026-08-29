# `futures-util` dependency decision

- Package: `futures-util` from crates.io, direct dev dependency owned by Stage 11 WebSocket tests.
- Purpose: Test-only `SinkExt`/`StreamExt` operations over tokio-tungstenite connections.
- Declaration: `version = "0.3.34"`, default features disabled, exactly `sink` and `std` enabled.
- Resolution, MSRV, and license: `futures-util 0.3.34`, Rust `1.71`, MIT OR Apache-2.0; verified
  with Rust `1.98`.
- Feature/update policy: Async-await macros, channels, compatibility, I/O, and write-all-vectored
  convenience are not selected directly. Compatible `0.3` updates require lock review and the real
  WebSocket suite.
- Transitive notes: Reuses futures-core/task/sink, pin-project-lite, slab, and memchr from the
  existing async dependency graph.
- Build/native/unsafe: No build script or native library; the package is portable Rust utility
  code.
- Advisories: The locked cargo-deny advisory check includes it and must pass without an ignore.
- Removal path: Remove with tokio-tungstenite if tests adopt equivalent stream/sink helpers.
- Review: Approved by the repository/project owner on 2026-08-28 for Stage 11 tests.
