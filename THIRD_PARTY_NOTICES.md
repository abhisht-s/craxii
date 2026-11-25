# Third-party notices

Craxii's Rust dependencies are declared in `backend/Cargo.toml`, resolved in `Cargo.lock`, and reviewed by the repository's dependency policy and `cargo-deny` configuration. The Swift package currently has no external package dependencies.

The locked Rust graph includes software offered under permissive licenses such as Apache-2.0, MIT, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CDLA-Permissive-2.0, and compatible license alternatives. In particular, the URL and internationalized-domain dependency graph includes Unicode-3.0 data/code, and the TLS root-certificate graph includes Mozilla certificate data distributed through `webpki-root-certs` under CDLA-Permissive-2.0.

This source repository does not yet define a binary release bundle. Anyone distributing Craxii artifacts must inspect the exact resolved dependency graph for that artifact and preserve all applicable copyright notices, license texts, attribution notices, and upstream `NOTICE` material. `cargo deny --locked check licenses` verifies the repository's accepted SPDX policy but is not a replacement for assembling a release-specific notice bundle.

The project does not currently have evidence that an upstream Apache `NOTICE` file is incorporated into a distributed Craxii artifact, so no root `NOTICE` file is included. Recheck that conclusion whenever release packaging or the dependency graph changes.

Distribution contract validation adds MIT-licensed `jsonschema` and its locked
regex/Unicode/numeric dependencies. Canonical binary encoding declares the existing
MIT OR Apache-2.0 `base64` package directly. Local Ed25519 test vectors declare the
existing ISC/Apache-2.0/BSD-3-Clause `aws-lc-rs` graph as a development dependency;
its native code and packaged license texts remain part of the applicable artifact
review. No provider management SDK is introduced. The dependency records describe
the exact layers and offline resolver restrictions. Preserve upstream license and
notice text when creating a binary distribution; this source change creates none.
