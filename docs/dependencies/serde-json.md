# `serde_json` dependency decision

- Package or tool: `serde_json` from crates.io.
- Dependency kind: Direct development-only Cargo dependency.
- Owning subsystem: Redaction, telemetry, and startup tests.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Structural JSON parsing and serialization assertions in tests.
- Why the standard library or current approved dependencies are insufficient:
  the standard library has no JSON parser, and substring checks cannot prove the
  structured shape or semantic parity of newline-delimited trace records.
- Permitted layer or scope: Tests only. Production JSON formatting is supplied
  transitively inside adapter-owned `tracing-subscriber` functionality.
- Alternatives considered: Ad hoc JSON parsing and snapshot-test frameworks. The
  first is fragile and the second is substantially broader than the focused gates.
- Feature choice: Default features are enabled; no additional features are enabled.
- Maintenance and security posture: Tests parse only test-owned redacted output and
  fixtures. No production secret, configuration, provider, or artifact input is
  accepted through this dev dependency.
- Exact license evidence: `serde_json` declares `MIT OR Apache-2.0`; the repository
  cargo-deny license policy accepts the resolved crate without an exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved development graph on 2026-08-27. The repository
  advisory ignore list is empty.
- Actual transitive graph: `serde_json 1.0.151` resolves `itoa 1.0.18`, `memchr
  2.8.3`, `serde_core 1.0.229`, and `zmij 1.0.23`. The same exact package is already
  lockfile-converged as a production transitive dependency of JSON trace formatting;
  Craxii's direct declaration remains development-only.
- Unsafe, native, system, and build-script implications: `serde_json` uses internal
  unsafe UTF-8, pointer-offset, and serialization fast paths; `memchr` and `zmij`
  use reviewed unsafe optimized search/numeric formatting. `serde_json` has a build
  script selecting arithmetic width from Cargo target variables, and `zmij` invokes
  `rustc --version` to select supported intrinsics. Neither invokes a native compiler
  or system library, and no resolved package declares Cargo `links`.
- Secrets, parsing, and persistence implications: Parsing is confined to generated
  test trace output; there is no runtime, credential, or persistence role.
- Removal or migration cost: Low; replace structural assertions with an equivalent
  test-only parser.
- Approved version requirement: Compatible `1.0` line with default features and no
  additional features.
- Resolved and tested version: `1.0.151` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
