# `tower` dependency decision

- Package: `tower` from crates.io, direct normal dependency owned by the HTTP adapter.
- Purpose: Bounded concurrency and service composition utilities without a second durable queue.
- Declaration: `version = "0.5.3"`, default features disabled, exactly `limit` and `util` enabled.
- Resolution, MSRV, and license: `tower 0.5.3`, Rust `1.64.0`, MIT; verified with Rust `1.98`.
- Feature/update policy: Buffer, retry, load-shed, discovery, balance, filter, and ready-cache are
  deliberately disabled. Compatible updates inside `0.5` require lock review and all HTTP,
  timeout, overload, and shutdown tests.
- Transitive notes: Uses `tower-layer`, `tower-service`, futures-core/util, pin-project-lite, and
  sync primitives already present or reviewed in the HTTP adapter graph.
- Build/native/unsafe: No build script or native library. Selected modules are Rust service-layer
  code; platform unsafe remains confined to the async runtime graph.
- Advisories: The locked cargo-deny advisory check must pass without an ignore.
- Removal path: A replacement must preserve the exact 64/16 request bounds and owned service
  behavior while leaving application ports unchanged.
- Review: Approved by the repository/project owner on 2026-08-28 for the HTTP adapter.
