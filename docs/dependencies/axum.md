# `axum` dependency decision

- Package: `axum` from crates.io, direct normal dependency owned by the HTTP/WebSocket adapter.
- Purpose: Typed protocol extraction, routing, JSON responses, HTTP/1 serving, and WebSocket
  upgrades over the existing Tokio runtime.
- Declaration: `version = "0.8.9"`, default features disabled, exactly `http1`, `json`,
  `matched-path`, `query`, `tokio`, `tracing`, and `ws` enabled.
- Resolution, MSRV, and license: `axum 0.8.9`, Rust `1.80`, MIT; verified with Rust `1.98`.
- Feature policy: Features are limited to Stage 11 HTTP/JSON/template tracing/query/WS behavior.
  HTTP/2, macros, form, multipart, and original-URI support are not enabled. Compatible patch/minor
  updates inside `0.8` require lockfile review, tests, cargo-deny, and protocol regression checks.
- Transitive notes: Brings reviewed `axum-core`, Hyper/HTTP implementation crates, `matchit
  0.8.4`, tungstenite support, body utilities, and serialization helpers. Hyper and HTTP remain
  transitive, not direct architectural dependencies. `matchit` declares `MIT AND BSD-3-Clause`;
  Stage 11 therefore explicitly admits the OSI-approved BSD-3-Clause license in `deny.toml`.
- Build/native/unsafe: The package declares no build script and requires no external native
  library. Its networking graph contains platform-specific unsafe in Tokio/mio/socket support.
- Advisories: The locked cargo-deny advisory check must pass without an ignore.
- Removal path: Replace only with an adapter preserving the exact protocol, middleware, upgrade,
  ownership, and shutdown contracts; application/domain types remain independent.
- Review: Approved by the repository/project owner on 2026-08-28 for Stage 11.
