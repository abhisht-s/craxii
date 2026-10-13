# `jsonschema` dependency decision

- Package: crates.io `jsonschema`, compatible requirement `0.58.6`, resolved0.58.6.
- Kind/layer: normal; offline distribution syntax validation, plus test conformance.
- Owner: project owner; engineering selection under explicit autonomous
  implementation authorization,2026-10-13. Independent verification pending.
- Need/alternatives: serde checks closed typed DTOs but does not enforce every JSON
  Schema bound/union/format. A handwritten validator would duplicate schema authority
  and become a security-sensitive framework. Use the authoritative bundled schemas
  in the codec as well as independently registered schemas in tests.
- Features: default-features=false; no optional features. HTTP/file/async resolution
  disabled; `.offline()` additionally denies every unregistered reference. A fixed
  in-memory registry contains trusted compile-time embedded Draft-7 schemas only.
- Source/maintenance/license: upstream Stranger6667/jsonschema rust-v0.58.6 release,
  packaged Rust MSRV1.85, MIT license and checksum-locked crates.io source. Project
  Rust1.98 compiles it. Dependencies and checksums are inspected with locked metadata,
  cargo tree and the mandatory cargo-deny gate; no advisory waiver is introduced.
- Native/unsafe: direct package forbids unsafe code; Unicode/regex/SIMD, numeric and
  registry dependencies add reviewed parsing/native-intrinsic surface. Native TLS
  remains inherited through existing application dependencies, not HTTP resolution.
- Security/performance: raw UTF-8/duplicate-key/lexical/depth/byte checks run first.
  Closed schemas and custom scalar formats run after bounded canonicalization.
  Validators cache once; no user-selected schema, network, provider or secret input.
  Syntax validation never authenticates or authorizes.
- Removal/cost: removing runtime schema validation requires proving equivalent closed
  DTO/range/bounds/union behavior against the same full conformance corpus. It cannot
  silently broaden acceptance. Existing native services never call this module.
- Evidence: focused locked schema/DTO tests and explicit HTTP/file ref denial pass;
  full dependency/license/source/advisory and repository gates required before handoff.
