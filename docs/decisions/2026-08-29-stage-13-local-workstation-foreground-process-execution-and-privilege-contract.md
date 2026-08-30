# Stage 13 LocalWorkstation foreground process execution and privilege contract

## Status and sources

Accepted for Craxii V0.0.01 local implementation on 2026-08-29. The normative source remains
[`craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md), and the dependency order remains
[`craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md). This record fixes
implementation choices without introducing Stage 14 tool semantics.

## Persistence and dependency boundary

Stage 13 has no migration, schema mutation, new event kind, or durable execution row. Migrations
`0001`–`0003`, schema ceiling `3`, and the historical `craxii.initialized` capability digest remain
unchanged. Startup refreshes only current `workstations.capabilities_json`, `last_seen_at`, and
`workspaces.local_resolved_root`.

The owner-approved dependency expansion adds only Tokio `process` and nix `signal` plus its required
`process` support. Tokio remains default-feature-free with `io-util`, `macros`, `net`, `process`,
`rt-multi-thread`, `signal`, `sync`, and `time`; nix remains default-feature-free with `fs`,
`process`, and `signal`. There is no new crate.

## Launch contract

One request contains one nonempty UTF-8 Bash command string, at most 65,536 bytes and without NUL.
LocalWorkstation passes it unchanged as the single command argument in:

```text
/bin/bash --noprofile --norc -o pipefail -c <command>
```

There is no login shell, startup file, argv joining, or second interpolation layer. Each call is a
fresh shell. Stdin is `/dev/null`; no backend stdin, PTY, or interactive terminal is inherited.

User execution runs at the backend's non-root Unix identity and receives exactly the launcher
allowlist `HOME=/home/craxii`, `USER=craxii`, `LOGNAME=craxii`, `SHELL=/bin/bash`, `LANG=C.UTF-8`, the
fixed user `PATH`, `CRAXII_WORK_ID`, and `CRAXII_WORKSPACE_ID`. Administrative execution uses
absolute `/usr/bin/sudo -n`, absolute `/usr/bin/env -i`, the fixed root identity values and root
`PATH`, then the same direct Bash arguments. `sudo -E`, prompts, a root backend, setuid, helper
daemons, and caller environment overrides are forbidden.

`ExecutionRequest` therefore carries `WorkId` and `WorkspaceId`, not a generic environment vector.
LocalWorkstation calls `env_clear`, marks observed unrelated descriptors close-on-exec, and never
injects provider keys, cloud credentials, proxy credentials, bearer/device tokens, SSH agent or Git
credential environment, tracing filters, or generic parent secret canaries. Bash-created shell
variables after exec are not ambient parent inheritance.

## Cwd and identity

Every request has an explicit logical cwd. The Stage 12 resolver handles workspace-relative and
absolute paths, including outside-workspace and symlink directories under broad V0 authority. The
resolved target must exist and be a directory. LocalWorkstation opens the resolved directory and the
narrow child hook uses `fchdir`, so path replacement after open cannot redirect execution. Missing,
file, malformed, and permission failures retain normalized Workstation categories. There is no
ambient current-directory fallback.

`OperationId` identifies the Workstation call. Caller-generated `ExecutionId` identifies one live
execution lifecycle and is never PID-derived. Workstation ID/generation and workspace ID are checked
before access. A duplicate live `ExecutionId` fails before a second spawn.

## Runtime ownership and containment

One LocalWorkstation runtime owns a short-lock `HashMap<ExecutionId, Arc<ExecutionEntry>>`. Entries
contain only ephemeral phase, cancellation latch, terminal notification, and supervisor handoff
state. A retained manager `JoinHandle` owns a `JoinSet` of supervisors; each supervisor owns and
joins its two drain tasks. Join panics are observed and become uncertain cleanup failure. No lock is
held across an await or OS call, no lifecycle task is detached, and no completion is fabricated or
force-cleared.

