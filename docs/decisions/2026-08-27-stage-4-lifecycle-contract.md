# Stage 4 Lifecycle Contract Decision

## Date

2026-08-27

## Status

Accepted

## Context / problem

Stage 4 must make Work, model, Tool, cancellation, terminal-output, and recovery legality
executable before persistence exists. The architecture defined the durable meaning but left some
exact reason/result/recovery literals, snapshot shape, and race-decision API details unnamed.
Without one frozen pure contract, later SQL transactions, schedulers, provider waits, and process
cleanup could encode conflicting lifecycle truth.

## Decision

### States and transitions

- `WorkState` is exactly `queued`, `running`, `waiting_on_model`, `waiting_on_tool`,
  `cancel_requested`, `completed`, `failed`, `cancelled`, and `interrupted`.
- `ModelInvocationState` is exactly `requesting`, `streaming`, `completed`, `failed`,
  `cancelled_locally`, and `provider_outcome_unknown`.
- `ToolExecutionState` is exactly `requested`, `dispatching`, `completed`,
  `interrupted_before_dispatch`, and `outcome_unknown`.
- The legal transition tables are the exact tables added to the V0 architecture. No self
  transition, terminal resurrection, or unlisted pair is legal.
- Transition decisions are immutable next snapshots plus semantic event/effect requirements. They
  perform no I/O.

### Work snapshot and ownership

- `WorkLifecycleSnapshot` is separate from immutable structural `WorkItem`.
- Only Work carries `ProjectionVersion`. Every actual transition increments once; no-op decisions
  do not increment; overflow is an internal invariant.
- `CurrentWorkAttempt` is exclusive: `none`, `model(ModelInvocationId)`, or
  `tool(ToolExecutionId)`.
- Queued Work has no owner/attempt; running Work has one owner/no attempt; waiting Work has one
  owner and the matching attempt; `cancel_requested` retains one owner and zero or one attempt;
  terminal Work clears both.
- Completion requires evidence that a completed terminal model invocation and immutable assistant
  Message are or will be committed in the same owning transaction and that cancellation has not
  won.

### Reasons, limits, and required effects

- Completion reasons are `answered` and `refused`.
- Cancellation reasons are `user_request` and `graceful_shutdown`.
- Interruption reasons are `runtime_ownership_lost`, `provider_outcome_unknown`,
  `tool_interrupted_before_dispatch`, `tool_outcome_unknown`, and `cleanup_unconfirmed`.
- Failure is a definite `NormalizedError`, `provider_exhausted`, invalid model output, or a
  `LifecycleLimit`.
- Limits are `context`, `model_attempts`, `agent_loop_steps`, `tool_calls`,
  `model_output_items`, `tool_argument_bytes`, `model_invocation_time`, and `total_work_time`.
- Final answer persistence must atomically insert the immutable assistant Message, append
  `assistant.message_committed`, move Work to `completed`, append `work.completed`, and clear owner
  and current attempt. Stage 4 returns that requirement and writes none of it.

### Model lifecycle and retry linkage

- The first valid provider event changes `requesting -> streaming` exactly once. Later deltas are
  no-op draft evidence and drafts remain ephemeral/noncanonical.
- `completed` means the complete normalized provider response has been durably observed by the
  future owning stage, not merely held in memory.
- A retry uses a new `ModelInvocationId`; preserves `LogicalInvocationId`, `WorkId`,
  `ContextManifestId`, and `AgentStepNo`; increments `AttemptNo` exactly once; and points `retry_of`
  to the immediate terminal predecessor.
- Duplicate identity/attempt number is a conflict. A predecessor remains terminal and no failed
  attempt is resurrected.
- Retry count, backoff, provider behavior, and provider wire payloads remain later-owned.

### Tool boundary, result, and cleanup

- A narrow `ToolLifecycleReference` is used before authority evaluation because Stage 3
  `ToolAttemptReference` includes authority evidence. It contains only lifecycle identity/linkage
  and can later be derived from the fuller reference; authority evidence is never fabricated.
- `requested` proves external side effects absent. `dispatching` means intent is durable and action
  may have crossed the boundary. `completed` means an observed terminal result is durably
  classifiable. `outcome_unknown` means dispatch/cleanup terminality cannot be proven and has no
  outbound path.
- Result classes are `success`, `validation_rejection`, `unknown_tool`, `authority_denial`,
  `file_error`, `process_exit`, `signal_termination`, `timeout`, `cancellation`, `spawn_failure`,
  and `cleanup_failure`.
- Cleanup status is `not_required`, `confirmed`, or `unconfirmed`.
- Pre-dispatch definite rejection/cancellation can complete with `not_required`. Timeout,
  cancellation, exit, signal, or cleanup-terminal results requiring cleanup complete only with
  `confirmed`. Any post-dispatch `unconfirmed` cleanup becomes `outcome_unknown`.
- There is no automatic Tool or recovery retry helper.

### Cancellation and precedence

- Queued cancellation goes directly to `cancelled`; running/model-wait/tool-wait cancellation goes
  to `cancel_requested`; repeated or terminal cancellation is a no-op with no version/event.
- First committed terminal Work decision wins. `cancel_requested` is nonterminal but blocks final
  answer, Tool dispatch, new model progression, and further loop progression.
