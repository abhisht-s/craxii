# Contributing to Craxii

Craxii is pre-alpha. Interfaces and internal implementation details can change quickly, but contributions should preserve the explicit durable-state, security, and protocol boundaries already implemented.

## Before you start

Small fixes, focused tests, and documentation corrections can go directly to a pull request. For substantial behavior, protocol, persistence, security, or dependency changes, open a feature request or discussion first so the intended contract can be agreed before implementation.

Security vulnerabilities must not be reported through ordinary issues or discussions. Use the process in [SECURITY.md](SECURITY.md).

## Development setup

Install Git and the Rust `1.98.0` toolchain declared in `rust-toolchain.toml`. macOS client changes require macOS 14 or newer and a Swift 6/Xcode toolchain. The repository has no external Swift package dependencies.

```sh
git clone https://github.com/abhisht-s/craxii.git
cd craxii
cargo build --locked --workspace
```

See [Getting started](docs/getting-started.md), [Development](docs/development.md), and [Configuration](docs/configuration.md).

## Working and testing

- Keep domain and application behavior independent of transport, database, provider, and operating-system library types.
- Add or update behavioral tests with behavior changes. Prefer a focused package, integration-test, or Swift target while iterating.
- Preserve protocol-v1 fixtures when the public wire contract is unchanged; update fixtures and their manifest deliberately when the contract changes.
- Keep logs, fixtures, and review evidence free of credentials, personal data, model-private content, and machine-specific paths.
- Write public documentation from implemented source, tests, and fixtures. Do not document planned behavior as available.

Useful focused commands include:

```sh
cargo test --locked -p craxii-server
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
swift test --package-path clients/macos/CraxiiClient
```

Before requesting review, run the consolidated gate:

```sh
scripts/verify
```

## Dependencies

New direct runtime, build, test, repository-tooling, or Swift dependencies require an explicit maintenance, security, licensing, source, and boundary review. Update the lockfile and the public dependency record together. See [Dependency policy](docs/dependency-policy.md).

## Pull requests

Keep each pull request focused. Explain the user-visible or contributor-visible problem, the chosen behavior, important tradeoffs, and the exact tests run. Call out protocol, persistence, security, platform, or dependency impact. Avoid committing generated output, local databases, credentials, or unrelated formatting churn.

By submitting a contribution, you represent that you have the right to submit it and agree that it is provided under the repository's Apache License 2.0. Craxii does not currently require a contributor license agreement, Developer Certificate of Origin signoff, or signed-off commits.
