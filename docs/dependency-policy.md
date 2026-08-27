# Dependency policy

This policy applies whenever Craxii adds or changes a direct dependency. A direct
dependency is any package, framework, executable, action, image, module, or managed
tool that repository code, builds, tests, verification, clients, or operations name
or invoke directly. Runtime, development, build, repository-tooling, Swift, and
infrastructure dependencies are all in scope.

Transitive dependencies do not each require a separate record, but their maintenance,
security, licensing, native-code, and source risks remain owned by the record for the
direct dependency that brings them in.

## Review ownership

The change author prepares the proposal and supporting evidence. The repository or
project owner, or a designated human reviewer, approves or rejects it. Codex may draft
the proposal and record, but does not self-approve a dependency.

Every dependency record must contain:

- package or tool;
- dependency kind;
- owning subsystem;
- responsible maintainer;
- primitive supplied;
- why the standard library or an existing approved dependency is insufficient;
- permitted layer;
- alternatives considered;
- maintenance and security posture;
- license and advisory result;
- unsafe, native, and system requirements;
- secrets, parsing, and persistence implications;
- removal or migration cost;
- approved version requirement; and
- review date.

## Categories requiring explicit scrutiny

| Category | Permitted ownership and layer concerns |
| --- | --- |
| Parsing | Trust-boundary parsing belongs in adapters or protocol-specific code. Domain and application code may receive validated types, not parser-specific types. |
| Persistence | Database engines, clients, migrations, and codecs belong in persistence adapters and bootstrap composition. Domain code must not depend on storage-library types. |
| Secrets | Secret acquisition belongs in bootstrap and secret-source adapters. Application and domain layers receive logical references or nonprintable values only. |
| Unsafe or Unix | Unsafe code and Unix-specific primitives require a narrow adapter or bootstrap boundary, documented invariants, and target-specific tests. They do not belong in domain code. |
| Schema generation | Generators belong in explicit build or repository tooling and must have an owned, reproducible output contract. Generated provider or transport types must not become domain types. |
| Cloud tooling | Cloud SDKs and infrastructure tools belong in adapters or `ops/`. Provider types and credentials must not leak into domain or application contracts. |

These categories require evidence about input bounds, failure behavior, platform or
native requirements, unsafe surface, credential handling, and the cost of replacement
where applicable.

## Rust version and source policy

- For an approved dependency purpose, select the latest stable sensible compatible
  release established by audit unless a recorded compatibility, security, maturity,
  architecture, or operational constraint justifies an older line.
- Use normal explicit compatible SemVer requirements by default.
- The committed `Cargo.lock` is the resolved application pin and is reviewed with
  every deliberate dependency update.
- Use an exact `=x.y.z` requirement only for a demonstrated compatibility or security
  constraint.
- Git dependencies require exceptional review and an immutable revision.
- Dependency updates are deliberate changes; they include the manifest, lockfile,
  upstream-change, advisory, and license review appropriate to the dependency.

The machine-readable approved Rust direct-dependency registry is
[`dependency-registry.json`](dependency-registry.json). It contains the owner-approved
`serde`, `sha2`, `toml`, and `url` requirements and declaration metadata. The approved
Swift dependency registry remains empty. No record may claim approval for a dependency
that has not been added and human-reviewed.

The first external Rust dependency has activated supply-chain enforcement with
`cargo-deny` exactly versioned by its decision record and `scripts/verify`, plus the
repository's `deny.toml`. Advisory, license, bans, and source checks are mandatory and
may not silently skip. Do not also add overlapping `cargo-audit` or `cargo-license`
checks unless a concrete uncovered gap is documented. `scripts/check-repository.mjs`
enforces every direct Cargo declaration against the machine-readable registry. Future
direct dependencies still require human review; Codex cannot expand or self-approve
the registry.

## Repository tooling records

The following exact repository-tool versions are baselined by Substage 1.3. Their
transitive packages are owned through these direct tool records; changing either tool
requires human review under this policy.

### `markdownlint-cli2` 0.22.1

- Package or tool: `markdownlint-cli2` `0.22.1`.
- Dependency kind: Development and repository-verification tooling; not a Craxii
  runtime dependency.
