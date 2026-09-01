# `reqwest` dependency decision

- Package: `reqwest` from crates.io, direct normal dependency owned only by the OpenAI outward
  adapter.
- Purpose: bounded authenticated HTTP/1.1 requests and incremental response-body reads for the
  OpenAI Responses API SSE transport.
- Declaration: `version = "0.13.4"`, default features disabled, exactly `json` and `rustls`
  enabled.
- Resolution, MSRV, and license: `reqwest 0.13.4`, Rust `1.85`, MIT OR Apache-2.0; verified with
  Rust `1.98` on 2026-09-01.
- Feature policy: default TLS, charset, HTTP/2, system-proxy, compression, cookies, multipart,
  SOCKS, and HTTP/3 features remain disabled. `rustls` supplies certificate verification and
  `json` supplies request/fixture serialization. The adapter explicitly selects HTTP/1.1,
  disallows redirects, applies connect/idle/absolute limits, and implements no transport retry.
- Transitive notes: the locked Rustls graph includes Hyper transport crates, platform certificate
  verification, `aws-lc-rs`/`aws-lc-sys`, and their build helpers. These remain transitive; no
  provider wire or transport type crosses the adapter boundary.
- Build/native/unsafe: `aws-lc-sys` has a CMake/C compiler build and native cryptographic code.
  Platform networking/TLS crates contain reviewed unsafe internals; Craxii adds no unsafe provider
  transport code.
- Secret boundary: the API key enters only a sensitive Authorization header from `SecretString`.
  Request bodies, response bodies, headers, provider messages, and keys are neither logged nor
  formatted; only canonical hashes and bounded classified evidence leave the adapter.
- Advisories: the locked cargo-deny advisory and license checks must pass without a Reqwest-specific
  ignore.
- Removal path: replace the outward adapter transport while retaining the canonical
  `ModelProvider` request/stream/error contract and local fixture suite.
- Review: approved by the repository/project owner on 2026-09-01 for Stage 19.
