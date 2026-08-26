# Craxii V0.0.01 — Deep Architecture and Implementation Source of Truth

**Status:** Authoritative working architecture for V0.0.01  
**Audience:** Craxii engineering, Codex, ChatGPT, future contributors  
**Purpose:** Define the product, runtime architecture, process boundaries, data model, control flow, model orchestration, tool execution, persistence, transport, deployment, and non-goals of Craxii V0.0.01 at implementation depth.

This document is intentionally **not** a tutorial. It assumes familiarity with Rust, Axum, Tokio, SQLite, SQLx, WebSockets, SwiftUI, and cloud infrastructure. Its job is to tell an engineer what Craxii is, how V0.0.01 is supposed to work, where each responsibility lives, what is authoritative, and which architectural constraints must not be violated.

---

# 1. Product Definition

Craxii is a persistent AI coworker whose identity and continuity are owned by our software, not by any one model provider or model session.

The model is a replaceable inference component. Craxii is the durable harness around inference.

The long-term product should feel like one continuous coworker relationship:

```text
User
  ⇄
Craxii
```

not:

```text
User
  ⇄
Agent Run A

User
  ⇄
Agent Run B

User
  ⇄
Agent Run C
```

The user gives Craxii responsibilities. Craxii decides how to investigate, reason, use tools, inspect its machine, and continue work.

V0.0.01 is the smallest architecture that preserves this product direction.

---

# 2. V0.0.01 Objective

V0.0.01 must prove one complete agent loop with durable continuity.

A successful V0.0.01 must support this end-to-end path:

```text
Native macOS client
        ↓
user message
        ↓
Rust backend
        ↓
append durable event
        ↓
assemble context
        ↓
select model
        ↓
invoke provider
        ↓
receive tool call
        ↓
dispatch tool
        ↓
execute on Ubuntu workstation
        ↓
append tool result
        ↓
invoke model again
        ↓
receive final answer
        ↓
append final answer
        ↓
stream answer to client
```

Then the backend must be killable and restartable without losing the conversation history required to reconstruct the next model request.

The V0.0.01 success claim is therefore:

> Craxii is a persistent, tool-using agent runtime with one native client, one durable event journal, one Linux workstation, and one explicit model/tool control loop.

---

# 3. Architectural Invariants

The following are hard constraints for V0.0.01.

The **Rust backend owns the agent loop**. No external agent framework may own iteration, tool execution, or continuation.

The **journal is authoritative for historical interaction events**. Process memory is never authoritative across restart.

The **model never directly executes tools**. It can only request them.

The **model never owns persistence**. Model-provider conversation state may be used later as an optimization, but it is not canonical state.

The **client is not authoritative for agent state**. It renders Craxii and submits user input.

The **model layer is multi-provider by design**. OpenAI is the first concrete adapter, not the architecture.

The **context window is a rendered view, never the system of record**.

The **EC2 machine is Craxii's first workstation, not Craxii's permanent identity**.

The **backend and workstation are the same machine in V0.0.01**.

The system must be understandable from source. Important behavior may not be hidden behind large orchestration frameworks.

---

# 4. Physical Deployment Topology

V0.0.01 uses one AWS EC2 instance.

```text
AWS Region
└── EC2 instance
    ├── x86-64
    ├── Ubuntu 24.04 LTS
    ├── RAM
    ├── network interface
    └── EBS volume
        └── Linux filesystem
```

The EC2 instance runs one long-lived Craxii backend process under systemd.

```text
Ubuntu
└── systemd
    └── craxii-server
```

The same machine is also the execution workstation.

```text
EC2 / Ubuntu
├── craxii-server
├── repositories
├── Git
├── shell
├── compilers
├── test runners
├── Docker later if needed
└── craxii.db
```

There is no separate tool-execution host in V0.0.01.

---

# 5. Runtime Process Model

The V0 backend is one Rust process.

```text
craxii-server
│
├── API server
├── WebSocket connection manager
├── agent runtime
├── journal repository
├── context assembler
├── model layer
├── tool registry
├── tool dispatcher
├── process executor
└── telemetry
```

Tokio is the async runtime underneath these components.

Concurrency is allowed at the runtime level, but the product semantics of concurrent user turns must remain conservative in V0.0.01.

The recommended initial rule is:

> One active agent turn per Craxii conversation at a time.

A second user message arriving while a turn is active should be persisted immediately but must not create unsafe interleaving inside the same model/tool loop until we explicitly design steering.

