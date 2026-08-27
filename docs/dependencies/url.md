# `url` dependency decision

- Package or tool: `url` from crates.io.
- Dependency kind: Direct normal Cargo dependency.
- Owning subsystem: Bootstrap configuration validation and normalization.
- Responsible maintainer or owner role: Repository or project owner.
- Primitive supplied: WHATWG-compatible absolute URL parsing, validation, and
  normalization for public and provider endpoint configuration.
- Why the standard library or current approved dependencies are insufficient:
  Rust's standard library has no URL parser. Manual parsing would mishandle authority,
  Unicode host names, percent encoding, schemes, and normalization at a security
  boundary.
- Permitted layer or scope: Bootstrap and adapter-owned operational URL values only.
  `url::Url` must not become a domain or application API type.
- Alternatives considered: Manual string validation, `http::Uri`, and regex-based
  validation. Manual or regex parsing is incomplete; `http::Uri` does not supply the
  intended URL-standard normalization and would add a different direct dependency.
- Maintenance and security posture: Registry metadata on 2026-08-27 reported stable
  `2.5.8` with Rust version `1.63`; it is compatible with Rust `1.98.0`. URL and IDNA
  parsing are security-sensitive, so resolved `url`, `idna`, and ICU changes require
  deliberate lockfile and advisory review.
- Exact license evidence: crates.io/Cargo package metadata declares
  `MIT OR Apache-2.0`; the packaged crate contains `LICENSE-MIT` and
  `LICENSE-APACHE`. The full resolved subtree also requires `Unicode-3.0`, which is
  explicitly allowed by `deny.toml`; cargo-deny accepted the graph without a
  clarification or exception.
- Advisory result: `cargo deny --locked check advisories -D warnings` reported
  `advisories ok` for the resolved application graph on 2026-08-27. The repository
  advisory ignore list is empty.
- Unsafe, native, system, and build-script implications: The direct crate declares
  `build = false` and contains one `from_utf8_unchecked` parser optimization. The IDNA
  path brings ICU data, zero-copy containers, proc macros, and transitive unsafe code;
  its data/proc-macro packages account for several Rust build scripts in the graph.
  No resolved package declares a native `links` target, and no external system library
  is required.
- Secrets, parsing, and persistence implications: It parses operator-controlled URL
  strings. Craxii must enforce allowed schemes, absolute URLs, bind/public URL safety,
  and redacted errors after syntax parsing. It must not receive credentials embedded
  in URLs and has no persistence role.
- Removal or migration cost: Moderate to high. A replacement must preserve exact URL
  acceptance, normalization, Unicode/IDNA behavior, security validation, and all
  configuration fixtures.
- Approved version requirement: Compatible `2.5` line with default features and no
  additional features.
- Resolved and tested version: `2.5.8` from crates.io.
- Review date: 2026-08-27.
- Approval status and approver role: Approved by the repository/project owner on
  2026-08-27. Codex recorded the decision and is not the approver.
