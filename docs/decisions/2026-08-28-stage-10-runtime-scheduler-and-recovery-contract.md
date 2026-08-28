# Stage 10 runtime, scheduler, and recovery contract

## Status and scope

Accepted for Craxii V0.0.01 Stage 10 on 2026-08-28. This record freezes process-lifetime
identity, runtime persistence, FIFO claims, owned asynchronous tasks, cancellation coordination,
graceful shutdown, deterministic startup recovery, readiness, and four crash failpoints. It
authorizes no Stage 11 HTTP/WebSocket surface, Stage 13/14 process or tool execution, or Stage 17
provider, agent-loop, assistant-message, or ordinary Work-completion behavior.

## No-migration decision

Stage 10 uses the existing V3 schema without alteration. It adds no migration `0004`, table,
index, foreign key, trigger, view, schema rebuild, or event kind. The schema ceiling remains `3`;
the repository retains exactly three migrations, 17 product tables, 40 named indexes, 28 journal
event kinds, and zero triggers/views. All migration bytes/checksums and the V1/V2/V3 structural
fingerprints remain frozen. Stage 10 is lifecycle behavior, strict event DTOs, narrow ports,
SQLite transactions over existing rows/indexes, application task ownership, composition, and
permanent tests.

## Runtime identity and lifecycle

Each successful process startup creates a fresh UUIDv7 `RuntimeInstanceId`; it is never reused and
is the only durable process-ownership identity. PID and Linux boot ID are bounded diagnostics and
never participate in ownership, liveness, or stale-runtime decisions. The exact lifecycle is
`new -> running -> stopping -> stopped(graceful_shutdown)`. A previous `running` or `stopping`
runtime found by a later process becomes `stopped(startup_failure)` after its owned state is
reconciled. Already stopped history is immutable.

On Linux the diagnostic boot ID is read from `/proc/sys/kernel/random/boot_id`. On non-Linux the
persisted value is the explicit bounded sentinel `non_linux_not_applicable`; it is not a fabricated
Linux or host identity. The diagnostic PID is the current positive process ID. Binary version,
Git revision, schema version, Craxii/workstation identity, workstation generation, and wall-clock
start time come from the already validated bootstrap evidence. No runtime event contains an
absolute path, secret, hostname, signal type, provider type, or tool wire type.

## Runtime events

All three existing runtime kinds retain journal payload version 1 but use distinct closed DTOs.

`runtime.started` contains exactly `runtime_instance_id`, `craxii_id`, `workstation_id`,
`workstation_generation`, `linux_boot_id`, `process_id`, `binary_version`, `git_revision`,
`schema_version`, and `started_at`. It is the first event in its runtime stream and exactly matches
the immutable runtime-row creation facts.

`runtime.recovery_performed` contains exactly `runtime_instance_id`,
`stale_runtimes_observed`, `stale_runtimes_closed`, `retained_queued_work`, `interrupted_work`,
`model_attempts_provider_outcome_unknown`, `model_attempts_terminal_preserved`,
`tool_attempts_interrupted_before_dispatch`, `tool_attempts_outcome_unknown`,
`tool_attempts_terminal_preserved`, `drafts_abandoned`, `orphan_artifacts_observed`,
`cleanup_checks_performed`, `cleanup_unconfirmed`, `recovery_duration_ms`, `binary_version`,
`schema_version`, and `recovered_at`. Counts and duration are nonnegative signed-64-bit-safe
integers. The event is emitted exactly once per successful current startup, including an all-zero
recovery, and is caused by that runtime's `runtime.started` event. V3 can prove stream identity,
ordering, version/binary/schema coherence, and current row consistency. The journal window after
the current `runtime.started` and before `runtime.recovery_performed`, together with stable attempt
rows, exactly reconstructs retained queued Work, Work interruptions, new and preserved model/tool
classifications, abandoned drafts, and cleanup checked/unconfirmed counts; those summary values
must match exactly. Stale runtimes observed/closed, orphan-artifact observations, and recovery
duration are observational-only V3 evidence and receive structural/type bounds, including
`stale_runtimes_closed <= stale_runtimes_observed`; they are not presented as independently
reconstructable history.

