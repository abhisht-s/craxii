# Stage 17 model gateway, agent loop, and runtime readiness contract

Date: 2026-09-01

Status: accepted for Craxii V0.0.01 Stage 17

## Decision

`ModelGateway` is the provider-neutral application service that owns physical attempt identity,
durable intent before provider I/O, canonical stream accumulation, usage and provider identifiers,
retry/backoff, deadline and cancellation observation, outcome certainty, and durable attempt
terminalization. It does not select a model, assemble context, execute tools, build the assistant
Message, schedule Work, expose public transport, or contain provider wire/HTTP types.

One `model_invocations` row represents one physical provider attempt. A shared
`logical_invocation_id` groups the initial attempt and retries. Attempt one atomically commits the
exact Stage 16 manifest, invocation intent, Work wait transition, and journal evidence before any
provider I/O. Every retry reuses the exact logical request and manifest, creates a new linked row,
and records typed predecessor, reason, chosen delay, and bounded provider Retry-After evidence.

The retry ceiling is three physical attempts: initial plus at most two retries. Automatic retry is
allowed only for `RateLimited`, `TemporarilyUnavailable`, `TransportBeforeResponse`, or
`TimeoutBeforeOutput`, with certainty `DefinitelyNotSent` or `DefiniteProviderFailure`, before any
semantic output or draft exposure, while cancellation, shutdown, absolute Work deadline, and both
logical/Work attempt budgets permit it. Full jitter uses a 250 ms base, a 5 second local cap, and
provider Retry-After capped at 30 seconds; cancellation and the absolute deadline interrupt
backoff.

`Authentication`, `Authorization`, `InvalidRequest`, `UnknownModel`,
`TransportAfterPossibleProcessing`, `TimeoutAfterOutput`, `MalformedResponse`,
`MalformedCompletedToolArguments`, `OutputTooLarge`, `UnsupportedResponseItem`, `ContextError`,
`SafetyRefusal`, `Cancelled`, `ProviderOutcomeUnknown`, `InternalProviderError`, `ScriptMismatch`,
and `InvalidScriptProgram` are never automatically retried. Semantic output begins with text,
reasoning-summary, tool-start, tool-argument, completed-tool, refusal, structured-data, or unknown
correctness-bearing evidence. Response metadata, identifiers, usage-only/unavailable events,
terminal markers, and transport bytes alone do not cross the cutoff.

V4 makes attempt evidence explicit with typed usage status, provider error kind, provider outcome
certainty, retry reason/delay/provider guidance, and billing ambiguity. Known usage may be recorded
for completed, failed, locally cancelled, or provider-unknown attempts. Non-completed attempts
never receive fabricated normalized output. Attempt one has no retry metadata; a retry has one
valid predecessor and bounded nonnegative evidence. Terminal state, certainty, error, usage, and
billing ambiguity must agree.

Provider request hashes and request/response IDs are evidence, not idempotency keys. If processing
may have occurred and absence or reconstructability cannot be proved—including transport after
possible processing, provider-outcome-unknown, process loss after durable requesting/streaming,
lost semantic deltas, or a terminal provider result not durably committed—Craxii does not retry.
The attempt becomes `provider_outcome_unknown`, `model.invocation_interrupted` is appended, Work
becomes `interrupted(provider_outcome_unknown)`, the draft is abandoned, and neither tools nor an
assistant Message are produced. A durably terminal attempt is never called again. Stage 17 does
not auto-resume an uncertain in-flight loop at startup.

`AgentLoop` is the sole real `WorkRunner`. It is iterative, reloads exact ownership at checkpoints,
reselects the model and assembles fresh Stage 16 context after every complete tool batch, and uses
only `ModelGateway` and `ToolExecutionService` for effects. Limits are 16 model steps, 32 provider
attempts per Work, 32 tool calls, 64 ordered response items, 64 KiB raw arguments per call, five
minutes per provider invocation, 60 seconds stream idle, and 30 minutes total Work duration
excluding queue time. Tool calls are validated as a whole batch and run sequentially in provider
order; parallel tools are disabled. An observed ordinary tool error remains a durable result and
the batch continues. `tool_outcome_unknown` stops the batch immediately and interrupts Work.

A final answer or refusal is authoritative only after one SQLite transaction verifies the exact
owned Work and completed model attempt, proves cancellation has not won, inserts one immutable
assistant Message with `produced_by_work_id`, appends `assistant.message_committed` caused by the
model terminal event, completes Work as `answered` or `refused`, appends `work.completed`, and
clears ownership/current-attempt/cancellation fields. No partial stream or text mixed with tools is
canonical conversation history.

Cancellation distinguishes requested, observed, and confirmed cleanup/outcome. Before durable
intent there are zero provider calls. A definitely-unsent live attempt may become
`cancelled_locally` and confirm Work cancellation; uncertainty becomes provider unknown plus Work
interruption. Late provider output cannot beat durable cancellation. Tool cancellation uses Stage
14 process-tree cleanup truth. One absolute Work deadline is created when running begins and never
reset; provider and tool local limits only narrow it.

Readiness has two explicit compositions. A deterministic Stage 17 integration composition may
become ready only after bootstrap, recovery, dependency construction, real `AgentLoop`
installation, scheduler start, and a successful initial scheduler scan. The production binary
remains `live_unready` because Stage 17 has no live provider and must never compose
`ScriptedProvider` as one. Stage 19 supplies and proves the live provider before production can be
`live_ready`; lack of its credentials does not block Stages 20–26. Once ready, a critical owned-task
or storage invariant is fatal rather than a fallback to `live_unready`.

Repository policy uses only narrow, direct Stage 17 boundary checks. Behavioral tests and durable
database invariants are authoritative; arbitrary semantic static analysis or mutation
checker-whack-a-mole is not required.

## Consequences

- Migration V4 is required and the maximum supported schema becomes 4.
- `ScriptedProvider` remains deterministic and test-only; production has no OpenAI, Reqwest, SDK,
  WebSocket draft, SSE, or public model/tool endpoint in Stage 17.
- Startup recovery conservatively interrupts every old non-resumable loop checkpoint before
  readiness. Stage 18 deepens crash-window testing but does not change the Stage 17 rule.
- Model-output manifest sources use the frozen invocation item/continuation locators defined in the
  architecture so a post-tool context can persist and reconstruct exact Stage 16 provenance.
- Stage 19 live-provider proof is the dependency for production readiness and may be deferred when
  credentials are unavailable without blocking later local stages.
