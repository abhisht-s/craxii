# Getting started

Craxii is pre-alpha. This guide starts the implemented backend locally and explains how to prepare the diagnostic macOS client. It does not describe a production deployment.

## Prerequisites

- Git.
- The Rust `1.98.0` toolchain selected by `rust-toolchain.toml`.
- A Unix-like development host for the backend's local-workstation adapter.
- For native-client work: macOS 14 or newer and a Swift 6/Xcode toolchain.

Clone and build:

```sh
git clone https://github.com/abhisht-s/craxii.git
cd craxii
cargo build --locked --workspace
cargo test --locked --workspace --all-targets
```

## Prepare the local fixture

The checked-in development configuration uses localhost, `/tmp/craxii-dev`, fixture model names, and `.invalid` provider endpoints. It contains no credential value.

```sh
install -d -m 700 /tmp/craxii-dev/state /tmp/craxii-dev/credentials
install -d -m 755 /tmp/craxii-dev/workspaces/primary
```

The state root must already exist, be private to its owner, and not be a symlink. The backend creates its database/lock children and artifact directory beneath that validated root.

Review `backend/tests/fixtures/config/valid/local.toml`, then start the backend:

```sh
cargo run --locked -p craxii-server --bin craxii-server -- \
  --config backend/tests/fixtures/config/valid/local.toml
```

The fixture can start without an OpenAI credential. This is useful for backend startup, liveness, persistence, bootstrap, and client transport development, but the scheduler is not started and model-backed work cannot run without a configured provider. Check the unprotected health endpoints from another shell:

```sh
curl --fail http://127.0.0.1:8080/health/live
curl --include http://127.0.0.1:8080/health/ready
```

Without a provider credential, liveness returns `200`/`live` while readiness deliberately returns `503`/`live_unready`. A correctly configured provider starts the scheduler; readiness becomes `200` only after its initial durable-work scan.

Stop the server with Control-C before using the offline administration command.

## Provision a development client

After the backend has initialized its state once and is stopped, provision a device:

```sh
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml \
  device provision "Local development Mac"
```

The command writes the bearer token once to standard output. Treat it as a secret: do not paste it into an issue, shell history, repository file, log, or screenshot. Restart the backend after the administration command exits.

## Inspect local evidence

With the backend stopped, the same administration binary can inspect initialized state without migrating or mutating it:

```sh
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml preflight
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml verify-state --format markdown
cargo run --locked -p craxii-server --bin craxii-admin -- \
  --config backend/tests/fixtures/config/valid/local.toml evidence-export
```

The default output is deterministic JSON; `--format markdown` is also supported. Use `inspect-work <work-uuid>` or `inspect-runtime <runtime-uuid>` for a single safe evidence view. Reports contain identifiers, classifications, timings, counts, hashes, and artifact integrity facts, but never conversation/model content, tool arguments/output, credentials, environment values, or arbitrary paths. `verify-state` prints its findings and exits nonzero when consistency fails; it never repairs data.

For authenticated protocol requests, send the token as `Authorization: Bearer <token>`. Mutation requests also require `Content-Type: application/json` and an `Idempotency-Key` matching the command identity in the request body. See [Protocol](protocol.md).

## Build the macOS diagnostic client

Run the package tests:

```sh
swift test --package-path clients/macos/CraxiiClient
```

Open `clients/macos/Craxii.xcodeproj` in Xcode or run the complete native test gate:

```sh
scripts/verify-macos-client
```

The debug app defaults to `http://127.0.0.1:8080/`. Install the provisioned bearer token through its secure credential field. The app stores the credential in Keychain and shows diagnostic connection and projection state; it is not yet a polished chat interface.

## Optional live OpenAI testing

Normal builds and deterministic tests do not require a live provider or incur API usage. The live OpenAI smoke test is explicit and spend-bearing:

```sh
CRAXII_OPENAI_LIVE=1 \
OPENAI_API_KEY='<set outside repository>' \
CRAXII_OPENAI_MODEL='<enabled model id>' \
scripts/verify-openai-live
```

The wrapper copies the key into a restricted temporary credential file, removes it from the child test environment, and removes the temporary directory on exit. Use only an account and model intentionally authorized for testing.

## Routine verification

Run the full local gate before submitting a pull request:

```sh
scripts/verify
```

Current limitations include pre-alpha compatibility, local-only workstation operation, no production deployment guide, no managed backup/restore, and no signed release distribution.