`runtime.stopping` contains exactly `runtime_instance_id`, `shutdown_requested_at`, bounded
reason `graceful_shutdown`, the persisted wall-clock `grace_deadline`, and deterministic
`active_work_count` and `active_task_count` snapshots. It is appended atomically with
`running -> stopping`, at most once, and is caused by the immediately prior runtime-stream event.
There is no `runtime.stopped` event.

## Startup order and failure handling

Startup completes configuration, tracing/health creation in `live_unready`, exclusive lock,
migrations, SQLite integrity, artifact initialization/integrity, canonical identity bootstrap,
and every Stage 3 through 9 consistency check before creating a runtime. Then one
`WriteCoordinator` plus `BEGIN IMMEDIATE` transaction inserts the current `running` runtime with
`last_heartbeat_at == started_at` and appends `runtime.started`. Recovery begins only after that
commit. No scheduler or heartbeat task starts before creation commits.

Recovery enumerates stale runtimes, reconciles their owned Work/attempts in bounded idempotent
units, closes each reconciled stale runtime as `stopped(startup_failure)`, and finally appends the
current runtime's recovery summary in its own transaction. Only after that commit may background
tasks start and readiness be evaluated. If any later bootstrap step fails, the original error is
preserved while a best-effort write marks the current runtime `stopped(startup_failure)`; failure
of that cleanup is left for the next startup.

The lifetime-exclusive process lock defines staleness. While it is held, every other runtime in
`running` or `stopping`, and every active Work owned by a noncurrent runtime, is stale. Heartbeat
age, PID liveness/reuse, boot ID, and distributed lease expiry are never used for that decision.

## Heartbeat and readiness

The owned heartbeat task runs every five seconds, updates only the current runtime while it is
`running`, never moves the stored time backwards, emits no journal event, and is explicitly
stopped and joined. Persistent storage failure transitions health to `fatal`, stops new claims,
and requests controlled shutdown; it never spins or silently retries forever. Fatal is terminal
health evidence and never transitions to `draining`, but the shutdown-controller latch remains
authoritative and still runs claim quiescence, stopping persistence when possible, cancellation,
task cleanup, and runtime closure. Cleanup failures do not mask the original fatal failure and
unclosed state remains stale for the next startup.

Health remains dependency-neutral with `live_unready`, `ready`, `draining`, and `fatal`. Stage 10
production composition has no real `WorkRunner`, so it deliberately remains `live_unready` even
after recovery and heartbeat startup. A test composition may install a scripted runner and mark
ready only after recovery, scheduler operation, and notification seams are proven. Stage 17 owns
the first real execution-readiness prerequisite; Stage 11 may only expose the read-only state.

## FIFO scheduler and task ownership

SQLite is queue truth. A notification is only a no-op-capable, lossy wakeup optimization; an
initial scan and a one-second fallback scan discover queued Work and committed cancellation even
when no hint is delivered. V0 selects queued Work by `conversation_work_ordinal ASC, work_id ASC`;
priority remains zero and never reorders. Terminal/cancelled/interrupted rows are ineligible, and a
conversation with `running`, `waiting_on_model`, `waiting_on_tool`, or `cancel_requested` Work is
excluded.

The claim transaction acquires `WriteCoordinator`, begins immediate, chooses the candidate,
rechecks absence of active sibling Work, reloads and validates state/version/owner/attempt links,
transitions `queued -> running`, assigns the current runtime, sets `started_at`, increments the
version, appends `work.started` caused by the immediately previous Work-stream event with the
current runtime actor/envelope, and commits. Only after commit does
`after_work_claim_commit` run, followed by registry insertion and runner spawn. Claim and Stage 9
cancellation use the same serialization discipline: cancellation-first yields direct queued
cancellation and no claim; claim-first yields `running` followed by `cancel_requested`.

The application scheduler owns its loop and every runner task in a `JoinSet` or equivalent joined
collection. Its registry is keyed by `WorkId` and holds only runtime ownership, cancellation
observation, and join association. Durable recovery truth never lives only in memory. Every normal
exit, start failure, panic, cancellation, timeout, and shutdown join is observed before registry
removal. A claimed Work whose runner cannot start, or whose runner exits abnormally while Work is
still active, becomes durably `interrupted`; it is never requeued. Persistent consistency/storage
failure sets health `fatal`, stops claims, and requests shutdown instead of retrying forever.

