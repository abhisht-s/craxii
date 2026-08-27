# `tracing` dependency decision

- Package or tool: `tracing` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Bootstrap operational diagnostics and adapter telemetry.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Structured spans and events with typed levels and fields.
- Why the standard library or current approved dependencies are insufficient:
  the standard library has no structured diagnostic event API or subscriber seam.
- Permitted layer or scope: Bootstrap, application instrumentation, ports-neutral
  call sites, and adapters. Tracing events are operational evidence only and must
  never become product journal or recovery input.
- Alternatives considered: Repository-owned logging macros and the `log` facade.
  Neither supplies the required structured span/event model without recreating a
  substantial ecosystem primitive.
- Feature choice: Default features are disabled; only `std` is enabled. Attribute
  macros, log compatibility, and compile-time level caps are not enabled.
- Maintenance and security posture: The approved `0.1` line is mature and narrowly
  scoped. Fields remain Craxii-owned closed schemas and must exclude secrets,
  content, commands, headers, configuration bodies, and provider payloads.
- Exact license evidence: `tracing` declares `MIT`; the repository cargo-deny
  license policy accepts the resolved crate without an exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved graph on 2026-08-27. The repository advisory
  ignore list is empty.
- Actual transitive graph: `tracing 0.1.44` resolves `pin-project-lite 0.2.17` and
  `tracing-core 0.1.36`; `tracing-core` resolves `once_cell 1.21.4`. The disabled
  attributes and log features add no proc macro or log facade.
- Unsafe, native, system, and build-script implications: The direct crate and this
  subtree declare no build script, native `links`, or external system-library
  requirement. `tracing` uses reviewed unsafe pin projections and guard handling;
  `tracing-core` and `once_cell` use unsafe synchronization, dispatch, callsite, and
  initialization internals. Craxii adds no unsafe code around these APIs.
- Secrets, parsing, and persistence implications: Event construction receives only
  explicitly selected safe metadata. The crate owns no secret loading, parsing, or
  persistence, and trace loss must not change recovery behavior.
- Removal or migration cost: Moderate once stable subsystem/event fields are used by
  operations; replace instrumentation and preserve field/redaction tests.
- Approved version requirement: Compatible `0.1` line with default features
  disabled and only `std` enabled.
- Resolved and tested version: `0.1.44` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