- Owning subsystem: Repository governance.
- Responsible maintainer: Repository or project owner.
- Primitive supplied: Structural Markdown linting, invoked by `scripts/verify`
  through `npx`.
- Why the standard library or an existing approved dependency is insufficient:
  Node's standard library has no Markdown rules engine, and the repository's
  structured invariant checker does not provide general Markdown linting.
- Permitted layer: Repository tooling and repository Markdown only. It is excluded
  from Craxii runtime code and the Cargo and Swift dependency registries.
- Alternatives considered: Manual review and custom Node lint rules. Manual review
  is not a repeatable gate, while custom rules would duplicate an established rules
  engine and expand repository-owned parsing logic.
- Maintenance and security posture: The direct version is exact in `scripts/verify`;
  its transitive packages are owned by this record, and any version change requires
  review of upstream changes and the security evidence available at that time.
- License and advisory result: Cached npm package metadata declares the MIT license.
  Advisory review was not independently queried in Substage 1.3 and is required at
  dependency update or review.
- Unsafe, native, and system requirements: Cached package metadata requires Node
  20 or newer and declares no direct install lifecycle script, native addon, or
  external system library. Transitive native or system behavior was not independently
  audited in Substage 1.3.
- Secrets, parsing, and persistence implications: It parses repository Markdown and
  lint configuration only. It must not receive credentials and has no Craxii runtime
  persistence role.
- Removal or migration cost: Replace the two pinned invocations in `scripts/verify`,
  migrate or remove `.markdownlint-cli2.jsonc`, and revalidate the Markdown gate.
- Approved version requirement: Proposed exact version `0.22.1`; it is not
  human-approved until the approval status below changes.
- Review date: 2026-08-27 for preparation of this technical record.
- Approval status and approver: Technical record prepared; pending repository or
  project owner approval before it is treated as human-approved. Codex is not the
  approver.

### `marked` 18.0.11

- Package or tool: `marked` `18.0.11`.
- Dependency kind: Documentation-rendering repository tooling; not a Craxii runtime
  dependency.
- Owning subsystem: Documentation tooling.
- Responsible maintainer: Repository or project owner.
- Primitive supplied: Markdown-to-HTML rendering, invoked through `npx` only by
  `docs/scripts/render-implementation-plan.mjs`.
- Why the standard library or an existing approved dependency is insufficient:
  Node's standard library has no Markdown renderer, and no existing approved
  dependency supplies one.
- Permitted layer: The implementation-plan documentation renderer only. It is
  excluded from Craxii runtime code and the Cargo and Swift dependency registries.
- Alternatives considered: A hand-written renderer and manual HTML maintenance. A
  hand-written renderer would create a larger repository-owned parsing surface, while
  manual maintenance conflicts with the Markdown-source/generated-HTML contract.
- Maintenance and security posture: The direct version is exact in the renderer;
  any version change requires review of upstream changes, rendered output, and the
  security evidence available at that time.
- License and advisory result: Cached npm package metadata declares the MIT license.
  Advisory review was not independently queried in Substage 1.3 and is required at
  dependency update or review.
- Unsafe, native, and system requirements: Cached package metadata requires Node
  20 or newer and declares no runtime package dependency, direct install lifecycle
  script, native addon, or external system library.
- Secrets, parsing, and persistence implications: It parses the repository-controlled
  implementation-plan Markdown and writes its generated HTML companion only when the
  renderer is intentionally run. It must not receive credentials and has no Craxii
  runtime persistence role.
- Removal or migration cost: Replace the single pinned renderer invocation, validate
  equivalent HTML generation, and deliberately regenerate the companion HTML if the
  rendering contract changes.
- Approved version requirement: Proposed exact version `18.0.11`; it is not
  human-approved until the approval status below changes.
- Review date: 2026-08-27 for preparation of this technical record.
- Approval status and approver: Technical record prepared; pending repository or
  project owner approval before it is treated as human-approved. Codex is not the
  approver.

These records establish repository-tooling scope only and do not expand the approved
Rust or Swift runtime dependencies. The approved Rust direct dependencies are defined
only by `dependency-registry.json`; the approved Swift external dependency registry
remains empty.