The live registry and execute-admission bit form one lock-protected authority. The final admission
decision and insertion are atomic: before insertion shutdown may reject the call without ownership;
after insertion LocalWorkstation owns the `ExecutionId` and shutdown latches cancellation on that
entry instead of applying another global admission rejection. A separate short-held entry lifecycle
lock orders the first terminal cause against the spawn claim. Shutdown winning that claim produces a
definite owned `Cancelled` result with `start_observed=false`, no process/cgroup/drain claims beyond
truthful not-applicable cleanup, and normal terminal notification/removal. A spawn claim winning
first keeps the child owned and routes shutdown through the existing TERM/KILL/reap/drain contract.

Dropping the caller awaiting `execute` does not drop the manager, child, drains, or registry entry.
The supervisor continues through terminal cleanup. A coherent terminal entry may be inspected for
one scheduler handoff before removal; no terminal history is retained indefinitely. After removal or
restart, `inspection_not_found` is honest and does not prove nonexecution.

The child hook is restricted to already-prepared `fchdir`, `setsid`, cgroup attachment, and
close-on-exec descriptor flags. It performs no tracing, formatting, allocation, locking, or arbitrary
Rust logic. The direct child has `kill_on_drop` only as defense in depth.

On Linux, configuration may name one absolute delegated cgroup-v2 root below `/sys/fs/cgroup`.
Capability probing must create, inspect, and remove an empty child there. Each execution creates a
subtree named from `ExecutionId`; the child writes `0` to its prepared `cgroup.procs` before Bash
exec. TERM targets the owned process group and cgroup membership; KILL targets both and uses
`cgroup.kill`. Terminal cleanup requires direct reaping, drain joins, process-group emptiness,
cgroup emptiness, and subtree removal. macOS uses only the locally provable new-session/process-group
approximation and never advertises cgroup equivalence.

All Unix terminal paths observe direct-child termination without reaping by using
`waitid(WEXITED | WNOHANG | WNOWAIT)`. The waitable session leader retains the PID/process-group
identity while final descendant TERM/KILL and quiescence proof run. A stable-group capability is
then consumed before the leader is reaped, and no later code may signal the numeric PGID. Linux
continues to use the execution cgroup as the definitive reusable containment identity.

Each supervisor poll makes one `waitid` attempt. `EINTR` normalizes to an interrupted, nonterminal
observation and returns control to Tokio's existing poll wake, cancellation, request deadline, and
original Stage 10 shutdown deadline selection before retry. It is not terminal state, `ECHILD`,
liveness proof, or cleanup failure. Zero `si_pid` remains pending; other syscall failures retain the
existing uncertain-ownership handling. This forbids synchronous retry loops that a signal storm
could use to outrun lifecycle deadline authority.

## Inspect, cancellation, timeout, and shutdown

Inspection exposes only `Running`, coherent `Terminal`, or `inspection_not_found`; starting and
terminating are represented as running. Cancellation exposes `Confirmed`, `AlreadyTerminal`,
`NotFound`, or `CleanupUnconfirmed`. Repeated and concurrent cancels join the same cleanup. No PID or
group is signalled after registry ownership is gone.

Natural exit, cancellation, timeout, and shutdown share one atomic first-cause latch. Runtime timeout
defaults to 120 seconds and has a 900-second hard maximum. The absolute request deadline covers
validation through cleanup. Termination sends TERM, waits at most five seconds without extending the
absolute deadline, sends KILL and Linux `cgroup.kill` when needed, reaps, drains, verifies, and only
then reports `TimedOut` or `Cancelled`. Any possible surviving member produces `CleanupFailed` with
outcome-unknown certainty. Direct-child exit also cleans descendants before pipe EOF is awaited, so
ordinary background grandchildren cannot hold drains open.

LocalWorkstation is a participant in the existing Stage 10 authority. Shutdown latches the existing
deadline, drains health/listeners/mutation admission, closes execute admission before
`runtime.stopping`, latches shutdown cancellation on owned executions, and joins their
TERM/KILL/reap/drain cleanup under that same deadline before SQLite closes. Inspect/cancel remain
available during cleanup. No machine action starts after execute admission closes, and no secondary
shutdown deadline or authority exists. The low-level result is `Cancelled`; Stage 14 later owns the
durable graceful-shutdown work classification.