The backend may still concurrently handle network connections, health endpoints, DB reads, logging, and unrelated I/O.

---

# 6. Backend Module Boundaries

Recommended backend layout:

```text
backend/
└── src/
    ├── main.rs
    ├── app.rs
    ├── config.rs
    ├── api/
    │   ├── mod.rs
    │   ├── http.rs
    │   ├── websocket.rs
    │   └── protocol.rs
    ├── agent/
    │   ├── mod.rs
    │   ├── runtime.rs
    │   ├── turn.rs
    │   └── state.rs
    ├── context/
    │   ├── mod.rs
    │   └── assembler.rs
    ├── journal/
    │   ├── mod.rs
    │   ├── schema.rs
    │   ├── repository.rs
    │   └── reconstruction.rs
    ├── models/
    │   ├── mod.rs
    │   ├── types.rs
    │   ├── registry.rs
    │   ├── selection.rs
    │   └── providers/
    │       ├── mod.rs
    │       └── openai.rs
    ├── tools/
    │   ├── mod.rs
    │   ├── registry.rs
    │   ├── dispatcher.rs
    │   ├── types.rs
    │   ├── read_file.rs
    │   └── run_shell.rs
    └── telemetry/
        ├── mod.rs
        └── tracing.rs
```

These module names are not sacred. The ownership boundaries are.

---

# 7. Agent Runtime

The agent runtime is the core of Craxii.

It owns turn execution.

Conceptually:

```text
start_turn(user_message)
    ↓
persist user_message
    ↓
loop
    ↓
assemble context
    ↓
select model
    ↓
invoke model
    ↓
persist model output metadata
    ↓
if final:
    persist assistant_message
    finish turn

if tool call:
    persist tool_call
    dispatch tool
    persist tool_result
    continue loop
```

Pseudo-structure:

```rust
while !turn.is_terminal() {
    let request = context_assembler.assemble(...).await?;

    let target = model_selector.select(&request.requirements)?;
    let output = model_layer.invoke(target, request).await?;

    match output {
        ModelOutput::FinalText(text) => { ... }
        ModelOutput::ToolCalls(calls) => { ... }
        ModelOutput::Error(err) => { ... }
    }
}
```

The loop must remain explicit enough that an engineer can trace every transition.

No library may silently trigger another model call.

---

# 8. Turn State

V0.0.01 should model turns explicitly.

Suggested states:

```text
queued
running
waiting_on_model
waiting_on_tool
completed
failed
cancelled
```

The journal should record durable lifecycle events, but runtime state may additionally exist in memory.

A restart during an active turn does not need to automatically resume from an arbitrary in-flight HTTP request in V0.0.01.

The required behavior is:

> After restart, completed history remains reconstructable and the system returns to a consistent state.

Incomplete turns should be detected and marked/recovered deterministically rather than silently pretending they completed.

---

# 9. Journal Architecture

The journal is the canonical interaction history.

V0.0.01 uses SQLite.

The journal must be append-oriented.

Do not treat the current conversation transcript as the primary database model. The primary model is events.

Recommended core table:

```sql
journal_events
--------------
event_id
conversation_id
turn_id
sequence_no
event_type
actor
payload_json
created_at
parent_event_id NULL
```

Recommended properties:

`event_id` should be globally unique.

`sequence_no` should provide deterministic ordering within one conversation.

`event_type` should be a constrained enum at the Rust layer.

`payload_json` should contain event-specific structured data.

`actor` should distinguish user, craxii, model, tool, system where useful.

The journal should not silently mutate old events.

Corrections later should append new events.

---

# 10. Initial Event Taxonomy

Recommended V0 event types:

```text
conversation_created
turn_started
user_message
model_request
model_response
tool_call
tool_result
assistant_message
turn_completed
turn_failed
runtime_error
```

`model_request` should not necessarily contain the full prompt payload if doing so duplicates large data excessively. It should at minimum contain enough metadata to correlate inference with the turn and usage record.

For V0, storing prompt snapshots may be useful for debugging, but the design should distinguish:

```text
canonical interaction history
vs
diagnostic request snapshot
```

They are not the same thing.

---

# 11. Model Usage Table

Model usage should be queryable without parsing every journal payload.

A separate table is acceptable:

```sql
model_invocations
-----------------
invocation_id
conversation_id
turn_id
provider
model
started_at
completed_at
input_tokens
cached_input_tokens
output_tokens
reasoning_tokens
latency_ms
status
provider_request_id NULL
error_code NULL
```

