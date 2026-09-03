# Dependency policy

This policy applies whenever Craxii adds or changes a direct dependency: any package, framework, executable, action, image, module, or managed tool named by the product, builds, tests, verification, clients, or operations. Runtime, development, build, repository-tooling, Swift, and infrastructure dependencies are in scope.

Transitive packages do not each require a separate decision record, but their maintenance, security, license, native-code, and source risks remain owned through the direct dependency that introduces them.

## Review record

The change author supplies the evidence; a repository maintainer approves or rejects the dependency. The record must identify:

- package/tool, version requirement, dependency kind, owning subsystem, and maintainer;
- the primitive supplied and why an existing dependency or standard library is insufficient;
- permitted layer and alternatives considered;
- maintenance, security, advisory, source, and license posture;
- unsafe code, native code, build scripts, platform/system requirements, and network behavior;
- parsing, credential, persistence, and sensitive-data implications;
- removal/migration cost; and
- review date and approval status.

Parsing, persistence, credentials, cryptography, network clients, operating-system primitives, code generation, and infrastructure tooling require additional scrutiny appropriate to their trust boundary.

## Rust dependencies

- Prefer the latest sensible stable compatible release supported by the project toolchain, unless a recorded compatibility, security, maturity, or operational reason justifies another line.
- Use normal explicit compatible SemVer requirements by default. Exact requirements need a demonstrated reason.
- Git dependencies require exceptional review and an immutable revision. Registry sources must comply with `deny.toml`.
- Commit and review `Cargo.lock` with every resolved dependency change.
- Review upstream changes, advisories, licenses, duplicate versions, native/build behavior, and enabled features.

The machine-readable direct-dependency inventory is [`dependency-registry.json`](dependency-registry.json). Every direct Cargo declaration must match an approved entry and a public record under `docs/dependencies/`. `scripts/check-public-repository.mjs` enforces that relationship.

`cargo-deny 0.20.2` is the exact repository supply-chain tool version. The consolidated gate fails on unavailable advisory data, disallowed licenses, forbidden sources, wildcards, or policy errors:

```sh
cargo deny --locked check advisories -D warnings
cargo deny --locked check licenses bans sources
```

Accepted licenses are explicit in `deny.toml`. Passing the SPDX policy does not replace release-specific preservation of third-party license and notice text; see [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).

## Swift dependencies

The current Swift package declares no external dependencies. Any proposal to add one requires the same maintenance, source, security, license, platform, privacy, and removal analysis, plus an explicit registry mechanism before approval.

## Repository tooling

`markdownlint-cli2` `0.22.1` is invoked exactly through `npx` by `scripts/verify`. It is development tooling, not shipped runtime code. Its declared license is MIT, it requires Node 20 or newer, and version changes require review of its package metadata, transitive graph, install behavior, advisories, and lint compatibility.

Avoid overlapping tools that duplicate advisory or license enforcement without closing a documented gap. Repository tools must not receive application credentials or sensitive runtime data.

## Update checklist

1. Prepare or update the public dependency record.
2. Obtain maintainer approval.
3. Change the manifest and regenerate the lockfile deliberately.
4. Update `dependency-registry.json` when a direct Cargo declaration changes.
5. Run focused affected tests, the dependency checks, and `scripts/verify`.
6. Update `THIRD_PARTY_NOTICES.md` or release packaging if notice obligations change.
