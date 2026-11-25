# `base64` dependency decision

- Package: crates.io `base64`, compatible requirement `0.22.1`, resolved0.22.1.
- Kind/layer: normal; distribution primitives only, canonical unpadded URL-safe
  encoding with strict decoded length and decode/re-encode equality.
- Owner: project owner; engineering selection under explicit autonomous
  implementation authorization,2026-10-13. Independent verification pending.
- Need/alternatives: Rust standard library has no base64 codec. Reuse the existing
  locked TLS transitive package rather than maintain a handwritten decoder or a
  broader crypto framework. No new resolved version is introduced.
- Maintenance/source/license: packaged crates.io source and Cargo.lock checksum;
  upstream `base64` project; MIT OR Apache-2.0 with bundled license texts. Deliberate
  compatible lock updates remain required; version pinning is not a security proof.
- Features/native/unsafe: default std feature only, no build script or system
  dependency; packaged source prohibits unsafe code. No network or credentials
  discovery. Encoded session material remains sensitive and lacks Debug exposure.
- Parsing/persistence: alternate alphabets, padding, trailing unused bits and wrong
  lengths reject. This supplies encoding, never authentication or storage trust.
- Removal: changing wire encoding is breaking and requires preserved golden vectors.
- Evidence: locked focused Rust tests validate normal and adversarial binary values;
  consolidated cargo-deny and repository policy checks are required before handoff.