This table is operational/analytical state.

The journal remains the historical event stream.

---

# 12. SQLite Configuration

SQLite runs in WAL mode.

Recommended V0 properties:

```text
journal_mode = WAL
foreign_keys = ON
busy_timeout configured
synchronous policy chosen explicitly
```

The exact `synchronous` setting must be chosen consciously during implementation.

Do not enable aggressive performance pragmas without documenting durability implications.

SQLx should own connection pooling.

Because SQLite permits one writer at a time, journal writes should remain short and transactional.

No long-running network work should occur inside a DB transaction.

---

# 13. Reconstruction

On startup, Craxii must reconstruct the current conversation state from durable storage.

The reconstruction path should not depend on stale in-memory snapshots.

V0 reconstruction can be simple:

```text
load conversation metadata
    ↓
load ordered journal events
    ↓
project relevant events into transcript/state
    ↓
backend ready
```

The reconstruction function should be deterministic for a given ordered journal.

This is the basis of the kill/restart acceptance test.

---

# 14. Context Assembler

V0.0.01 intentionally implements a naive context assembler.

Its contract should still be explicit:

```rust
assemble(
    conversation_id,
    turn_id,
    model_profile,
) -> ModelRequest
```

The assembler should pull from canonical internal state, not from provider-specific response objects.

The V0 request should contain:

```text
system instructions
tool definitions
full reconstructed conversation
current user turn
```

The exact representation passed to a provider is not the assembler's responsibility.

The assembler produces Craxii's internal model request format.

Provider adapters translate that format.

---

# 15. Context Architecture Boundary

The context assembler must not own durable memory.

It reads durable state and renders a bounded model input.

In V0.0.01, the rendering is intentionally unsophisticated.

In V0.0.02, this component is expected to evolve substantially.

Therefore the boundary must already make later replacement possible.

Avoid scattering prompt-building logic through:

```text
API handlers
provider adapters
tool handlers
WebSocket code
```

All conversation-context assembly belongs in one subsystem.

---

# 16. Model Layer Overview

The model layer is not one `ModelProvider` abstraction.

Craxii is designed for multi-model, multi-provider inference.

The model subsystem has four responsibilities:

```text
1. canonical internal request/response types
2. capability registry
3. model selection policy
4. provider adapters
```

Architecture:

```text
Agent Runtime
    ↓
Internal Model Request
    ↓
Selection Policy
    ↓
Capability Registry
    ↓
Selected Model Target
    ↓
Provider Adapter
    ↓
Provider API
```

---

# 17. Canonical Internal Model Types

Craxii needs its own model-facing types.

Example conceptual request:

```rust
ModelRequest {
    messages,
    tools,
    requirements,
    output_mode,
    inference_preferences,
}
```

Example conceptual response:

```rust
ModelResponse {
    content,
    tool_calls,
    usage,
    provider_metadata,
}
```

These types should cover common semantics without pretending all providers are identical.

The common abstraction should be narrow enough to remain stable.

---

# 18. Provider-Native Escape Hatches

Provider adapters must retain access to provider-native features.

Craxii must not collapse every provider to the lowest common denominator.

The model layer should support extension data such as:

```text
OpenAI-specific reasoning controls
Anthropic-specific cache controls
Gemini-specific generation features
provider-specific structured-output controls
provider-specific state identifiers
```

The agent runtime should not depend on these fields unless an explicit capability requires them.

A good rule is:

```text
common semantics in canonical types
provider-specific optimizations in adapter options
```

---

# 19. Capability Registry

The capability registry is data, not routing logic.

A model entry may eventually include:

```text
provider
model_id
supports_tools
supports_vision
supports_structured_output
supports_streaming
context_window
supports_prompt_cache
supports_reasoning_controls
cost_class
latency_class
enabled
```

V0.0.01 can keep this registry small and static.

It should still exist as a distinct concept so the selection layer does not hard-code provider facts.

---

# 20. Model Selection Policy

The selection policy decides which configured model handles an inference.

V0.0.01 should not attempt sophisticated learned routing.

A simple policy is enough:

```text
if user explicitly selected model:
    use it if capable

else:
    use configured default model
```

The architecture must permit later rules based on:

```text
task type
tool requirements
context size
cost
latency
vision
coding
reasoning
availability
eval results
```

Do not encode future policy prematurely.

---

# 21. OpenAI Adapter

OpenAI is the first concrete provider adapter.

