# Stage 12 workstation port, capabilities, logical paths, and real file reads contract

Date: 2026-08-29

Status: accepted for Stage 12

## Decision

Stage 12 introduces one dependency-neutral `Workstation` port and one production
`LocalWorkstation` adapter. It changes no durable schema and adds no dependency. The schema ceiling
remains version 3, migrations remain exactly `0001` through `0003`, and the current 17-table,
40-index, 28-journal-kind, zero-trigger, zero-view contract is unchanged. Existing standard-library,
Tokio, nix, and SHA-256 facilities are sufficient; Tokio filesystem features are not enabled.

The port has exactly five methods: `capabilities`, `read_file`, `execute`, `inspect_execution`, and
`cancel_execution`. `capabilities` and `read_file` are real in Stage 12. The other three methods have
canonical request/result types, validate workstation identity and generation (and workspace where
applicable), then return `unsupported_capability` without starting, inspecting, registering, or
cancelling a process. Stage 13 owns all real process, PID, process-group, cgroup, output-capture,
inspection, and cancellation behavior.

Every operation has a caller-generated canonical UUIDv7 `OperationId`, distinct from `ExecutionId`,
`WorkId`, correlation identity, and PID. Requests carry workstation identity, expected generation,
workspace identity where relevant, logical request data, and a process-local monotonic deadline
where the architecture requires one. No ambient current directory supplies authority or path
meaning.

The Stage 12 capability snapshot reports only implemented behavior:

- filesystem read and user privilege are true;
- foreground execution, cancellation, inspection, administrative privilege, process-group cleanup,
  and cgroup cleanup are false;
- execution timeout, stdout, and stderr maxima are zero until Stage 13;
- architecture and OS family come from `std::env::consts::{ARCH, OS}` without a subprocess;
- the active workspace ID and logical root are exact.

Configured future shell or execution ceilings do not become capability claims. Durable OS evidence
continues to represent normalized platform-family truth. Exact Ubuntu 24.04 and non-root behavior
are Linux target assertions, not facts fabricated on macOS.

`LocalWorkstation` is constructed after the verified bootstrap snapshot is available and is retained
behind `Arc<dyn Workstation>` in composition. It owns immutable workstation ID, generation, active
workspace ID, logical workspace root, canonical configured physical root, read hard limit, truthful
capability snapshot, and a clock. The physical production root comes from validated
`paths.primary_workspace_root`; tests provide explicit temporary roots. Construction never consults
`current_dir` and owns no process registry or execution state.

The existing POSIX `LogicalPathReference` grammar remains authoritative. Workspace-relative paths
resolve from the explicit physical workspace root. Absolute paths remain explicit machine paths;
the workspace root is a base, not a sandbox. Relative traversal may pop prior normal components but
cannot escape above the logical root, while absolute traversal clamps at `/`. NUL, backslash, empty
relative results, and canonical UTF-8 paths above 4,096 bytes are invalid. Resolution never uses a
string-prefix containment test.

One shared adapter resolver serves `read_file` and future workstation operations. Existing targets
are canonicalized for redacted resolved-path evidence. Symlinks to regular files inside or outside
the workspace and symlink chains are allowed. Broken links return `not_found`; loops return
`invalid_path`. Only the final opened regular file can succeed. Directories, FIFOs, sockets,
character devices, block devices, and recognized pseudo-filesystem targets are rejected before
content reads. This is broad own-machine authority, not path confinement.

`read_file` uses `tokio::task::spawn_blocking` around synchronous descriptor-based I/O. The blocking
operation canonicalizes for evidence, opens read-only with close-on-exec and nonblocking-safe Unix
flags, checks metadata on the opened descriptor, enforces logical size, reads the same descriptor
into bounded memory, and checks the same descriptor again. Internal descriptor identity and
metadata comparisons detect observable replacement/mutation without exposing device or inode
values through the port. Atomic replacement after open may consistently return the originally
opened file; mixing path metadata from one file with bytes from another is forbidden.

The caller/model-facing default read maximum is 1,048,576 bytes and the adapter hard maximum is
8,388,608 bytes. A request maximum must be positive and no greater than the hard maximum. Exact-limit
files succeed. Oversize logical files fail before allocation with `file_too_large`; sparse files use
logical length. Reads never truncate or partially succeed. Growth beyond the limit, shrinkage, or
relevant observable metadata mutation yields `changed_during_read`.

Success requires strict UTF-8 and preserves exact bytes as text, including a BOM, newline form,
whitespace, Unicode normalization, and valid NUL bytes. Invalid UTF-8 yields `binary_content`, never
lossy text or base64. A successful `FileReadResult` contains `OperationId`, the requested logical
path, redacted resolved-path evidence, regular file type, exact byte length, optional modified time,
UTF-8 encoding, SHA-256 of the returned bytes, exact text, and `truncated = false`. It exposes no file
descriptor, device/inode identity, process fact, artifact path, SQL type, public DTO, or protocol
shape. Binary failure may retain only safe byte length and SHA-256 evidence.

The stable workstation error kinds are `workstation_unavailable`, `generation_mismatch`,
`workspace_not_found`, `invalid_path`, `not_found`, `permission_denied`, `binary_content`,
`file_too_large`, `changed_during_read`, `unsupported_capability`, `timeout`, `cancelled`, `io_error`,
and `internal_workstation_error`. Errors do not retain raw I/O messages or hostile path text.
`timeout` and `changed_during_read` have bounded retry-later advice; workstation unavailability and
permission denial require operator/environment action; deterministic failures and generic I/O are
not automatically retryable. No automatic retry is performed.

An already-expired deadline fails before filesystem I/O. The blocking read rechecks the monotonic
deadline at safe boundaries. Dropping its future can stop awaiting but does not pretend to cancel a
kernel read and creates no durable cancellation protocol. Regular-file and size gating are the
primary in-flight safety boundaries; Stage 13 owns execution cancellation.

Each read is memory-only and performs no journal append, Work transition, client command, artifact
copy/finalization, durable receipt, target write, or adjacent temporary-file creation. Stage 14's
Tool Execution Service owns durable dispatch and result evidence.

Bootstrap may refresh current `workstations.capabilities_json`, `workstations.last_seen_at`, and
`workspaces.local_resolved_root` in its controlled SQLite write transaction after existing durable
state has been decoded and verified. The `craxii.initialized.capabilities_sha256` field remains the
immutable initial snapshot digest and is never rewritten to follow current capability evidence.
Bootstrap retains the workstation for later injection and remains `live_unready`; it does not call
`mark_ready`.

Stage 12 adds no HTTP route, WebSocket message, public DTO, capability endpoint, or model-facing tool
schema. Stage 11 protocol inventory and goldens stay unchanged. Stage 14 owns the Tool Registry,
authority evaluation, model tool definitions, durable dispatch, and artifact promotion. Provider,
context, agent-loop, native-client, deployment, and RemoteWorkstation work also remain deferred.
