# `nix` dependency decision

- Package or tool: `nix` from crates.io.
- Dependency kind and owner: Direct normal dependency confined to Unix SQLite lifecycle support
  and LocalWorkstation process cleanup.
- Purpose: `statfs` filesystem classification, owned advisory `flock` lifetime locking, and typed
  Unix process-group/PID signal operations.
- Declaration: `version = "0.31"`, default features disabled, exactly `fs`, `process`, and `signal`
  enabled. `process` is the required feature support for the approved signal APIs.
- Resolved version and MSRV: `nix 0.31.3`, MSRV `1.69`; verified with Rust `1.98.0`.
- License and source: MIT, crates.io registry; accepted by cargo-deny.
- Feature rationale: `fs` exposes statfs/flock; `signal` exposes typed SIGTERM/SIGKILL operations;
  `process` exposes the typed `Pid` support required by those operations. Socket, user, mount, and
  other convenience features remain disabled.
- Graph and build script: resolves `bitflags`, `cfg-if`, `libc`, and build dependency
  `cfg_aliases 0.2.2`; its Rust build script selects platform cfg aliases and invokes no native
  compiler.
- Native/unsafe implications: nix wraps platform `statfs(2)`, `flock(2)`, `kill(2)`, and
  process-group operations through libc and contains reviewed FFI/unsafe internally. Craxii keeps
  its narrow child `pre_exec` syscalls inside LocalWorkstation and retains the owned lock object for
  lifetime release. No external native library is required.
- Advisories: the locked cargo-deny advisory check must pass with no ignore.
- Alternatives rejected: PID files do not provide exclusion; ad-hoc libc adds unsafe code; a
  filesystem-name heuristic cannot replace mounted-filesystem classification.
- Removal/replacement path: Replace with standard-library APIs only if they provide equivalent
  owned nonblocking locking and platform filesystem identity; preserve classifier and process tests.
- Review date and approval: 2026-08-29, feature expansion explicitly approved by the
  repository/project owner for Stage 13.