The adapter owns:

```text
authentication
Responses API request construction
tool schema translation
stream decoding
usage parsing
provider errors
provider request IDs
provider-specific options
```

The rest of Craxii should not import OpenAI-specific wire types.

The adapter should convert:

```text
Craxii ModelRequest
        ↓
OpenAI request

OpenAI response
        ↓
Craxii ModelResponse
```

Reqwest is transport only.

---

# 22. Tool Architecture

The tool subsystem has three layers:

```text
Tool Registry
    ↓
Tool Dispatcher
    ↓
Tool Implementation
```

The model receives tool schemas.

The model may return a tool call.

The dispatcher resolves and executes it.

The result is transformed into a canonical Craxii tool result and returned to the agent loop.

---

# 23. Tool Definition Contract

Each tool should expose:

```text
name
description
input schema
handler
```

Recommended conceptual Rust type:

```rust
ToolDefinition {
    name: String,
    description: String,
    input_schema: JsonSchema,
}
```

Implementation registration should associate metadata with an executable handler.

Tool schemas sent to providers must be derived from or kept consistent with runtime validation.

Do not trust provider-generated tool arguments merely because they conform approximately to a schema.

Validate before execution.

---

# 24. Tool Dispatcher Responsibilities

The dispatcher owns:

```text
tool existence validation
argument decoding
argument validation
execution
timeout policy
result capture
error normalization
journal event creation
```

It must return structured results.

For shell execution, a result should include at least:

```text
exit_code
stdout
stderr
duration
timed_out
```

Large output handling can be naive in V0, but output size must have a defined limit to prevent accidental unbounded context growth.

---

# 25. Initial Tool: read_file

`read_file` proves direct workstation filesystem inspection.

Input should include a path.

The implementation should:

```text
resolve path
read bytes
validate UTF-8 or return binary/not-text error
apply size limit
return content + metadata
```

The tool must not fabricate content on failure.

The result must be explicit about:

```text
success
not found
permission denied
too large
binary
I/O error
```

---

# 26. Initial Tool: run_shell

`run_shell` proves real process execution.

The implementation should not require invoking an interactive shell unless shell syntax is explicitly necessary.

Prefer direct process invocation where practical.

If the tool accepts a shell command string, the semantics must be documented clearly.

Result shape:

```text
command
cwd
exit_code
stdout
stderr
duration_ms
timed_out
```

The tool must support timeout.

The backend must remain alive when the child process exits non-zero.

---

# 27. Workstation Semantics

The EC2 Ubuntu machine is the V0 workstation.

Craxii should have a dedicated OS user.

Recommended:

```text
/home/craxii/
```

Project workspaces should live below a predictable root such as:

```text
/home/craxii/projects/
```

The backend should have a defined default working directory.

Tool calls that depend on `cwd` should make it explicit.

V0.0.01 does not yet implement trust realms or sandbox cells.

That means this workstation should contain only data we are comfortable exposing to a highly capable tool-using agent during development.

---

# 28. Client/Backend Protocol

The Mac client and backend should speak a Craxii-owned protocol.

Do not expose raw Axum internals or raw model-provider events.

Protocol messages should be versionable.

Recommended server-to-client event categories:

```text
conversation_snapshot
turn_started
assistant_delta
tool_started
tool_finished
turn_completed
turn_failed
error
```

Recommended client-to-server messages:

```text
send_user_message
cancel_turn
subscribe_conversation
```

V0 may omit `cancel_turn` if not implemented, but the protocol should not assume every turn is a single HTTP response.

---

# 29. HTTP vs WebSocket Responsibilities

Use HTTPS for stateless bootstrap operations.

Use WebSocket for live conversation events.

Recommended division:

```text
HTTPS
------
health
initial conversation snapshot
configuration/bootstrap
possibly history fetch

WebSocket
---------
user message submission
assistant streaming
tool lifecycle events
turn lifecycle
live errors
```

This division is not immutable.

The important constraint is that live agent execution must have a transport capable of incremental server events.

---

# 30. WebSocket Session Semantics

A WebSocket connection is not Craxii's identity.

If the socket disconnects, Craxii must continue to have durable history.

Client reconnection should:

```text
reconnect
   ↓
identify conversation
   ↓
fetch/replay current durable state
   ↓
resume receiving live events
```

The socket is transport.

The journal is continuity.

---

# 31. macOS Client Architecture

The macOS client should remain thin.

Recommended layers:

