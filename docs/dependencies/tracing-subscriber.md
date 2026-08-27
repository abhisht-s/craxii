# `tracing-subscriber` dependency decision

- Package or tool: `tracing-subscriber` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Adapter-owned telemetry initialization and formatting.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: Pretty and newline-delimited JSON event formatting, UTC event
  timestamps, closed level filtering, and global/scoped subscriber installation.
- Why the standard library or current approved dependencies are insufficient:
  `tracing` defines events but intentionally does not provide a formatting or
  collection implementation.
- Permitted layer or scope: `adapters::telemetry` only. Subscriber, formatter, and
  writer types must not enter domain or application contracts.
- Alternatives considered: A repository-owned subscriber and broader telemetry
  stacks. The former recreates subtle formatting/dispatch behavior; the latter add
  collectors, protocols, runtimes, and shipping outside V0 scope.
- Feature choice: Default features are disabled; `fmt`, `ansi`, `json`, and `time`
  are enabled. `env-filter`, log compatibility, local time, regex, OpenTelemetry,
  appender, and metrics features are not enabled.
- Maintenance and security posture: Format and filter inputs come only from
  `ValidatedConfig`; no `RUST_LOG` or arbitrary directive parser is accepted.
  Writer failure and subscriber conflict are typed fatal initialization outcomes.
- Exact license evidence: `tracing-subscriber` declares `MIT`; the repository
  cargo-deny license policy accepts the resolved crate without an exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved graph on 2026-08-27. The repository advisory
  ignore list is empty.
- Actual transitive graph: `tracing-subscriber 0.3.23` resolves `nu-ansi-term
  0.50.3`, `serde 1.0.229`, `serde_json 1.0.151`, `sharded-slab 0.1.7`,
  `thread_local 1.1.10`, `time 0.3.55`, `tracing-core 0.1.36`, and
  `tracing-serde 0.2.0`. Their small supporting graph adds `cfg-if 1.0.4`,
  `itoa 1.0.18`, `lazy_static 1.5.0`, `memchr 2.8.3`, `once_cell 1.21.4`, and
  `zmij 1.0.23`; shared Serde/time subtrees are lockfile-converged.
- Unsafe, native, system, and build-script implications: The direct crate declares
  no build script or native `links`. Its layer-filter/downcast implementation and
  the sharded/thread-local/JSON/formatting subtrees contain reviewed internal unsafe
  synchronization, pointer, UTF-8, and numeric-formatting code. `serde_json` has a
  target-arithmetic configuration build script and `zmij` invokes `rustc --version`
  to select supported intrinsics; neither invokes a native compiler or links a
  system library. No package in the resolved graph declares Cargo `links`.
- Secrets, parsing, and persistence implications: JSON serialization is restricted
  to an explicit safe event schema. Traces are noncanonical operational evidence
  and no reader is exposed as a journal or recovery port.
- Removal or migration cost: Moderate; preserve pretty/JSON semantic parity,
  filtering, global-initialization, failure, and redaction behavior.
- Approved version requirement: Compatible `0.3` line with default features
  disabled and `fmt`, `ansi`, `json`, and `time` enabled.
- Resolved and tested version: `0.3.23` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
