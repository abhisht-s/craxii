# Architecture review: Craxii V0.0.01

## Executive verdict

I would approve the overall direction, but I would not approve the source-of-truth document unchanged for implementation.

This is not a disposable chatbot architecture. The central choices—Craxii-owned agent loop, durable history outside model state, native client, Linux workstation, explicit tools, provider adapters, and a deliberately boring single-host deployment—point toward the intended V1.

However, six boundaries need correction before code exists:

1. Craxii identity, conversation, user message, and durable work must be separate concepts.
2. The journal envelope needs causal, versioning, and replay fields beyond `conversation_id + sequence_no + parent_event_id`.
3. Queued messages need explicit causal visibility; otherwise the active turn can accidentally consume them.
4. Model selection and context assembly are ordered inconsistently, and the proposed response type is too simplistic.
5. Shell execution must not inherit the backend’s database, credentials, OS identity, and process privileges.
6. Cancellation, interrupted-work recovery, and replayable client commands are minimum V0 semantics, not optional polish.

The right response is a targeted architectural amendment, not a different stack or a larger distributed system.

I reviewed the complete [V0.0.01 source-of-truth document](../../CRAXII_V0.0.01_DEEP_ARCHITECTURE_SOURCE_OF_TRUTH.md) and the separate [identity and credential architecture](../craxii-identity-credential-architecture.md).

---

## 1. Product-architecture fit

### What fits V1 well

The following decisions are strong foundations:

- Craxii, rather than a provider session, owns identity and continuity.
- The context window is treated as a rendered view.
- The agent loop is explicit and inspectable.
- Tools are requests that Craxii validates and executes.
- The native client is a view into backend state.
- The workstation is treated as a body, not the identity itself.
- Complex memory, distributed workers, and production authority are consciously deferred.

Those are precisely the decisions that prevent this from collapsing into a chat wrapper.

### Where the current design still encodes chatbot assumptions

The primary problem is that “conversation turn” is doing too much work. It currently acts as:

- an input grouping;
- the scheduling unit;
- the failure unit;
- the concurrency lock;
- the model-context boundary;
- and implicitly the unit of responsibility.

A mature coworker needs an internal durable work concept independent of the conversational surface. Call it `work_item`, `responsibility`, or `task`; the name matters less than the separation:

```text
Craxii identity
  ├── conversations / client surfaces
  ├── durable work items
  │     ├── input events
  │     ├── attempts
  │     ├── model invocations
  │     └── tool executions
  └── workspaces
```

A user message may create a work item. Later, a scheduled event, webhook, recovery action, or background responsibility may create one without a user message. This is not multi-agent orchestration; it is the minimum durable work model.

The schema also lacks an immutable `craxii_id`. The durable identity described in the credential architecture should exist now as a simple database principal, even though its future control plane and cryptographic identity are deferred. Otherwise the “default conversation” risks becoming Craxii’s de facto identity.

### Product-fit verdict

V0.0.01 is a good first slice if conversation remains a user-facing channel and durable work becomes the internal execution primitive. Without that correction, later background work and multiple clients will require a substantial rewrite of scheduling, recovery, context, and history semantics.

---

## 2. Backend architecture