```text
SwiftUI Views
    ↓
View Models
    ↓
Craxii Client
    ↓
HTTP/WebSocket transport
```

The client owns:

```text
rendering
user input
connection state
stream presentation
local UI state
```

The client does not own:

```text
agent loop
provider API keys
model routing
tool execution
canonical history
memory
```

A local cache is acceptable later but must never become the canonical journal.

---

# 32. Native Client Product Constraint

Craxii is not intended to become a web application.

The first client is native macOS.

Future clients may include:

```text
iOS
Android
Windows
```

All clients should connect to the same backend identity.

The native client architecture should therefore avoid macOS-specific assumptions in the backend protocol.

---

# 33. Authentication for V0.0.01

V0.0.01 is single-user development software.

Do not build consumer auth.

The backend must still not be unauthenticated on the public internet.

A simple development authentication mechanism is sufficient.

The exact mechanism must be chosen explicitly during infrastructure implementation.

Provider API keys must exist only server-side.

No provider credential may ship inside the macOS application.

---

# 34. Secrets

V0.0.01 does not implement the final Authority Service.

For this version:

```text
provider credentials
    live server-side

client
    never receives them

model prompt
    never receives them

journal
    never intentionally stores them
```

Secrets must not be included in tracing output.

The future credential architecture is intentionally deferred.

---

# 35. Observability Architecture

There are two separate records:

```text
Craxii Journal
= product/history semantics

tracing
= software/runtime diagnostics
```

Do not conflate them.

The journal answers:

> What happened in Craxii's work?

Tracing answers:

> What happened inside the program while it was doing that work?

---

# 36. tracing Instrumentation

Recommended spans:

```text
http_request
websocket_connection
turn
model_invocation
tool_dispatch
tool_execution
journal_write
journal_read
context_assembly
```

Recommended correlation fields:

```text
conversation_id
turn_id
invocation_id
tool_call_id
request_id
```

Do not put full user text or secrets into structured log fields by default.

---

# 37. Error Taxonomy

Errors should be normalized at subsystem boundaries.

Recommended high-level categories:

```text
client_protocol_error
journal_error
context_error
model_selection_error
provider_error
tool_validation_error
tool_execution_error
transport_error
internal_error
```

Provider-specific error structures should remain available for debugging but should not leak across the entire application.

---

# 38. Failure Semantics

A tool failure is not automatically a turn failure.

Example:

```text
model calls read_file
   ↓
file not found
   ↓
structured tool result
   ↓
model sees failure
   ↓
model may choose another action
```

A provider transient failure may be retried according to a bounded policy.

A backend panic is not an acceptable control-flow mechanism.

The journal should never claim a turn completed unless it actually reached terminal completion.

---

# 39. Retry Policy

V0 retries should be minimal and explicit.

Good retry candidates:

```text
transient provider 5xx
connection reset
rate-limit response with usable retry guidance
```

Bad automatic retry candidates:

```text
invalid tool arguments
permission denied
user-caused invalid path
deterministic provider request validation error
```

Retries should use bounded exponential backoff with jitter if implemented.

Do not create infinite agent retries.

---

# 40. Idempotency

V0.0.01 should avoid duplicate durable events when the client retransmits a user message after a network failure.

Client messages should carry a client-generated message ID or idempotency key.

The backend should detect duplicates.

This is especially important because WebSocket reconnect behavior can otherwise create duplicate turns.

A minimal design:

```text
client_message_id UNIQUE
```

associated with user-message ingestion.

---

# 41. Ordering

Journal ordering must not rely solely on wall-clock timestamps.

Use deterministic sequence numbers within a conversation.

For example:

```text
conversation_id + sequence_no UNIQUE
```

This gives reconstruction a canonical order even if timestamps collide.

---

# 42. Time

Store timestamps in UTC.

Use monotonic clocks for duration measurements.

Do not use wall-clock differences for latency where a monotonic timer is available.

---

# 43. Streaming Model Output

Provider streaming should be converted into Craxii-owned events.

The client must never depend on provider-specific streaming chunk shapes.

The provider adapter can emit internal deltas.

The agent runtime can forward appropriate deltas as:

```text
assistant_delta
```

Final text must still be persisted as a complete assistant-message event.

---

# 44. Tool Streaming

V0.0.01 does not need arbitrary tool stdout streaming.

It is acceptable to emit:

```text
tool_started
tool_finished
```

and return the final bounded stdout/stderr.

Later versions may stream long-running process output.