Claim admission uses an explicit asynchronous quiescence barrier. The shutdown controller closes
the admission latch and wakes the scheduler, then waits until no claim section is in flight. A
claim section covers candidate scan/claim through SQLite commit or no-claim, the
`after_work_claim_commit` boundary, and runner registration/spawn or durable start-failure
interruption. After the latch closes no later scan may call `claim_next_work`, while reconciliation
and draining of already-owned Work remain available.

`WorkRunner` is a narrow application boundary returning an owned future plus explicit
cancellation observation. Stage 10 production provides no implementation. Scripted permanent
tests may stop at a safe boundary and confirm `cancel_requested -> cancelled`; Stage 10 does not
call a provider, stream, tool, process, context assembler, assistant-message writer, or normal
completion path.

## Cancellation coordination

For a truly new Stage 9 message commit, the application reaches
`after_message_transaction_commit` before sending a scheduler hint or returning to its caller.
For a truly new active `cancel_requested` commit, it reaches
`after_cancel_requested_commit` before the cancellation hint or caller delivery. Replays, terminal
no-ops, and queued direct cancellation do not re-fire those postcommit windows. The persisted
client-command contract and canonical response hashes are unchanged; the existing
`CommandOutcome::Committed|Replayed` metadata is sufficient.

After active cancellation commits, a lossy hint wakes the scheduler and the matching registry
observer. The fallback scan reads current-runtime `cancel_requested` Work, so message delivery or
hint loss cannot strand cancellation. Durable database state is canonical. At a scripted safe
boundary with no external cleanup, the runner stops and the store appends `work.cancelled`,
preserving `started_at`, setting terminal time/reason, and clearing runtime/current attempt links.
Waiting-model provider cancellation belongs to Stage 17; waiting-tool/process cancellation belongs
to Stages 13/14. Stage 10 supplies coordination and conservative persistence only.

## Graceful shutdown

An application-level idempotent latch records the first request, wall-clock request time, one
wall-clock grace deadline derived from the existing `shutdown.grace_period_ms`, and one monotonic
in-memory deadline. Repeated requests neither append another `runtime.stopping` nor reset/extend
the deadline. Signal-specific types do not cross the composition edge.

The order is: latch; mark nonfatal health `draining` while preserving `fatal`; close claim
admission and await quiescence; atomically begin runtime stopping and append
`runtime.stopping`; reach `during_graceful_shutdown`; stop/join heartbeat; request/reconcile owned
Work cancellation; notify runners; and drain/join until the original deadline. At the deadline the
scheduler parent first freezes join reconciliation and acknowledges that boundary. The controller
then reloads and durably classifies unresolved Work/attempts conservatively; only after that commit
does the scheduler parent call `JoinSet::abort_all` (or equivalent), drain every child join result,
and clear each registry entry after its exit is observed. Expected cancelled joins after durable
classification create no second terminal transition. Parent-task abortion or dropping its
`JoinSet` is not the normal grace-timeout mechanism. Residual classification precedes
`stopped(graceful_shutdown)`; no cleanup is claimed without proof. Lifetime lock release remains
owned by its guard.

OS composition uses Tokio's existing `signal` feature only: portable Ctrl-C plus SIGTERM on Unix
feed the application shutdown seam. No new direct dependency, `tokio-util`, or `futures` crate is
introduced. `craxii-admin` remains an offline path and creates no runtime, recovery, heartbeat, or
scheduler.

## Recovery matrices and transaction granularity

Runtime creation is one transaction. Each stale Work recovery and stale-runtime closure is a
bounded idempotent transaction; no transaction spans all startup recovery. The final summary is a
separate transaction. Queued Work remains queued with no owner/event. Stale `running` Work becomes
terminal `interrupted(runtime_ownership_lost)` and is never requeued.

For `waiting_on_model`, `requesting` or `streaming` becomes
`provider_outcome_unknown` before Work becomes interrupted in the same transaction. Existing
`provider_outcome_unknown` is preserved; completed/failed/cancelled-local attempts remain terminal
while stale Work is interrupted. Recovery makes no provider call and never manufactures a
completed agent continuation.