The controller establishes that absolute deadline once at the Stage 10 latch and passes the exact
instant into LocalWorkstation. Each supervisor uses
`min(execution_request_deadline, original_stage10_shutdown_deadline)` for every remaining cleanup
phase, including a shortened TERM grace. Deadline expiry triggers an immediate final group/cgroup
and direct-child kill attempt, outcome-unknown `CleanupFailed`, a truthful Stage 10 error, and a
nonblocking supervisor/registry handoff join before SQLite closure; it does not start a new grace
period, detach a lifecycle task, or fabricate cancellation proof.

## Output and result contract

Stdout and stderr are distinct raw byte pipes drained concurrently from spawn. Each artifact capture
retains at most 8,388,608 prefix bytes while draining and saturating-counting all observed bytes.
Crossing the ceiling never kills the command and is not a terminal error. Each stream reports
observed, captured, omitted, projection-omitted, saturation, truncation, SHA-256/artifact evidence,
and a lossy-UTF-8 replacement marker. Newlines are not normalized.

The display projection is at most 32,768 raw bytes. A shortened projection is exactly a 24 KiB head
plus rolling 8 KiB tail. The binary artifact remains the authoritative captured prefix. Stage 13 may
finalize unreferenced low-level artifacts but creates no durable tool reference or transaction.

`ExecutionResult` carries operation/execution IDs, start observation, requested and resolved cwd,
effective privilege, command SHA-256, `Exited`, `Signaled`, `TimedOut`, `Cancelled`, `SpawnFailed`,
or `CleanupFailed`, exit code or normalized signal, timeout/cancel flags, monotonic duration,
independent stream evidence, cleanup evidence, normalized error, and certainty. PID, PGID, and cgroup
path never enter the port result. Bash exit 127 after spawn is `Exited(127)`, not spawn failure.
Missing launchers, permission, invalid cwd, resource, child creation, child-hook, and cgroup attach
failures map to stable redacted Workstation outcomes.

## Capability, logging, crash, and Stage 14 boundaries

Filesystem read remains true. Foreground execution, cancel, inspect, user privilege, process-group
cleanup, and the fixed timeout/output limits become true only after local host probes support them.
On macOS, cgroup and administrative flags are false. On Linux, foreground/cgroup flags require the
delegated-root probe, while administrative additionally requires configured enablement plus a safe
`sudo -n` root/environment probe. Capabilities are evidence, never authority.

Ordinary tracing may contain stable IDs, phase, result kind, privilege, duration, counts,
truncation, cleanup, and a justified trace-only PID. It never contains raw command/cwd/cgroup paths,
environment, output, artifact bytes/paths, hostile OS strings, or credentials. Request/result Debug
redacts command, paths, and output.

Test-failpoint builds activate the architecture's after-spawn and after-direct-exit markers and add
operational-only abort markers during TERM/KILL and after cleanup. The in-memory registry does not
survive a process crash. systemd `KillMode=control-group` limits leaked execution after service death,
but only Stage 14 durable dispatch intent can classify ambiguity; Stage 13 writes no tool journal
event.

Stage 14 remains completely out of scope: no Tool Registry, authority evaluator, Tool Execution
Service, model-facing `run_shell`, durable dispatch/outcome, provider call, agent loop, or public
HTTP/WebSocket execute route is introduced.

## Verification split

macOS verifies Bash form, cwd/open-handle races, exact launcher allowlist and secret exclusion,
closed stdin/no TTY, independent bounded capture, binary projection, exit/signal/spawn distinctions,
registry/inspect/cancel races, caller drop, process-group descendant cleanup, timeout escalation,
shutdown admission/joins, capability truth, history preservation, and redaction.

Linux code and ignored target tests verify Ubuntu 24.04 x86-64, non-root `craxii`, Bash/Git, cgroup
v2 delegation, per-execution attachment, `cgroup.kill`, session escape, emptiness/removal stress,
reviewed `sudo -n`, root identity/environment, Docker, disposable systemd behavior, all four process
crash markers, service restart cleanup, and reboot leak scans. Actual execution of that suite is
`DEFERRED_BY_USER_TO_LATER_STAGE`; it is not a local implementation blocker and must not be claimed
as executed on macOS.