Do not complicate V0 before the agent loop works.

---

# 45. Cancellation

Cancellation is useful but not required for the earliest slice.

If implemented, cancellation must be cooperative.

A cancelled turn should:

```text
stop future model calls
terminate cancellable child process
append turn_cancelled or equivalent
leave journal consistent
```

Do not simply drop an async task and pretend the turn never existed.

---

# 46. systemd Deployment

The backend should run as a systemd service.

Recommended properties:

```text
dedicated user
working directory
environment/secrets source
Restart=on-failure
restart delay
stdout/stderr captured by journald initially
```

The systemd unit should execute the compiled binary directly.

It should not run Cargo.

---

# 47. Build and Deployment

Recommended initial flow:

```text
local development
    ↓
cargo test
cargo build --release
    ↓
artifact copied to EC2
    ↓
systemd service restart
```

A more sophisticated CI/CD pipeline is out of scope.

The deployment process must still be repeatable and documented.

---

# 48. Database Migrations

SQLx migrations should be version-controlled.

Do not create tables ad hoc at runtime.

Recommended structure:

```text
backend/migrations/
```

Startup should verify the schema is compatible.

Whether migrations auto-run in development or are an explicit deployment step should be decided before implementation.

---

# 49. Repository Ownership Boundary

The Craxii repository should contain:

```text
backend
native clients
protocol definitions
migrations
architecture docs
```

Do not put unrelated product repos inside the Craxii repository.

The Craxii workstation may clone external repos into `/home/craxii/projects`, but those remain independent Git repositories.

---

# 50. V0.0.01 Model Choice

Architecture must not encode one fixed model.

Implementation should start with one OpenAI model available through the Responses API to validate the path.

The actual model ID is runtime configuration.

Example:

```text
CRAXII_DEFAULT_MODEL_PROVIDER=openai
CRAXII_DEFAULT_MODEL=<configured model id>
```

Do not scatter model names through source code.

---

# 51. Configuration

Configuration should come from typed startup configuration.

Likely categories:

```text
server bind address
database path
default conversation
provider credentials
enabled models
default model
tool timeouts
workspace root
logging level
```

Configuration should be validated at startup.

Missing critical configuration should fail fast.

---

# 52. One Conversation in V0

V0.0.01 should expose one primary Craxii conversation.

Internally, the schema may support multiple conversation IDs if doing so costs little, but the product should not turn into a thread-management UI.

The first product experience is:

```text
open app
   ↓
Craxii is there
```

not:

```text
choose agent
choose session
choose thread
```

---

# 53. Resume Semantics

V0.0.01 resume means:

```text
backend process restarts
    ↓
journal remains
    ↓
conversation reconstructs
    ↓
next user message includes prior history
```

It does not yet mean:

```text
resume exact in-flight provider stream
resume arbitrary half-executed tool
restore every async future
```

Those are later problems.

---

# 54. Context Growth

V0.0.01 intentionally allows context to grow linearly with conversation history.

This is temporary and deliberate.

Instrumentation should make the growth visible.

V0.0.02 will revisit:

```text
compaction
bounded recent tail
history retrieval
possibly structured memory
```

Do not prematurely add these to V0.0.01.

---

# 55. Large Tool Outputs

V0 needs a hard output cap.

A shell command that produces 500 MB of logs must not be inserted blindly into:

```text
SQLite payload
model context
WebSocket frame
```

Recommended V0 behavior:

```text
capture up to configured limit
mark truncated=true
retain exit metadata
```

Blob storage is a later extension.

---

# 56. S3

S3 is not required for the first passing V0.0.01.

It is a planned future storage layer for:

```text
large artifacts
large command outputs
screenshots
binary files
snapshots
```

The journal would store references/hashes rather than giant blobs.

Do not add S3 until a real V0 requirement exceeds reasonable SQLite/file limits.

---

# 57. Security Non-Goals

V0.0.01 does not yet solve:

```text
multi-tenant isolation
production cloud authority
provider-native workload federation
fine-grained tool permissions
trust realms
hazard cells
browser isolation
high-impact admin isolation
secret brokerage
long-lived delegated authority
```

These are known future architecture.

They should not be accidentally approximated with fake local protections that create a false sense of security.

---

# 58. Production Safety Principle

Because V0.0.01 has broad workstation access and immature authority controls, it should operate only in a development environment with no catastrophic credentials.

Do not connect the first V0 workstation to production PTG infrastructure.

Do not place broad cloud-root credentials on the machine.

