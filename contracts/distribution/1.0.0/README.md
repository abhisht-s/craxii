# Craxii distribution contracts 1.0.0

This is the authoritative, publishable language-neutral bundle for `craxii.worker`
protocol **1.0**. It is distinct from the native client protocol, application
release, PostgreSQL contract and local SQLite migration version.

The thirteen Draft-7 schemas define primitives, negotiation, authentication, work,
results, status, lifecycle, accounting, compute, model semantics, execution
profiles, safe errors and test fixtures. Definition references use local
`urn:craxii:distribution:1.0.0:<family>#/definitions/<Name>` identifiers. Resolve
only the listed trusted schemas. Do not fetch schemas supplied in payloads.

[SEMANTICS.md](SEMANTICS.md) specifies bytes, identity, ownership, commit boundaries
and uncertainty. [compatibility.md](compatibility.md) specifies negotiation and
release rules. `manifest.json` pins every other file's exact bytes and the bundle
digest. All fixtures, including the explicitly public Ed25519 seed, are synthetic
conformance data. They grant no enrollment, trial, payment or provider authority.

The control plane vendors this complete directory byte-for-byte, including the
manifest, and pins its content digest. Its guarded synchronization command accepts
an explicit Rust checkout; builds and CI verify the vendor copy offline. Neither
language needs a checkout of the other repository at runtime.

Run `node scripts/check-distribution-contracts.mjs` and
`cargo test --locked -p craxii-server --test distribution_contracts` from the Rust
repository root. Both language implementations must pass every shared case before
publication. Publish the authoritative source before a verified vendor update.
Frozen wire changes require a new version and preserved older fixtures; never
regenerate existing golden expectations to hide disagreement.
