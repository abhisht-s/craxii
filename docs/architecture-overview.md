# Architecture overview

This document describes the architecture implemented in the public source. It is intentionally a contributor-level overview, not a promise of future deployment or product design.

## System boundary

```text
native macOS diagnostic client
              |
       HTTP commands/snapshot
       WebSocket event delivery
              |
      authenticated backend
      /        |         \
 SQLite     scheduler    model provider
 state       agent loop     adapter
                |
         context + tools
                |
       local workstation
```

The Rust backend is the canonical authority. The macOS client maintains a replaceable local projection; it does not decide durable truth. Provider and workstation behavior enter the application through explicit ports and adapters.

## Backend composition

Startup validates TOML configuration, loads restricted credential files, binds the HTTP listener, opens the SQLite runtime, verifies migrations and application consistency, initializes the artifact store and workstation, performs recovery, then exposes readiness. Startup records build/runtime evidence and fails closed on incompatible schema, corrupt state, unsafe workstation setup, or invalid provider composition.

The backend is layered around domain types, application services, ports, and adapters:

- Domain types define identities, content, lifecycle, commands, journal records, model evidence, and execution outcomes.
- Application services implement authentication, command admission, projection, scheduling, context assembly, model invocation, tool execution, publication, and lifecycle coordination.
- Ports define storage, clock, provider, artifact, observation, and workstation boundaries.
- Adapters supply SQLite, HTTP/WebSocket, OpenAI, deterministic scripted-provider, telemetry, local artifacts, and local-workstation behavior.

## Durable state and journal projection

SQLite stores canonical entities and attempts as well as an ordered journal. State mutations and the corresponding journal facts commit together. Projected public state and public durable events are derived from committed records, not optimistic client state.

On startup, the runtime verifies the schema and cross-table consistency, checks referenced artifacts, identifies orphan artifacts, and reconciles interrupted work and ambiguous model/tool outcomes conservatively. The system does not assume an external side effect failed merely because local completion evidence is missing.

## Scheduler and agent loop

Accepted messages create durable queued work. The scheduler claims eligible work and drives a bounded agent loop. Each iteration assembles causal context, selects a configured model target, records the exact invocation manifest/evidence, consumes ordered provider output, and either commits an assistant response, executes validated tools, or records a terminal/ambiguous outcome.

Configured limits bound work duration, model steps and attempts, tool calls, output items, argument sizes, provider timeouts, and tool resources.

## Model providers and context

The provider port separates application semantics from provider wire formats. The implemented production adapter targets the OpenAI Responses API; deterministic tests use a scripted provider implementing the same contract.

Context assembly reads canonical conversation and prior model/tool evidence, applies token limits, includes the registered tool definitions, and produces a stable manifest. Provider-specific identifiers and opaque continuation material remain evidence, not public protocol state.

## Tools and workstation

The current tool registry exposes `read_file` and `run_shell`. Inputs are closed, typed, and size-bounded before dispatch. The local-workstation adapter resolves logical paths against the configured workspace, performs bounded reads, and runs foreground processes with explicit timeouts, output capture, artifact overflow handling, cancellation, and recovery observations.

Child processes receive a clean environment; configured inherited variables are currently forbidden. Administrative execution is separately configured and capability-probed on supported Linux hosts. It is disabled in the local development fixture.

## HTTP and WebSocket roles

HTTP provides unprotected liveness/readiness checks and authenticated bootstrap, message, and cancellation operations. Commands use stable client-generated identities and idempotency keys so safe retries resolve to the original committed outcome or a conflict.

WebSocket `/v1/events?after=<cursor>` is server-to-client delivery. After authentication, the server replays durable public events after the requested cursor through a fixed high-water mark, emits `sync.complete`, then continues with live durable events. Draft events are ephemeral, lossy, and cursorless.

## Draft and canonical state

Streaming assistant drafts improve observability but never become durable truth. A client drops drafts on reconnect, abandonment, or canonical terminal events. Only a committed assistant message and its durable event represent the canonical answer.

The native client follows the same separation: it persists endpoint/profile, command, binding, and replay-cursor state atomically; keeps bearer credentials in Keychain; rebuilds the canonical projection from bootstrap and durable events; and holds drafts only in memory.