Do not treat root access inside the VM as equivalent to provider-level administrative authority.

---

# 59. Acceptance Test A — End-to-End Agent Loop

Canonical task:

> Inspect your machine and tell me the operating system, CPU architecture, current working directory, and Git version.

Expected sequence:

```text
user_message
turn_started
model_request
model_response(tool_call)
tool_call
tool_result
possibly repeated tools
model_request
model_response(final)
assistant_message
turn_completed
```

The tool execution must happen on the EC2 Ubuntu workstation.

---

# 60. Acceptance Test B — Persistence

After the first successful turn:

```text
verify events in SQLite
kill craxii-server
allow systemd restart
reconnect Mac app
load previous conversation
```

The history must still be present.

---

# 61. Acceptance Test C — Continuity

After restart, ask:

> What Git version did you just tell me I have?

Craxii must answer from reconstructed durable history.

This proves that continuity does not depend on RAM.

---

# 62. Acceptance Test D — Tool Failure

Ask Craxii to inspect a nonexistent file.

Expected behavior:

```text
tool call
   ↓
structured filesystem error
   ↓
journal tool_result/error
   ↓
model receives result
   ↓
turn continues or terminates gracefully
```

The backend must not crash.

---

# 63. Acceptance Test E — Provider Failure

Simulate or induce a recoverable provider failure.

Expected behavior:

```text
provider error classified
bounded retry if applicable
journal/telemetry updated
turn eventually succeeds or fails honestly
```

No infinite retry loop.

---

# 64. Acceptance Test F — Duplicate Client Message

Send the same client message ID twice.

Expected behavior:

```text
one logical user_message
one turn
no duplicate execution
```

This verifies basic idempotency.

---

# 65. Success Metrics

V0.0.01 success is architectural, not benchmark-based.

Required measurements:

```text
turn completion success
model invocation latency
tool execution latency
journal write latency
input tokens
cached input tokens if available
output tokens
provider/model used
process restart recovery
duplicate-message handling
error classification
```

We are not yet optimizing these values.

They create the baseline for subsequent versions.

---

# 66. Explicit Non-Goals

V0.0.01 must not expand into:

```text
semantic memory
embeddings
vector database
memory projectors
automatic context compaction
background scheduling
multi-agent swarms
distributed workers
multi-region architecture
PostgreSQL migration
mobile apps
Windows app
full browser automation
production credential architecture
advanced sandboxing
autonomous deploys
team accounts
consumer auth
billing
complex model routing
learned routing
framework-driven orchestration
```

---

# 67. What Codex May Propose Changes To

Codex may propose changes to:

```text
module layout
Rust type design
DB schema details
error types
specific Axum route structure
WebSocket protocol details
SQLx usage
Tokio task ownership
provider adapter interface
tool trait/interface shape
tracing span structure
systemd configuration
```

Codex should not silently change:

```text
Craxii product identity
journal-as-authoritative-history
backend-owned agent loop
multi-provider model architecture
native-client direction
single-workstation V0 topology
explicit tool execution boundary
V0.0.01 scope
```

If Codex believes one of those should change, it should stop and present the architectural reason before implementation.

---

# 68. Change Proposal Standard

Any proposed architecture change should answer:

```text
Current design
Problem observed
Proposed change
Why the current design is insufficient
Alternatives considered
Migration cost
New failure modes
Effect on V0 scope
Whether the change is reversible
```

This document is a source of truth, not a prison.

Changes are allowed when they are reasoned and explicit.

---

# 69. Implementation Order

Recommended sequence:

```text
Phase 1 — Backend skeleton
Rust project
Tokio runtime
Axum health endpoint
typed config
tracing bootstrap

Phase 2 — Persistence
SQLx
SQLite
migrations
journal repository
conversation reconstruction

Phase 3 — Model layer
canonical model types
capability registry
selection policy
OpenAI adapter
Responses API integration

Phase 4 — Agent loop
turn state
context assembler
explicit inference loop

Phase 5 — Tools
registry
dispatcher
read_file
run_shell
structured tool results

Phase 6 — Streaming API
WebSocket protocol
turn events
assistant deltas
tool lifecycle

Phase 7 — Native macOS client
conversation rendering
input composer
WebSocket client
stream handling

Phase 8 — AWS deployment
EC2
Ubuntu
EBS
systemd
server-side secrets

Phase 9 — Acceptance tests
end-to-end
kill/restart
continuity
tool failure
provider failure
duplicate input
```

