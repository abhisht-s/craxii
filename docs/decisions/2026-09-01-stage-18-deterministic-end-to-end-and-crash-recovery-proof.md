# Stage 18 deterministic end-to-end and crash/recovery proof

Date: 2026-09-01

Status: accepted for Craxii V0.0.01 Stage 18

## Decision

Stage 18 freezes one reusable deterministic composition that starts at the authenticated HTTP
command boundary and crosses real command persistence, file-backed SQLite WAL, scheduler ownership,
`AgentLoop`, target selection, Stage 16 context assembly, `ModelGateway`, test-only
`ScriptedProvider`, `ToolExecutionService`, `LocalWorkstation`, assistant completion, bootstrap, and
durable replay. Tests use real workspace and artifact directories and a fresh runtime identity on
every start. The composition does not create a second command path or a production provider.

Process-loss claims use a parent/child controller. The child reaches a feature-gated durable
boundary, emits one bounded marker, and blocks; the parent sends `SIGKILL`, reaps it, and creates a
new runtime over the same files. No scheduler, provider program, cancellation token, health state,
runtime identity, or task survives in memory. Startup classifies stale Work before readiness and
never automatically resumes an old loop, reissues an ambiguous provider call, or repeats an
uncertain workstation effect.

The seven Stage 18 architecture failpoints are attached to their real physical boundaries: model
manifest rows before intent, all model rows before commit, committed intent before provider I/O,
first semantic provider delta, committed model terminal before loop interpretation, final-answer
rows before commit, and committed final answer before notification. Existing Stage 10 and Stage 14
failpoints cover Work claim and tool request, dispatch, spawn, and terminal-result classes. All
failpoints remain excluded unless the `test-failpoints` feature is enabled.

A test-only provider wrapper syncs a redacted JSON-lines ledger before delegation. Each record keeps
only Work ID, logical invocation ID, physical attempt, and request hash. It is the authority for
zero, one, retry, no-reissue, and follower-invocation call counts. A disposable workspace marker
provides the tool-effect authority. Tool evidence distinguishes definitely unrequested dispatch,
dispatch uncertainty, and a definite terminal result without blind retry.

The versioned `stage18-v1` evidence fixture contains stable semantic facts only. UUID values,
timestamps, runtime IDs, process IDs, latency, and temporary roots are omitted or symbolically
normalized. Ordering, state transitions, request hashes, semantic output, counts, causal links,
artifact hashes, and failure classifications remain material. Two empty-state executions must
produce identical normalized evidence, and a cold reopen must preserve the same durable meaning.
Behavioral runtime tests and direct durable evidence are authoritative; repository checks enforce
only the simple existence and feature-gating boundaries.

Ubuntu 24.04 x86-64 target assertions live in `scripts/verify-stage18-deterministic`. When that
target is locally present, the script also checks machine facts, cgroup v2 visibility, process
cleanup, filesystem behavior, and the deterministic benchmark. On any other host the portable
suite still runs, target success is not claimed, and execution is reported exactly as deferred by
the user to Stage 27 or earlier.

## Consequences

- Stage 18 adds no migration or dependency; the schema ceiling remains V4.
- Production remains `live_unready`, with no OpenAI, Reqwest, live provider, parallel tools, or
  client draft protocol.
- A shutdown race exposed by the harness is fixed by recording durable Work cancellation before
  workstation-wide cleanup and by giving observed cancellation priority over a simultaneously
  ready tool completion.
- Finalized unreferenced artifacts are safe orphans. Missing or corrupt referenced artifacts and
  journal/projection inconsistency block readiness.
- Stage 18 proves recovery classification and replay, not automatic in-flight loop continuation,
  backup/restore, snapshotting, or distributed orchestration.
