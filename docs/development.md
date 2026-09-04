# Development

This guide covers the current public codebase. Craxii is pre-alpha; tests and implemented contracts are the authority when documentation and code disagree.

## Repository layout

| Path | Purpose |
| --- | --- |
| `backend/src/domain/` | Validated identities, content, lifecycle, journal, model, evidence, and runtime types. |
| `backend/src/application/` | Commands, authentication, scheduling, context, model/tool execution, publication, and runtime services. |
| `backend/src/ports/` | Interfaces for state, artifacts, providers, clocks, observations, and workstations. |
| `backend/src/adapters/` | SQLite, HTTP/WebSocket, provider, telemetry, artifact, and local-workstation implementations. |
| `backend/src/bootstrap/` | Configuration, credentials, startup composition, health, and process metadata. |
| `backend/tests/` | Cross-boundary integration, recovery, protocol, and optional live-provider tests. |
| `clients/macos/` | Native diagnostic app, Swift libraries, adapters, tests, and integration probe. |
| `docs/dependencies/` | Reviewed direct-dependency records. |
| `scripts/` | Public verification and environment-specific test entry points. |

## Toolchains

The Rust workspace uses edition 2024 with Rust `1.98.0`, selected by `rust-toolchain.toml`. Install `rustfmt` and `clippy` for that toolchain. The full dependency gate requires `cargo-deny 0.20.2`.

Repository Markdown uses Node 20 or newer and an exact `markdownlint-cli2` invocation. On the documented macOS development machine, Node and npm are under `/opt/homebrew/bin`; `scripts/verify` prepares that path.

The macOS client requires macOS 14 or newer, Swift language mode 6, and a compatible Xcode toolchain. Its Swift package has no external package dependencies.

## Focused Rust workflow

Run the narrowest useful check while editing:

```sh
cargo test --locked -p craxii-server <test-name>
cargo test --locked -p craxii-server --test <integration-test>
cargo check --locked --workspace --all-targets
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Tests marked `ignored` are explicit environment or capability checks, not part of a routine `cargo test` result. Invoke them only through their documented wrapper or exact test command.

## Swift and macOS workflow

The Swift package provides protocol, client-core, Apple-adapter, and integration-probe targets:

```sh
swift test --package-path clients/macos/CraxiiClient
swift build --package-path clients/macos/CraxiiClient
```

For the Xcode app plus the localhost backend-client integration:

```sh
scripts/verify-macos-client
```

This complete gate is macOS-only. Keep the package dependency list empty unless a new dependency has completed public review.

## Protocol fixtures

`backend/tests/fixtures/protocol-v1/` contains language-neutral request, response, snapshot, durable-event, draft-event, health, error, and sync examples. `manifest.sha256` fixes their byte-level inventory.

When protocol behavior changes deliberately:

1. update the Rust DTO/adapter behavior and behavioral tests;
2. update Swift decoding/projection behavior where affected;
3. update the public fixtures and deterministic manifest;
4. update `docs/protocol.md`; and
5. test forward/additive or version-breaking behavior explicitly.

Do not edit fixtures to hide a test failure. A protocol change requires a reviewed compatibility decision.

## Dependencies

Before adding a direct dependency, follow [the dependency policy](dependency-policy.md). Update the manifest, lockfile, machine-readable registry, and dependency record together. Review the resolved graph with:

```sh
cargo tree --locked --workspace --edges normal,build,dev
cargo deny --locked check advisories -D warnings
cargo deny --locked check licenses bans sources
```

## Debugging basics

- Start from `backend/tests/fixtures/config/valid/local.toml`; configuration errors intentionally collapse to safe diagnostics, so compare values against [Configuration](configuration.md) and the validation tests.
- Use `/health/live` and `/health/ready` to separate process liveness from application readiness.
- Treat local SQLite databases, WAL files, artifacts, traces, and provider responses as sensitive. Keep them outside Git and redact before sharing.
- Public HTTP errors include an `x-request-id` and a safe error code. Server traces contain request metadata but intentionally omit bodies and credentials.
- For reconnect bugs, record the bootstrap cursor, last applied durable cursor, `sync.complete` cursor, and event types—not message/model content or bearer tokens.

## Tracing and offline evidence

The `tracing.format` setting selects human-readable `pretty` output or newline-delimited `json`; both carry the same semantic span fields. The validated filters are `trace`, `debug`, `info`, `warn`, and `error`. The local fixture uses `pretty`/`info`, while service-shaped configuration uses JSON.

Tracing is disposable operational evidence. SQLite projections are current truth, journal entries are historical evidence, and neither startup recovery nor a state transition may rely on a trace being present. Trace spans correlate request, command, replay, work, model, tool, workstation, artifact, and storage activity using Craxii-generated identifiers. Missing measurements are left unavailable rather than reported as zero.

Stop the server before running the local evidence commands; they take the same exclusive state lock and open SQLite and artifacts read-only:

```sh
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml preflight
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml verify-state --format markdown
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml inspect-work <work-uuid>
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml inspect-runtime <runtime-uuid>
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml evidence-export --format json
```

JSON is the default; `--format markdown` embeds the same deterministic JSON document. Every artifact declares `craxii.operator-evidence/v1` and a read-only, noncanonical role. `verify-state` exits nonzero after printing its report if journal/projection or referenced-artifact checks fail. These commands never repair state and have no HTTP equivalent.

## Pull requests and full verification

Keep changes focused and describe the observable behavior, tests, platform impact, and any protocol/persistence/security/dependency consequences. See [CONTRIBUTING.md](../CONTRIBUTING.md).

After focused checks pass, run the consolidated gate once:

```sh
scripts/verify
```

The gate checks Rust formatting/build/lints/tests, release failpoint exclusion, dependency policy, public repository structure and links, Markdown, and the native client on macOS.
