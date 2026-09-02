# `reqwest` dependency decision

- Package: `reqwest` from crates.io, direct normal dependency owned by the OpenAI outward adapter.
- Purpose: authenticated HTTP/1.1 Responses API requests and incremental SSE body reads.
- Declaration: `version = "0.13.4"`, default features disabled, exactly `rustls` enabled.
- Resolution/MSRV/license: `reqwest 0.13.4`, Rust 1.85, MIT OR Apache-2.0; compatible with Craxii's Rust 1.98 policy as reviewed on 2026-09-02. The selected Rustls graph additionally uses the permissive ISC and MIT-0 software licenses and CDLA-Permissive-2.0 for the public certificate-root dataset. Those three licenses are admitted explicitly in `deny.toml`; no per-crate exception or confidence override is used.
- Feature policy: no SDK, JSON feature, cookies, redirects, proxy discovery, compression, multipart, SOCKS, HTTP/2, or HTTP/3. Existing `serde_json` performs bounded wire serialization.
- Transport policy: HTTP/1.1, no redirects, `no_proxy`, `retry(never)`, no verbose connection logging, explicit connect/idle/absolute deadlines, and HTTPS-only production endpoints. Local HTTP is accepted only in adapter unit tests.
- Transitive notes: the locked Rustls/Hyper graph includes platform certificate verification and `aws-lc-rs`/`aws-lc-sys`; those remain adapter-internal transitive implementation details. The Rustls feature is retained over native TLS so production Linux does not acquire an OpenSSL system dependency.
- Secret boundary: `SecretString` enters only a bearer header. Authorization, bodies, provider messages, tool data, error bodies, and opaque continuation bytes are never formatted or traced.
- Removal path: replace the outward transport while retaining the provider-neutral `ModelProvider` request, stream, error, and certainty contracts.