For `waiting_on_tool`, `requested` becomes `interrupted_before_dispatch`; `dispatching` becomes
`outcome_unknown` with cleanup unconfirmed; either attempt event precedes the resulting
`work.interrupted`. Existing interrupted/unknown classifications are preserved. A completed tool
result is never executed again; because Stage 14/17 continuation is absent, ordinary stale waiting
Work is safely interrupted rather than emitting `work.resumed`.

For `cancel_requested`: no attempt and no cleanup ambiguity becomes cancelled; model requesting/
streaming becomes provider-outcome-unknown plus interrupted; existing model unknown becomes
interrupted; another terminal model attempt with confirmed/no-required cleanup becomes cancelled;
otherwise cleanup uncertainty interrupts. Tool requested becomes interrupted-before-dispatch plus
interrupted; dispatching becomes outcome-unknown plus interrupted; existing predispatch/unknown is
preserved plus interrupted; completed with confirmed/no-required cleanup becomes cancelled; and
completed with uncertainty becomes interrupted. Current model/tool attempt links are valid in
`cancel_requested` when their runtime matches Work ownership; zero or one exclusive link is legal.

Recovery never automatically retries or returns to queued: provider outcome unknown, tool outcome
unknown, stale tool dispatch, interrupted non-idempotent effects, stale running Work, and stale
waiting states are terminal from the scheduler's perspective. Attempt-classification events commit
before and cause the corresponding Work interruption where applicable. Recovery performs no
external cleanup and reports that honestly.

## Runtime/projector consistency

Startup enforces queued Work without owner; every active Work with an existing runtime owner;
matching current attempt/Work/runtime identity; the exact state-to-attempt-link rules including
valid cancel-requested links; terminal owner/link shape; and no two active Work items in one
conversation. Runtime started/stopping payloads exactly agree with their row facts and stream
history. Recovery summaries are checked exactly for the journal/attempt-row-derivable counters
named in the runtime-event contract and structurally for the explicitly observational counters.
Contradictions fail closed; neither rows nor journal are repaired from the other.

Claims use `ix_work_items_queued_fifo`; active/runtime recovery uses
`ix_work_items_nonterminal_by_runtime`; model/tool recovery uses their existing runtime-nonterminal
indexes. Stage 10 adds no index and permanently checks the plans.

## Failpoint activation

Exactly four already registered Stage 2 names become active, development/test only:
`after_message_transaction_commit`, `after_work_claim_commit`,
`after_cancel_requested_commit`, and `during_graceful_shutdown`. The first three run immediately
after their named new commit and before notification/caller delivery or runner registration. The
shutdown point runs after claim admission closes, the in-flight claim section quiesces, and
`runtime.stopping` commits, but before drain/classification. Release compilation retains the
existing failpoint guard; no new public name is created. Subprocess tests use abrupt process loss
and the same file-backed state to prove replay,
fallback scanning, stale-owner interruption, cancellation convergence, stopping-runtime recovery,
and idempotent recovery.

## Tradeoffs and rollback path

The exclusive lock deliberately makes staleness deterministic for one-host V0 but is not a
distributed lease. Five-second heartbeat and one-second scan intervals favor simple bounded
observation over tunable policy; changing them is a durable operational contract amendment.
Production remains unready because claiming without a real runner would falsely advertise a
working service. V3 cannot prove every recovery-summary count after mutations, so validation is
strict only where durable evidence remains sound instead of inventing an unbacked audit table.

Rollback is code-only: stop the binary and deploy the prior Stage 9 binary after ensuring no Stage
10 runtime is active. V3 remains readable because no schema or event-kind inventory changed, but a
prior binary that does not understand event-specific runtime V1 payloads must not be used on a
database containing Stage 10 runtime events without an explicit compatibility decision. No
automatic data rewrite or migration rollback is authorized.

## Deferred ownership

- Stage 11 exposes read-only liveness/readiness and command/event transport; it does not own runtime
  semantics.
- Stages 13/14 own Linux child processes, cgroups/process groups, tool registry/dispatch, and proven
  cleanup.
- Stage 17 owns providers, context assembly, the real agent loop/`WorkRunner`, assistant output,
  normal Work completion, and provider-aware live cancellation.
