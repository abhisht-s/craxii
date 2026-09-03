# Craxii

Craxii is an open-source agent-runtime harness that gives frontier AI models durable state and controlled access to a persistent workstation.

> **Status: pre-alpha.** Craxii is under active development, has no stable release, and is not production-ready.

## What Craxii is

Craxii runs an authenticated agent backend beside a workstation. It turns user messages into durable work, assembles model context, mediates model and tool execution, and records canonical outcomes so a client can reconnect and recover state.

The project exists to make long-running model work inspectable and recoverable without giving a model an unbounded or implicit execution channel. State transitions, model attempts, tool attempts, and public events have explicit boundaries.

## Implemented today

- A Rust backend with validated TOML configuration, SQLite-backed durable state, migrations, journaling, projections, and startup recovery.
- Device bearer authentication, idempotent message and cancellation commands, health endpoints, bootstrap snapshots, and protocol-v1 error envelopes.
- Durable WebSocket replay from a cursor, live durable events, lossy draft events, and an explicit `sync.complete` boundary.
- A scheduler and bounded agent loop with context assembly, model selection, retry classification, and conservative ambiguous-outcome handling.
- An OpenAI Responses API adapter plus a deterministic scripted provider used by tests.
- Explicit `read_file` and foreground `run_shell` tools through a local-workstation boundary, including output limits, timeouts, artifact capture, cancellation, and optional administrative capability on supported Linux hosts.
- A native macOS 14+ diagnostic client with Keychain-backed bearer credentials, bootstrap/replay, reconnect behavior, durable projection, draft handling, message submission, and cancellation.
- Deterministic Rust, protocol, recovery, live-event, and native-client test coverage.

## Not implemented yet

Craxii does not yet provide a polished conversation UI, production cloud deployment, managed multi-user operation, backup/restore tooling, a hardened release pipeline, signed or notarized releases, or a stable compatibility promise. There are no mobile clients. The repository should be evaluated as pre-alpha development software.

## Architecture

```text
native macOS diagnostic client
              |
       authenticated HTTP + WebSocket
              |
        Craxii Rust backend
     /          |           \
durable      scheduler /    model-provider
state/events   agent loop     adapter
                  |
            context + tools
                  |
         local workstation
```

The server owns canonical state. HTTP carries snapshots and commands; WebSocket delivery carries replayable durable events and non-canonical draft updates. The agent loop reaches the workstation only through registered, validated tools. See [the architecture overview](docs/architecture-overview.md) for the implemented boundaries.

## Quick start

Prerequisites are Rust 1.98 and Git. macOS client work additionally needs macOS 14 or newer and a Swift 6/Xcode toolchain.

```sh
git clone https://github.com/abhisht-s/craxii.git
cd craxii
cargo build --locked --workspace
cargo test --locked --workspace --all-targets
```

The checked-in local configuration is safe and uses only localhost, temporary paths, fixture model identifiers, and reserved endpoints. It can start the backend without a provider credential for backend and protocol development:

```sh
cargo run --locked -p craxii-server --bin craxii-server -- \
  --config backend/tests/fixtures/config/valid/local.toml
```

Protected API use requires a provisioned device bearer token. Agent execution additionally requires a deliberately configured provider credential; the routine build and deterministic tests do not require an OpenAI key. Follow [Getting started](docs/getting-started.md) for the initialization and provisioning sequence.

## Backend requirements

- Rust toolchain `1.98.0` as declared by `rust-toolchain.toml`.
- A Unix-like host for the current local workstation implementation.
- Writable state, artifact, and workspace directories selected by configuration.
- Optional OpenAI credentials supplied as restricted files, never in the TOML file.
- Linux-specific support for configured administrative shell execution; normal user-mode development does not require it.

## macOS client status

The repository includes a Swift package and an Xcode application under `clients/macos/`. The app is a diagnostic interface for endpoint selection, Keychain credential installation, connection state, projection counts, and safe error display. It is not a polished end-user chat application.

```sh
swift test --package-path clients/macos/CraxiiClient
scripts/verify-macos-client
```

The full native-client gate runs only on macOS with the required Xcode toolchain.

## Configuration and security

Configuration is strict, versioned TOML loaded with `--config`. Unknown keys, invalid bounds, unsafe network combinations, and unsupported values fail closed. Read [Configuration](docs/configuration.md) for the current surface and [Security model](docs/security-model.md) for the implemented trust boundaries and limitations.

Do not commit credentials, local databases, logs containing sensitive material, provider responses, or build artifacts.

## Development and testing

Use focused Rust or Swift tests while working. The consolidated repository gate is:

```sh
scripts/verify
```

It checks formatting, compilation, lints, tests, release safeguards, dependencies, public repository invariants, Markdown, and the macOS client when running on macOS. See [Development](docs/development.md) and [Contributing](CONTRIBUTING.md).

## Protocol

The implemented public transport contract is protocol version 1. Its route, command, replay, draft, and compatibility rules are summarized in [Protocol](docs/protocol.md). Checked-in language-neutral fixtures live in `backend/tests/fixtures/protocol-v1/`.

## Contributing and security reporting

Contributions are welcome under the guidelines in [CONTRIBUTING.md](CONTRIBUTING.md). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

Do not report suspected vulnerabilities in a public issue. Follow [SECURITY.md](SECURITY.md), which uses GitHub private vulnerability reporting once the repository setting is enabled.

## Project status

The current boundary is the implemented pre-alpha runtime and diagnostic client described above. Planned work is intentionally not presented as shipped behavior; changes become part of the public contract only when they are implemented, tested, and documented.

## License, copyright, and brand

The software is licensed under the [Apache License 2.0](LICENSE).

Copyright 2026 Abhisht Shrivastava.

The software license does not grant permission to present a modified distribution as the official Craxii project or product. Truthful references to origin and compatibility are welcome; see [TRADEMARKS.md](TRADEMARKS.md).
