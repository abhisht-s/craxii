# `time` dependency decision

- Package or tool: `time` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Clock adapter and bootstrap metadata formatting/conversion.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Checked UTC conversion, calendar access, and formatting for
  process/build operational timestamps.
- Why the standard library or current approved dependencies are insufficient:
  `SystemTime` represents an instant relative to an epoch but has no checked UTC
  calendar representation or RFC 3339 formatting support.
- Permitted layer or scope: Clock ports/adapters and bootstrap metadata/telemetry
  presentation. Stage 3 retains ownership of canonical durable timestamp types.
- Alternatives considered: Manual Gregorian calendar conversion and a broader date
  framework. The former is unnecessary correctness risk; the latter adds unused
  parsing, timezone, and localization surface.
- Feature choice: Default features are disabled; `std` and `formatting` are enabled.
  Parsing, macros, local-offset lookup, serde, randomness, and large dates are not.
- Maintenance and security posture: Only UTC is used. Duration and deadline logic
  remains on process-local monotonic time and never uses persisted wall differences.
- Exact license evidence: `time` declares `MIT OR Apache-2.0`; the repository
  cargo-deny license policy accepts the resolved crate without an exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved graph on 2026-08-27. The repository advisory
  ignore list is empty.
- Actual transitive graph: `time 0.3.55` resolves `deranged 0.5.8`, `num-conv
  0.2.2`, `powerfmt 0.2.0`, and `time-core 0.1.9`. `time-macros`, timezone lookup,
  libc, randomness, parsing, and serde support are absent from this feature subtree.
- Unsafe, native, system, and build-script implications: The direct crate and its
  enabled subtree declare no build script, native `links`, or external system
  library. No local-offset feature is enabled. `time`, `deranged`, and `powerfmt`
  contain reviewed internal unsafe range, numeric-formatting, and UTF-8 operations;
  Craxii uses checked constructors/conversions and adds no unsafe wrapper code.
- Secrets, parsing, and persistence implications: The crate receives timestamps
  only, parses no user or secret material, and introduces no persistence.
- Removal or migration cost: Moderate after timestamp evidence formats become
  operational contracts; replace conversion/formatting and retain boundary tests.
- Approved version requirement: Compatible `0.3` line with default features
  disabled and `std` plus `formatting` enabled.
- Resolved and tested version: `0.3.55` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