---

# 70. Final Architecture Diagram

```text
┌─────────────────────────────────────────────────────────────┐
│                        macOS Client                         │
│                  Swift + SwiftUI + AppKit                  │
│                                                             │
│  conversation UI ── input ── stream rendering              │
└──────────────────────────┬──────────────────────────────────┘
                           │
                    HTTPS / WebSocket
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                         AWS EC2                             │
│                  Ubuntu 24.04 LTS x86-64                   │
│                                                             │
│   systemd                                                  │
│      │                                                      │
│      ▼                                                      │
│   ┌──────────────────────────────────────────────────────┐   │
│   │               Craxii Rust Backend                  │   │
│   │                                                      │   │
│   │  Tokio                                               │   │
│   │    │                                                 │   │
│   │    ├── Axum / Tower / Hyper                         │   │
│   │    │       └── Client Protocol                      │   │
│   │    │                                                 │   │
│   │    ├── Agent Runtime                                │   │
│   │    │    ├── Turn State                             │   │
│   │    │    └── Context Assembler                      │   │
│   │    │                                                 │   │
│   │    ├── Model Layer                                  │   │
│   │    │    ├── Model Selection Policy                 │   │
│   │    │    ├── Capability Registry                    │   │
│   │    │    └── Provider Adapters                      │   │
│   │    │         └── OpenAI Adapter (first)            │   │
│   │    │                │                               │   │
│   │    │              Reqwest                           │   │
│   │    │                │                               │   │
│   │    │                └──────────────► Model APIs     │   │
│   │    │                                                 │   │
│   │    ├── Tool System                                  │   │
│   │    │    ├── Tool Registry                          │   │
│   │    │    ├── Tool Dispatcher                        │   │
│   │    │    ├── read_file                              │   │
│   │    │    └── run_shell                              │   │
│   │    │                │                               │   │
│   │    │                ▼                               │   │
│   │    │        Ubuntu filesystem/processes             │   │
│   │    │                                                 │   │
│   │    ├── Journal                                      │   │
│   │    │    └── SQLx → SQLite WAL                      │   │
│   │    │                │                               │   │
│   │    │                ▼                               │   │
│   │    │           craxii.db                            │   │
│   │    │                │                               │   │
│   │    │                ▼                               │   │
│   │    │        Linux filesystem                        │   │
│   │    │                │                               │   │
│   │    │                ▼                               │   │
│   │    │              EBS                               │   │
│   │    │                                                 │   │
│   │    └── tracing                                      │   │
│   └──────────────────────────────────────────────────────┘   │
│                                                             │
│   /home/craxii/projects/                                   │
│        └── repos / workstation state                       │
└─────────────────────────────────────────────────────────────┘
```

---

# 71. Core Runtime Sequence

```text
1. User submits message from Mac
2. Backend validates message ID
3. Backend appends user_message event
4. Backend starts turn
5. Context Assembler reconstructs current model-visible context
6. Model Selection Policy chooses target model
7. Provider Adapter builds provider-native request
8. Reqwest sends request
9. Provider returns response
10. Adapter normalizes response

IF final text:
11. append assistant_message
12. append turn_completed
13. stream final event to Mac

IF tool call:
11. append tool_call
12. Tool Dispatcher validates arguments
13. Tool Registry resolves implementation
14. Tool runs on Ubuntu
15. append tool_result
16. add result to model-visible context
17. return to step 6
```

---

# 72. Source-of-Truth Hierarchy

When implementation details conflict, resolve them according to this hierarchy:

```text
1. Product invariants in this document
2. Explicit later architecture decisions
3. Current database migrations / protocol schema
4. Current Rust type contracts
5. Implementation details
```

Comments and stale code should not override explicit architecture.

---

# 73. V0.0.01 Definition of Done

V0.0.01 is complete only when this is true in real software:

```text
User opens native Mac app
        ↓
sends task
        ↓
Craxii persists it
        ↓
model reasons
        ↓
model asks for tools
        ↓
Craxii executes real Ubuntu tools
        ↓
results are persisted
        ↓
model continues
        ↓
final answer streams back
        ↓
backend is killed
        ↓
systemd restarts it
        ↓
conversation reconstructs
        ↓
Craxii correctly continues from durable history
```

At that point we have the first real Craxii runtime.

The next architectural problem is no longer “can an agent exist?”

It becomes:

> How should Craxii manage context and memory as the relationship grows?

That is the beginning of V0.0.02.
