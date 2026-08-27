# `nix` dependency decision

- Package or tool: `nix` from crates.io.
- Dependency kind and owner: Direct normal dependency confined to Unix SQLite lifecycle support.
- Purpose: `statfs` filesystem classification and owned advisory `flock` lifetime locking.
- Declaration: `version = "0.31"`, default features disabled, exactly `fs` enabled.
- Resolved version and MSRV: `nix 0.31.3`, MSRV `1.69`; verified with Rust `1.98.0`.
- License and source: MIT, crates.io registry; accepted by cargo-deny.
- Feature rationale: `fs` exposes the required statfs/flock APIs. No process, signal, socket, user,
  mount, or other convenience feature is enabled.
- Graph and build script: resolves `bitflags`, `cfg-if`, `libc`, and build dependency
  `cfg_aliases 0.2.2`; its Rust build script selects platform cfg aliases and invokes no native
  compiler.
- Native/unsafe implications: nix wraps platform `statfs(2)` and `flock(2)` through libc and
  contains reviewed FFI/unsafe internally. Craxii writes no raw syscall wrapper and retains the
  owned lock object for lifetime release.
- Advisories: the locked cargo-deny advisory check must pass with no ignore.
- Alternatives rejected: PID files do not provide exclusion; ad-hoc libc adds unsafe code; a
  filesystem-name heuristic cannot replace mounted-filesystem classification.
- Removal/replacement path: Replace with standard-library APIs only if they provide equivalent
  owned nonblocking locking and platform filesystem identity; preserve classifier and process tests.
- Review date and approval: 2026-08-27, approved by the repository/project owner.