| Choice | Verdict | Review |
|---|---|---|
| Rust | Approve | Appropriate for a durable process supervisor, typed protocols, provider adapters, and process execution. The additional implementation effort is justified for this product. |
| Tokio | Approve | Correct for concurrent sockets, streaming HTTP, timers, subprocess I/O, and cancellation. All spawned tasks need explicit ownership; detached tasks should be treated as bugs. |
| Axum | Approve | A good thin transport layer. Keep agent semantics out of handlers. Axum already integrates with Tokio, Hyper, and Tower rather than inventing its own middleware model. [Axum documentation](https://docs.rs/axum/latest/axum/index.html) |
| Tower / tower-http | Approve selectively | Use for authentication, request IDs, body limits, tracing, and ordinary HTTP timeouts. Do not apply generic request timeouts blindly to WebSockets or long-lived streams. |
| Hyper | Do not treat as a separate architectural subsystem | It is the HTTP foundation beneath Axum. Depend on it directly only when a concrete low-level need appears. |
| Reqwest | Approve | Suitable as provider HTTP transport. Configure connect, response-idle, and overall invocation deadlines explicitly. Keep retry policy outside the raw transport. |
| Serde / JSON | Approve | Correct for protocol and journal boundaries. Do not let `serde_json::Value` replace typed domain structures internally. |
| tracing | Approve | Correct diagnostic foundation. Use structured fields and strict redaction. Traces are not historical product events. |
| SQLx | Approve | Appropriate for migrations and typed database access. SQLite write behavior still needs to be designed explicitly rather than delegated to a large connection pool. |

### Module boundaries

The proposed layout is broadly good, but I would introduce a small stable domain layer:

```text
domain/
  ids
  content
  events
  work
  model
  tools
```

The API, SQLite repository, OpenAI adapter, and executor should translate to and from those domain types. Avoid traits for every module; use abstraction primarily at genuine boundaries:

- model provider;
- tool executor;
- storage transaction/repository where testing benefits;
- client protocol.

### Single-process runtime

One Rust control process is appropriate for API, scheduling, context, models, and persistence.

It should not imply that shell commands run with the same OS authority. I recommend one control process plus a minimal local executor boundary running commands under a different Unix principal and cgroup. That is still a single-machine architecture and not a worker fleet.

### systemd

Systemd is the right supervisor. The backend unit should have:

- a non-login, non-root server user;
- a narrowly writable state directory;
- no ambient capabilities;
- `NoNewPrivileges`;
- controlled shutdown;
- cgroup-wide child cleanup;
- bounded restart behavior;
- a startup recovery pass.

The executor needs a separate, deliberately wider filesystem profile.

---

## 3. Cloud and workstation architecture

### AWS and EC2

AWS is a reasonable choice, especially because the long-term credential architecture already anticipates AWS identity, KMS, Secrets Manager, STS, and instance attestation. Switching to a cheaper VM provider would reduce cost but would not solve a meaningful V0 architectural problem.

EC2 is a much better fit than serverless compute, ECS tasks, or short-lived application platforms because Craxii needs a persistent, engineer-like Linux environment.

### Ubuntu 24.04 and x86-64

Both are appropriate:

- Ubuntu 24.04 LTS provides a conventional engineering environment.
- x86-64 maximizes compatibility with downloaded binaries and developer tooling.
- Graviton/ARM could reduce cost, but compatibility surprises are an unnecessary V0 variable.

Architecture should still be recorded as a workstation capability, not hard-coded into model or tool logic.

### EBS

EBS is suitable for V0 persistence, but the current document understates the difference between persistent storage and recoverable identity. EBS is not automatically backed up; AWS explicitly makes scheduled snapshots the customer’s responsibility. [AWS EBS snapshot documentation](https://docs.aws.amazon.com/ebs/latest/userguide/ebs-snapshots.html)

Before implementation, define at least:

- encrypted EBS;
- `DeleteOnTermination=false` for durable data volumes;
- automated snapshot retention;
- a manual restore test;
- which state is durable, recoverable, regenerable, or ephemeral.

I recommend separate paths, and preferably separate logical volumes:

```text
/opt/craxii/                         versioned server binaries
/etc/craxii/                         non-secret configuration
/var/lib/craxii/                     journal and server-owned state
/var/lib/craxii/artifacts/           local artifact/blob store
/srv/craxii/workspaces/<id>/         executor-owned project files
/var/cache/craxii/                   regenerable caches
/run/craxii/                         sockets and ephemeral runtime state
```

Do not use one `/home/craxii` hierarchy for server state, credentials, and model-controlled project files.

For live SQLite backups, use SQLite’s backup mechanism or capture the database and WAL together. SQLite considers the WAL part of persistent database state; copying the `.db` file alone can lose committed transactions or corrupt the copy. [SQLite WAL documentation](https://www2.sqlite.org/wal.html)

### Machine replacement seam

Add durable `workstation_id`, `workspace_id`, and workstation generation metadata. Tool history should refer to logical workspace paths plus workspace identity, not assume that `/home/craxii/projects` is eternally meaningful.

No automated machine replacement is needed in V0.0.01, but a clean separation between server state, workspace data, caches, binaries, and credentials is needed now.

### Deployment

Copying a release binary and restarting systemd is fine. Make the operation atomic and reversible:

- upload to a versioned path;
- verify a checksum;
- switch the active version;
- restart;
- retain one previous version;
- run migration compatibility checks before serving traffic.

No CI/CD platform is required yet.

---

## 4. Journal and state architecture

### Are we storing the right primitive?

Almost.

An immutable event is the right historical primitive. A conversation-scoped event containing generic JSON is not yet a sufficient durable substrate for Craxii’s full future.

The current journal schema needs the following changes:

```text
journal_offset        local monotonically increasing replay cursor
event_id              globally unique immutable ID
craxii_id
stream_id
stream_seq
event_type
event_version
conversation_id NULL
work_id NULL
causation_event_id NULL
correlation_id NULL
actor_kind
actor_id NULL
payload_json
recorded_at
```

Important differences:

- `journal_offset` supports reconnect replay across all server events.
- `stream_id + stream_seq` supports deterministic ordering within an aggregate.
- `conversation_id` must be nullable because future work may not originate in a conversation.
- `work_id` makes responsibility first-class.
- `event_version` makes JSON payload evolution explicit.
- `causation_event_id` and `correlation_id` replace ambiguous `parent_event_id`.
- `craxii_id` prevents conversation from becoming identity.
- `actor_kind + actor_id` is more expressive than a single `actor` string.

A hash chain or WORM storage is not required in V0.

### Events plus mutable state

Do not pursue event-sourcing purity. Use append-only evidence plus explicit current-state tables updated atomically:

- `craxii_principals`
- `conversations`
- `client_commands`
- `work_items`
- `model_invocations`
- `tool_executions`
- `artifacts`
- `stream_heads`

For example, one transaction should:

1. validate the client idempotency key;
2. append the user-message event;
3. create the queued work item;
4. append the work-queued event;
5. record the command response mapping.

That prevents a crash from leaving a durable message with no scheduled responsibility.

### Event taxonomy

The existing taxonomy lacks enough lifecycle precision. A minimal taxonomy should distinguish:

```text
conversation.created
message.accepted

work.queued
work.started
work.cancel_requested
work.cancelled
work.completed
work.failed
work.interrupted

model.invocation_started
model.invocation_completed
model.invocation_failed

tool.requested
tool.started
tool.completed

assistant.message_committed
runtime.recovery_performed
```

Not every diagnostic detail has to be a journal event. Attempt rows can hold mutable operational state, while the journal records meaningful historical transitions.

### Model invocations

The proposed table is a good start. Add:

- `attempt_no`;
- `retry_of_invocation_id`;
- model-selection reason;
- model/config capability version;
- context-manifest hash;
- input byte/token estimate;
- first-token latency;
- provider stop reason;
- request and response artifact references;
- whether output had already been exposed to a client;
- normalized terminal/error classification.

Create the invocation row before making the provider request, then update it on completion or failure.

A provider request snapshot is diagnostic evidence, not canonical conversation history. Store a redacted canonical request or artifact reference plus the exact source-event manifest used to build it.

### Tool executions

Tool executions need a symmetrical table. `run_shell` may mutate files, launch processes, or call external systems. After a crash, “requested but no result recorded” means outcome unknown—not “safe to retry.”

### SQLite configuration

SQLite in WAL mode is appropriate for one server. WAL permits concurrent readers but still has only one writer. [SQLite WAL concurrency documentation](https://www2.sqlite.org/wal.html)

For V0 I would choose:

```text
journal_mode = WAL
synchronous = FULL
foreign_keys = ON
busy_timeout = explicit
small connection pool
short transactions
```

The journal is Craxii’s continuity substrate, and its write rate is low. SQLite documents that WAL with `synchronous=NORMAL` can lose a committed transaction after an OS crash or power failure, whereas `FULL` adds a WAL sync per commit. [SQLite synchronous documentation](https://www.sqlite.org/pragma.html)

Sequence allocation must be transactional. Do not use an unprotected `MAX(sequence_no) + 1`.

### Idempotency

`client_message_id UNIQUE` is incomplete. Store:

- device identity;
- client message/idempotency key;
- request payload hash;
- resulting message ID;
- resulting work ID;
- original HTTP response.

The same key and same payload returns the original result. The same key with different content is a conflict.

### Artifacts and raw evidence

Separate:

- capture limit;
- journal inline limit;
- client display limit;
- model-context limit.

A command may capture several megabytes into a server-owned local artifact while returning a small head/tail projection to the model. Store hash, size, MIME type, truncation status, and producing tool execution ID.

This provides the future S3 seam without requiring S3 now.

### Journal verdict

With these changes, SQLite plus append-only evidence is sufficient for early memory, search, compaction, work sessions, multiple clients, provenance, and recovery. Without them, conversation scope and ambiguous causality will become high-cost migrations.

---

## 5. Context architecture

### Is naive full history correct?

Yes—as an experiment and first implementation strategy.

It gives you:

- a correctness baseline;
- measurable context growth;
- simple restart reconstruction;
- evidence for later compaction and retrieval design.

But “full history” must mean:

> Include every eligible canonical event that fits the selected model’s budget.

It cannot mean “blindly send until the provider rejects the request.” V0 should measure serialized size and estimated tokens, and fail honestly with `context_limit_exceeded` if the full-history policy no longer fits. Do not implement compaction yet.

### Critical causal bug

The document says that a second message is persisted while a turn is active, while the context assembler reads full reconstructed history. Those two rules can cause the active turn to consume the queued message accidentally.

Each message must be assigned atomically to a work item. Context for work item N includes:

- terminal prior work items;
- input events assigned to N;
- events produced by N;
- explicitly steered input targeted to N, once steering exists.

It excludes inputs assigned to later queued work. A raw “all events up to the latest sequence number” filter is insufficient because later queued inputs and earlier work completion events can interleave.

Persist an invocation-specific context manifest listing the exact event and artifact IDs included.

### Selection/context ordering

The document currently assembles context and then selects a model, while the assembler contract requires a `model_profile`. That is circular.

Use:

```text
work state + requested capabilities
        ↓
InvocationIntent / ContextStats
        ↓
model selection
        ↓
selected ModelTarget
        ↓
context assembly for that model’s budget and formats
        ↓
canonical ModelRequest
        ↓
provider adapter
```

Future routing may require a cheap first pass that measures context before selecting a model. Final rendering still occurs after selection.

### Better assembler contract

Conceptually:

```text
assemble(
  craxii_id,
  work_id,
  selected_model_target,
  context_policy,
  eligible_input_manifest
) -> ContextPackage
```

`ContextPackage` should contain:

- ordered canonical content items;
- tool definitions;
- system-instruction version;
- source event/artifact IDs;
- byte and token estimates;
- omissions and truncation reasons;
- assembler version/hash.

Memory and retrieval remain separate durable projections. The assembler consumes them later; it does not own them.

Also ensure the current user message is not duplicated by appearing once in reconstructed history and once again as a separate “current turn.”

---

## 6. Multi-model architecture

### Overall split

The conceptual split is correct:

- common internal semantics;
- capability/configuration data;
- selection policy;
- provider adapters;
- provider-native escape hatches.

The risk is implementing a grand provider abstraction before seeing a second provider.

### V0 implementation I recommend

Use one small provider interface and one configured OpenAI target:

```text
ModelTarget
  provider
  model_id
  endpoint/account configuration
  supported capabilities
  provider-specific typed options
```

Selection policy in V0:

```text
explicit target if configured and capable
otherwise configured default
```

The capability registry can be a static typed configuration structure. It does not yet need dynamic discovery, pricing logic, learned routing, or a standalone registry service.

Add a deterministic scripted provider used in tests. That validates the runtime/provider boundary without pretending it proves Anthropic or Gemini compatibility.

### Canonical response shape

The current mutually exclusive shape:

```text
FinalText
ToolCalls
Error
```

is too narrow. Provider responses can contain ordered mixtures of text, tool calls, refusals, structured data, and provider-native continuation items.

Prefer:

```text
ModelResponse {
  output_items: Vec<ModelOutputItem>,
  stop_reason,
  usage,
  provider_continuation,
  provider_metadata,
}
```

The runtime decides whether the response is terminal after processing its output items.

Provider-native opaque continuation data should be kept inside the adapter boundary and associated with the invocation. It may be reused within a same-provider tool loop, but it must never become the only durable conversation state.

### OpenAI adapter

The Responses API is an appropriate first adapter because it supports streaming responses and custom function calls. [Official OpenAI Responses API documentation](https://developers.openai.com/api/reference/typescript/resources/beta/subresources/responses/methods/create)

For V0:

- explicitly choose the provider’s storage behavior; I recommend `store: false` unless there is a conscious retention reason;
- do not use provider conversation state for correctness;
- preserve provider response IDs and output items;
- keep authentication and wire types inside `openai.rs`;
- set parallel tool calls off initially, or define deterministic sequential handling;
- keep provider error classification in the adapter, but retry decisions in the runtime;
- persist each retry as a separate invocation attempt.

Routing belongs at the invocation boundary, before final context rendering—not in the API layer, conversation layer, or provider adapter.

---

## 7. Tool architecture

### Registry and schemas

Registry → dispatcher → implementation is a good shape.

Use typed `Deserialize` input structures and derive or test the JSON schemas against them. Provider schema conformance is never authorization and is not sufficient runtime validation.

Tool identity should include a schema/implementation version.

### Change dispatcher ownership

The current document gives the dispatcher responsibility for journal creation. That blurs orchestration and execution.

Use:

```text
Agent Runtime
    ↓
Tool Execution Service
    ├── persist requested intent
    ├── policy/authority decision
    ├── validation
    ├── registry lookup
    ├── dispatcher/executor call
    └── persist terminal outcome
```

The registry and tool implementation should not write directly to the journal. This guarantees the important order:

```text
persist intent
    ↓
perform side effect
    ↓
persist observed outcome
```

### Tool request envelope

The model should provide only ordinary arguments. Craxii injects authority-bearing context:

```text
tool_call_id
work_id
tool_name
tool_version
validated arguments
workspace_id
logical cwd
deadline
output policy
authority/policy context
```

The model should not select arbitrary workspace identities or credential scopes by passing hidden arguments.

### `read_file`

Good first structured tool. It should return:

- logical and resolved path;
- file size and modification metadata;
- encoding;
- content or artifact reference;
- truncation;
- structured error.

Canonicalize symlinks and define allowed path behavior, while recognizing that path restrictions do not sandbox an unrestricted shell.

### `run_shell`

A shell string is defensible because real engineering relies on pipes, redirects, and compound commands. Define its semantics precisely, for example a non-interactive shell without profile loading.

It needs:

- explicit logical `cwd`;
- a sanitized allowlisted environment;
- no inherited server credentials or open file descriptors;
- timeout;
- concurrent stdout/stderr draining;
- capture and inline caps;
- exit code and terminating signal;
- process group or cgroup ownership;
- cancellation that kills descendants and reaps them;
- no surviving background jobs in V0.

Tokio child processes continue running by default when the handle is dropped, so cancellation cannot rely on dropping a future. [Tokio process documentation](https://docs.rs/tokio/latest/tokio/process/struct.Child.html)

`kill_on_drop` is useful but insufficient by itself because a shell can spawn descendants. Use a process group or cgroup and verify that cleanup completed.

V0 `run_shell` should be a bounded foreground execution primitive. Add a separate durable `start_process`/`process_status` design later if long-lived development servers become necessary.

### Future tools

The same execution envelope can later support:

- browser adapters;
- database tools;
- cloud APIs;
- subprocess handles;
- MCP-backed tools;
- sandboxed executors.

MCP should be another tool source behind Craxii’s validation and authority gate, never a replacement for the gate.

---

## 8. Concurrency and turn semantics

### Correct V0 behavior: queue

For:

> “Also tell me how many users we have”

while a test command is running, V0 should:

1. authenticate and persist the message immediately;
2. create a separate queued work item atomically;
3. acknowledge it to the client;
4. show it as queued;
5. exclude it from the active work item;
6. execute it after the current work reaches a terminal state.

Do not:

- merge it silently;
- steer the active model loop implicitly;
- start a parallel work unit;
- reject it;
- let “full history” expose it to the active invocation.

A separate explicit Cancel action should allow the user to stop the active work before the queued item begins.

### State model

`work_items` should include at least:

```text
work_id
conversation_id NULL
conversation_work_ordinal NULL
trigger/input event IDs
state
current_attempt
runtime_instance_id NULL
created_at
started_at NULL
terminal_at NULL
cancel_requested_at NULL
terminal_reason NULL
```

States:

```text
queued
running
waiting_on_model
waiting_on_tool
completed
failed
cancel_requested
cancelled
interrupted
```

`interrupted` is different from `failed`: the backend lost ownership before it could observe a terminal result.

### Restart rules

- Queued, never-started work may resume automatically.
- Work running under a dead runtime instance becomes `interrupted`.
- An in-flight shell execution becomes `outcome_unknown`; do not rerun it automatically.
- V0 does not resume inside an arbitrary provider stream or shell command.
- Later, steering can be an append-only input event explicitly targeted to a running `work_id` and consumed at a safe point.

This state model supports future parallelism without implementing it now.

---

## 9. Client and protocol architecture

### Native client

SwiftUI with AppKit where required is the correct first client. Keep the client thin and put device credentials in macOS Keychain.

### HTTP versus WebSocket

I recommend:

- HTTP for durable client commands.
- WebSocket for server event delivery.

Example:

```text
POST /v1/conversations/{id}/messages
POST /v1/work/{id}/cancel
GET  /v1/bootstrap
GET  /v1/conversations/{id}/history
WS   /v1/events?after=<cursor>
```

Submitting user messages over WebSocket creates ambiguous delivery after disconnect: the client may not know whether the server committed the message. HTTP plus an idempotency key gives much cleaner retry and acknowledgement semantics.

The WebSocket may still carry subscription acknowledgements and heartbeats, but not authoritative mutations.

### Command response

A successful message submission should return `202 Accepted` with:

```text
message_id
work_id
state = queued | running
server_cursor
duplicate = true | false
```

### Server event envelope

```text
protocol_version
event_id
cursor
event_type
conversation_id NULL
work_id NULL
occurred_at
payload
```

The protocol cursor should be distinct from provider sequence numbers and timestamps.

### Reconnection

Use:

1. fetch bootstrap/snapshot with a high-water cursor;
2. connect with `after=cursor`;
3. replay durable events after that cursor;
4. switch to live delivery.

This closes the snapshot/WebSocket race.

Distinguish durable events from ephemeral streaming drafts:

- `assistant.message_committed` is durable.
- token deltas are draft events tagged with `draft_id`, `invocation_id`, and delta sequence.
- after reconnect, committed state is authoritative.
- if a provider attempt fails after emitting deltas, emit `draft_abandoned`; do not concatenate a retry into the old draft.

Publish terminal client events only after the corresponding database commit.

---

## 10. Failure and recovery architecture

The minimum invariant should be:

```text
durable intent → external action → durable observation
```

### Startup recovery

On startup:

1. create a new `runtime_instance_id`;
2. verify schema compatibility;
3. inspect nonterminal work and execution attempts;
4. mark work owned by an old runtime as interrupted;
5. mark unresolved tool executions as outcome unknown;
6. retain queued work;
7. emit a recovery event;
8. begin scheduling.

Do not synthesize `turn_failed` as if Craxii observed a definite failure.

### Provider failures

- Record every attempt independently.
- Retry only bounded, classified transient failures.
- Respect provider retry guidance.
- A connection reset after request transmission may have consumed provider capacity; record the ambiguity.
- Retry automatically only before semantic output has escaped to the client in V0.
- Never execute a partial tool call; wait until arguments are complete and validated.

### Tool failures

- Nonzero exit is a completed command outcome, not infrastructure failure.
- File-not-found is a structured tool outcome and may be returned to the model.
- Timeout and cancellation must kill and reap the process tree.
- Spawn failure, validation failure, timeout, signal termination, and nonzero exit must remain distinct.
- Never automatically retry `run_shell`; it may have performed irreversible work.

### Cancellation

Cancellation should be required for V0 once long-running shell commands exist. An uninterruptible agent waiting on a command is not a viable coworker.

A turn becomes `cancelled` only after:

- future provider calls have stopped;
- the active provider request has been abandoned locally;
- child processes have been terminated or explicitly reported as not terminated;
- the cancellation event has committed.

### Required crash-injection tests

Add tests for crashes:

- after message/work creation but before scheduling;
- after model-attempt creation but before response;
- after tool intent but before observed result;
- after assistant-message commit but before client delivery;
- while a second message is queued;
- during graceful systemd shutdown.

These are more valuable than merely killing the backend after a completed turn.

---

## 11. Security and authority boundary

### Must solve in V0

1. **Separate server and executor OS identities.**  
   One “dedicated Craxii user” is insufficient if model-controlled shell commands can read `craxii.db`, server configuration, provider keys, or `/proc` state belonging to the backend.

2. **Run neither component as root.**  
   The workspace user should have no `sudo`, Docker socket, or ability to mutate the server binary/unit.

3. **Do not inherit server environment into tools.**  
   Clear the environment and construct an allowlist.

4. **Keep provider credentials outside model-controlled files.**  
   A server-only systemd credential file is sufficient for this development slice. Secrets Manager is also possible, but the workspace must then be unable to use the EC2 instance role or metadata endpoint to retrieve it.

5. **No production credentials or production data.**  
   This is the safety property that makes broad V0 shell access acceptable.

6. **No unauthenticated public listener.**  
   Use TLS plus a private network or tightly restricted security group. Store the device token in Keychain and a verifier/hash server-side.

7. **Encrypt EBS and protect backups.**

8. **Never log request bodies, user text, tool output, authorization headers, or credentials by default.**

9. **Apply basic resource controls.**  
   Time, process count, output, and disk growth need limits even without a sandbox.

10. **Restrict the hosting instance profile.**  
    It must not contain customer/project authority. If it has SSM or secret access, ensure workspace code cannot obtain or exercise it.

### Can safely defer

- external Authority Service;
- renewable short-lived credential minting;
- per-project VMs;
- workload attestation;
- production cloud identities;
- trust realms;
- browser isolation;
- strong sandboxing;
- multi-tenant isolation;
- policy compilation for “do not deploy.”

### Seams that must exist now

- `craxii_id`, `workspace_id`, `work_id`;
- authority context on every tool execution;
- a policy-gate call before dispatch, even if V0’s implementation is “allow in this development workspace”;
- a separate executor boundary;
- a secrets-provider interface that is never exposed as a model tool;
- device/user identity in the protocol.

The separate credential document correctly observes that a root-controlled workspace can exercise every authority reachable from that workspace. V0 should therefore remain explicitly classified as a development runtime, not the “first production Craxii.”

---

## 12. Observability and evaluation

The proposed tracing spans are good but not sufficient for evidence-driven architecture decisions.

### Record for every work item

- queue wait;
- total duration;
- completion state and reason;
- number of model invocations;
- number of tool executions;
- agent-loop iterations;
- cancellation/interruption;
- time to first progress;
- time to committed answer.

### Context assembly

- source event count by type;
- source artifact count;
- context bytes;
- estimated and provider-reported tokens;
- system/tool/history token contribution;
- percentage of model context window;
- truncations and omissions;
- context manifest hash;
- assembler and system-prompt version;
- assembly latency.

### Model invocation

- provider, model, endpoint/config version;
- selection reason;
- attempt number;
- request bytes;
- time to first token;
- total latency;
- input, cached input, reasoning, and output tokens;
- stop reason;
- tool-call count;
- provider request ID;
- retry classification and delay;
- stream failure after partial output;
- normalized error code.

### Tool execution

- tool and version;
- validation result;
- workspace;
- queue and execution latency;
- exit code or signal;
- timeout/cancellation;
- stdout/stderr bytes captured and returned;
- artifact bytes;
- truncation;
- process-tree cleanup result.

### Storage and recovery

- journal transaction latency;
- `SQLITE_BUSY` count;
- WAL size and checkpoint latency;
- database and artifact-store size;
- backup age;
- restore-test status;
- runtime restart count;
- queued work resumed;
- work marked interrupted;
- unresolved tool outcomes.

### Protocol

- message deduplications;
- idempotency conflicts;
- reconnect count;
- replayed event count;
- cursor lag;
- dropped ephemeral events;
- authentication failures.

Do not put conversation IDs, work IDs, or invocation IDs into low-cardinality metric labels. Put them in structured traces and logs.

V0 does not need Prometheus, OpenTelemetry infrastructure, or an evaluation platform. Structured tracing plus the invocation/execution tables is enough, provided the above fields exist.

---

## 13. Scope assessment

V0.0.01 is simultaneously:

- slightly too ambitious in speculative abstraction;
- slightly too small in correctness and authority semantics;
- correctly sized as an end-to-end product slice after rebalancing.

### Cut or simplify

- sophisticated capability registry behavior;
- advanced routing;
- parallel tool execution;
- provider-state optimization;
- WebSocket command submission;
- tool stdout streaming;
- automatic resumption of in-flight work;
- automatic retries of tools;
- elaborate macOS UI;
- any claim of production security.

### Add because it is architecturally necessary

- durable work items;
- causal context visibility;
- executor/server OS separation;
- cancellation;
- tool execution attempt records;
- HTTP idempotent commands;
- replay cursor;
- interrupted/unknown recovery semantics;
- context budget failure;
- EBS backup and restore procedure.

### Explicitly continue deferring

- memory and compaction;
- search and vectors;
- background scheduler;
- multiple real providers;
- production authority service;
- per-project workstations;
- S3;
- PostgreSQL;
- browser;
- mobile/Windows;
- sandbox fleet;
- deployment authority.

This is a swap of scope, not an expansion into a platform.

---

## 14. Recommended alternative V0.0.01 architecture

```text
macOS native client
  ├── HTTP: durable commands, snapshot, history, cancellation
  └── WebSocket: replayable server events and ephemeral drafts
                         │
                         ▼
                craxii-server
                Rust / Tokio / Axum
  ┌──────────────────────────────────────────────────────────┐
  │ protocol                                                 │
  │ durable work queue / one active work per conversation    │
  │ explicit agent loop                                      │
  │ context assembler                                        │
  │ model selector + OpenAI adapter                          │
  │ tool execution service + policy seam                     │
  │ journal/recovery                                         │
  └───────────────┬─────────────────────┬────────────────────┘
                  │                     │
        server-only SQLite WAL     local artifact store
          /var/lib/craxii          /var/lib/craxii/artifacts
                  │
                  ▼
       persist intent before execution
                  │
                  ▼
          local executor boundary
       separate Unix user and cgroup
       sanitized environment, no secrets
                  │
                  ▼
       /srv/craxii/workspaces/<id>
       Linux files and foreground processes
```

This differs from the proposal only where the migration cost would otherwise become high:

- work rather than conversation is the execution primitive;
- journal events carry durable identity and causality;
- server and shell authority are separated;
- commands are retryable HTTP operations;
- streaming is a reconnectable presentation channel;
- incomplete side effects are represented honestly.

It still uses Rust, Tokio, Axum, Reqwest, Serde, tracing, SQLx, SQLite, EC2, EBS, Ubuntu, systemd, SwiftUI, one backend host, and one explicit agent loop.

---

## 15. Final verdict

### Would I approve it for implementation?

Not exactly as written.

I would approve it after the targeted boundary changes above. The stack and overall topology should remain.

### Changes required before writing code

1. Add immutable `craxii_id`, `work_id`, `workspace_id`, and runtime-attempt identities.
2. Revise the journal envelope for schema versioning, causality, stream ordering, and replay cursors.
3. Add explicit current-state tables for work, client commands, model attempts, tool executions, and artifacts.
4. Specify queue, cancellation, interruption, and causal context-visibility semantics.
5. Reorder model selection and context rendering; replace `FinalText | ToolCalls` with ordered output items.
6. Separate backend and tool execution by OS identity/process boundary.
7. Use HTTP for durable commands and WebSocket for event delivery.
8. Choose SQLite WAL with `synchronous=FULL`, encrypted EBS, backups, and a restore procedure.
9. Expand acceptance tests to cover crash windows and unknown side effects.

### Explicit deferrals

Continue deferring semantic memory, compaction, background scheduling, production authority, multiple real providers, project VM isolation, S3, PostgreSQL, vector retrieval, browser automation, mobile clients, and advanced routing.

### Top three risks

1. **Incorrect work and causal-history model:** queued inputs, background responsibilities, recovery, and future memory would all inherit ambiguous semantics.
2. **Authority collapse at the shell boundary:** a model-controlled process could corrupt Craxii’s journal, steal provider credentials, or survive cancellation.
3. **False durability:** EBS without backup, simplistic retry behavior, and “failed” versus “unknown” confusion could make Craxii’s historical record untrustworthy.

### Top three things the design gets right

1. It correctly makes Craxii’s explicit harness—not a model session—the owner of continuity and execution.
2. It chooses boring, proven components and avoids premature distributed or framework-driven orchestration.
3. It draws the right high-level boundaries around context, providers, tools, native clients, and the workstation while deliberately deferring speculative systems.

### Exact first implementation milestone

Build a headless, crash-safe responsibility spine on Ubuntu before OpenAI or the macOS UI:

1. Rust/Tokio/Axum service under systemd.
2. Revised SQLite schema and migrations.
3. Idempotent HTTP message submission that atomically creates a message event and queued work item.
4. One durable per-conversation scheduler.
5. A deterministic scripted model adapter that requests a real `read_file` or bounded shell command.
6. Tool execution through the separate executor identity.
7. Persisted tool request/result and committed final answer.
8. Replayable event cursor.
9. Crash-injection tests before scheduling, during model wait, during tool execution, and after final commit.
10. Verify that queued work resumes, active work becomes interrupted, unknown shell outcomes are not retried, and duplicate client input creates exactly one work item.

Only after that spine passes should the OpenAI adapter, live streaming, native macOS client, and final EC2 acceptance path be layered on top.

That milestone proves Craxii’s hardest foundational claim: not merely that a model can call a tool, but that one durable Craxii can accept responsibility, execute through controlled boundaries, survive failure honestly, and continue from trustworthy state.
