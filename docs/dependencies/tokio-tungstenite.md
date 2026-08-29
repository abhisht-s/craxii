# `tokio-tungstenite` dependency decision

- Package: `tokio-tungstenite` from crates.io, direct dev dependency owned by Stage 11 real-socket
  protocol tests; it is not linked by production targets.
- Purpose: Connect test clients to real ephemeral loopback WebSocket listeners and observe frames,
  close codes, replay, backpressure, and shutdown.
- Declaration: `version = "0.29.0"`, default features disabled, exactly `connect` enabled.
- Resolution, MSRV, and license: `tokio-tungstenite 0.29.0`, Rust `1.63`, MIT; verified with Rust
  `1.98`.
- Feature/update policy: TLS/native-tls/rustls and URL convenience features are disabled. Updates
  inside `0.29` require lock review and the complete real WebSocket suite.
- Transitive notes: Uses `tungstenite 0.29.0`, HTTP, byte/encoding/SHA-1 support, and Tokio I/O.
  Test-only `connect` adds connection orchestration; no production WebSocket truth depends on it.
- Build/native/unsafe: No build script or native TLS/system library. Platform socket unsafe remains
  in Tokio/mio; pure-Rust tungstenite parses frames.
- Advisories: The locked cargo-deny advisory check includes dev dependencies and must pass without
  an ignore.
- Removal path: Replace with another independent real-socket client that can assert exact RFC close
  behavior without sharing the server adapter implementation.
- Review: Approved by the repository/project owner on 2026-08-28 for Stage 11 tests.
