# `tower-http` dependency decision

- Package: `tower-http` from crates.io, direct normal dependency owned by the HTTP adapter.
- Purpose: Sensitive authorization marking, fixed response headers, timeouts, limit primitives,
  and safe request tracing. Axum's typed body limiter performs the route-specific predecode caps
  so 413 responses retain the single versioned public error envelope.
- Declaration: `version = "0.7.0"`, default features disabled, exactly `limit`,
  `sensitive-headers`, `set-header`, `timeout`, and `trace` enabled.
- Resolution, MSRV, and license: `tower-http 0.7.0`, Rust `1.65`, MIT; verified with Rust `1.98`.
- Feature/update policy: CORS, auth, catch-panic, compression, filesystem, request-id generation,
  normalize-path, metrics, and redirect modules are disabled. Compatible updates inside `0.7`
  require lock review, header/redaction tests, and full transport regression.
- Transitive notes: Reuses HTTP/body, Tower, tracing, bytes, and pin-project-lite. It adds no TLS
  implementation and does not make forwarded headers trusted.
- Build/native/unsafe: No build script or native library; selected layers are pure Rust.
- Advisories: The locked cargo-deny advisory check must pass without an ignore.
- Removal path: A replacement must preserve predecode bounds, exact timeout classes, sensitive
  headers, fixed security headers, and matched-route-only traces.
- Review: Approved by the repository/project owner on 2026-08-28 for Stage 11.
