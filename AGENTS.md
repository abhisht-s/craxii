# Repository Guidelines

## Architecture and source-of-truth order

Implement Craxii V0.0.01 from `docs/craxii-v0.0.01-architecture.md`; its MUST/MUST NOT requirements are normative. Follow `docs/craxii-v0.0.01-implementation-plan.md` for dependency-ordered stages and acceptance gates. Use `docs/craxii-identity-credential-architecture.md` only for long-term seams not superseded by V0. Treat the deep draft, `docs/temp/` review, annotated HTML, and empty `docs/craxii2.md` as non-normative history or placeholders. Change a durable contract in the architecture before changing its implementation.

## Project structure and module organization

The repository is currently documentation-first. Keep design records in `docs/` and rendering utilities in `docs/scripts/`. Generated HTML files are companions; edit their Markdown sources and regenerate them rather than hand-editing HTML.

As implementation stages begin, use these ownership boundaries:

- `backend/`: one Rust package with `bootstrap`, `domain`, `application`, `ports`, and `adapters`; keep `main` composition-only.
- `backend/migrations/`: immutable, versioned SQLx SQLite migrations.
- `clients/macos/`: native SwiftUI app plus unit and UI tests.
- `ops/`: Ubuntu/AWS deployment, systemd, Caddy, backup, and restore assets.
- `scripts/`: repository-wide verification commands.

Create directories only with their first real file. Do not add speculative empty layers.

## Build, test, and development commands

For current documentation work, run:

```sh
rg '^#{1,6} ' docs/
rg 'TODO|FIXME|TBD' docs/
export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:$PATH"
npx markdownlint-cli2 'docs/**/*.md'
node docs/scripts/render-implementation-plan.mjs
```

When Stage 1 adds Rust, the baseline gate must include `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace --all-targets`, and `cargo test --workspace`. Run macOS and Ubuntu-specific suites only on environments whose semantics they claim.

## Coding and testing conventions

Use sentence-case Markdown headings, one H1, fenced blocks with language tags, relative links, and kebab-case filenames. In Rust, preserve inward dependencies: domain code must not import Axum, SQLx, provider, or operating-system types. Test every durable transition against both current-state rows and journal events; test side effects for intent-before-action and honest unknown outcomes. Live-provider tests are opt-in and must report “not configured,” never silently pass.

## Commits, pull requests, and security

No Git history exists yet, so use short imperative subjects such as `docs: clarify replay contract`. Keep commits stage-focused. Pull requests must identify the implementation-plan stage, architecture consequences, validation performed, unresolved questions, and primary-source links; include screenshots for layout-sensitive UI or generated documents.

Never commit credentials, local databases/WAL files, artifacts, test evidence containing secrets, Terraform state, or build output. Redact tokens, content, commands, outputs, and headers from logs and review artifacts by default.
