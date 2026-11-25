# `aws-lc-rs` dependency decision

- Package: crates.io `aws-lc-rs`, compatible requirement `1.18.1`, resolved1.18.1.
- Kind/layer: runtime dependency; worker Ed25519 key generation/signing and local verification goldens.
- Owner: project owner; engineering selection under explicit autonomous
  implementation authorization,2026-10-13. Independent verification pending.
- Need/alternatives: standard Rust has no Ed25519 implementation. Reuse the existing
  locked TLS crypto library; avoid another crypto dependency or handwritten crypto.
- Source/maintenance: checksum-locked packaged upstream source and existing TLS
  dependency; inspect the resolved graph and advisory data at each update.
- License/native/unsafe: ISC and Apache-2.0/BSD-3-Clause upstream components;
  aws-lc-sys0.45.0 uses C/assembly/build tooling and unsafe FFI. Default features
  select alloc/prebuilt-nasm/non-FIPS behavior; inspect Cargo's unified graph.
  Building uses native compiler/CMake as supported by upstream. Existing TLS already
  builds the native library; adding the public signing API also resolves untrusted0.7.1.
- Network/security: this is a local cryptography package, with no infrastructure
  management API, account access, provider call or credential discovery. The public
  RFC8032 seed is test-only and cannot enter deployment trust/configuration.
- Parsing/persistence:32-byte keys and64-byte signatures; exact purpose-separated
  message bytes, no prehash. Worker keys use OS-backed SystemRandom and PKCS8 in protected service storage.
- Removal: signing implementation can change only while preserving standard and protocol
  signature vectors; worker key recovery must preserve the enrolled public identity.
- Evidence: focused tests independently sign/verify all five purposes and reject
  mutated context/input; consolidated supply-chain checks required before handoff.