- Completion committed first makes later cancellation a terminal no-op. Cancellation requested
  first makes later completion a conflict. Late provider/Tool evidence cannot authorize new action.
- Confirmed local model-wait cancellation becomes `cancelled_locally`; lost continuity becomes
  `provider_outcome_unknown` and interrupted Work.
- A recovered requested Tool becomes `interrupted_before_dispatch`. A dispatching Tool with
  confirmed cleanup can complete cancellation; unconfirmed cleanup becomes `outcome_unknown` and
  interrupted Work.
- The first `process_exit`, `timeout`, or `cancellation` execution-control latch wins; later
  observations cannot replace it. This is semantic only and observes no process or clock.

### Recovery and shutdown

- Recovery classifications are `retain_queued`, `already_terminal`, `interrupt_active_work`,
  `mark_model_provider_outcome_unknown_and_interrupt`,
  `mark_tool_interrupted_before_dispatch_and_interrupt`,
  `mark_tool_outcome_unknown_and_interrupt`,
  `reconcile_committed_tool_result_without_execution`, and `finalize_cancellation`.
- Old-runtime running Work with no current attempt is interrupted. Requesting/streaming models
  become provider-unknown plus interrupted. Requested Tools become interrupted-before-dispatch
  plus interrupted. Dispatching Tools become outcome-unknown plus interrupted.
- A consistent committed terminal Tool result may be reconciled without execution. Contradictory
  evidence is an invariant failure. Same-current-runtime use is a conflict, not recovery.
- A committed model response owned by an old runtime still interrupts Work. V0 does not synthesize
  an assistant Message or silently resume the loop.
- Model-visible uncertainty is only a semantic synthetic-status requirement here; no wording or
  Message content is fabricated.
- Recovery never calls adapters and never emits retry or dispatch.
- Graceful shutdown leaves queued Work queued, cancels active Work when child cleanup/absence is
  confirmed, and interrupts when cleanup or continuity is unconfirmed.

### Terminal output

- `TerminalDecision` distinguishes `answered`, `refused`, `continue_with_tools`, `failure`,
  `limit_reached`, and `cancel_wins`.
- Complete text-only or structured-only output answers; a sole refusal refuses; tools-only or
  text-plus-tools continues; incomplete, failed, empty, contradictory refusal, or unknown
  correctness-bearing output fails closed.
- Cancellation takes precedence over limits/output, and limits take precedence over otherwise
  renderable output.

### Lifecycle errors and failpoints

- Conflict kinds are `stale_state`, `stale_version`, `stale_owner`, `wrong_current_attempt`,
  `illegal_transition`, `duplicate_terminal_decision`, `duplicate_attempt_identity`, and
  `duplicate_attempt_number`; explicit projection is `state_conflict`.
- Invariant kinds are `invalid_state_shape`, `missing_required_evidence`, `version_overflow`,
  `unclassifiable_recovery`, `contradictory_projection`, and `impossible_terminal_shape`; explicit
  projection is `internal_invariant_error`.
- Display/Debug and normalized projection contain no raw content, provider, process, output, path,
  or Tool material. There is no blanket `DomainValidationError` conversion.
- The 14 reserved Stage 2 failpoints have a static semantic recovery map but no active hooks.
  `after_context_manifest_commit`, `after_model_intent_commit`, and
  `after_assistant_message_commit` remain distinct compatibility aliases whose precise precommit or
  post-whole-transaction physical hook must be selected by the later owning stage. They are not
  collapsed into a fictional partial commit.

## Rationale

Pure closed decisions make later persistence transactional code an executor of reviewed domain
truth instead of a second state-machine authority. Conservative unknown/interrupted outcomes avoid
inventing external success, duplicate side effects, or hidden restart progression. Keeping version
and ownership only on Work preserves child-attempt immutability and avoids competing projection
versions.

## Consequences / tradeoffs

- V0 interrupts some recoverable-looking committed model states instead of silently resuming.
- Conservative Tool dispatch classification can report unknown even when no process actually
  spawned.
- Exact enums require deliberate architecture changes for new lifecycle semantics.
- Pure evidence booleans/requirements must later be backed by atomic database constraints and
  transaction methods.
- The narrower Tool identity is a second type, but prevents premature/fabricated authority evidence.

## Rollback / change path

- Additive states/reasons require an architecture and migration/event compatibility decision.
- A later resumable recovery policy may add explicit deterministic reconciliation only after
  durable evidence and replay semantics are proven; it must not reinterpret existing unknown rows.
- Provider cancellation reconciliation or safe Tool deduplication can extend child decisions behind
  new evidence contracts without reopening existing terminal attempts.
- If a durable literal changes, version its storage/protocol interpretation rather than silently
  renaming existing history.

## Scope

This decision adds no persistence, migration, journal envelope, provider/process/workstation call,
filesystem operation, timer, scheduler/task ownership, failpoint activation, HTTP DTO, or client UI.
Those remain assigned to later implementation-plan stages.

## References

- [`docs/craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md)
- [`docs/craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md)
- [`backend/src/domain/lifecycle.rs`](../../backend/src/domain/lifecycle.rs)
