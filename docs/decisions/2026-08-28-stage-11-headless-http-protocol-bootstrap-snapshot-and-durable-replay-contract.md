# Stage 11 headless HTTP protocol, bootstrap snapshot, and durable replay contract

## Status and scope

Accepted for Craxii V0.0.01 Stage 11 on 2026-08-28. This record freezes protocol v1,
authenticated HTTP command admission, public bootstrap projection, durable replay, WebSocket
replay/live handoff, bounded delivery, server ownership, and mutation shutdown quiescence. It
authorizes no Stage 12 workstation capability or file API, Stage 13 process execution, Stage 14
Tool Registry, Stage 15 provider expansion, Stage 16 context assembly, Stage 17 real WorkRunner,
Stage 20 draft streaming, native client, TLS, Caddy, or AWS work.

## Dependency decision

Stage 11 adds exact compatible declarations for `axum 0.8.9` with default features disabled and
`http1`, `json`, `matched-path`, `query`, `tokio`, `tracing`, and `ws`; `tower 0.5.3` with default
features disabled and `limit`, `util`; and `tower-http 0.7.0` with default features disabled and
`limit`, `sensitive-headers`, `set-header`, `timeout`, and `trace`. The existing Tokio declaration
adds only `net` and `io-util`.

Tests add `tokio-tungstenite 0.29.0` with default features disabled and `connect`, plus
`futures-util 0.3.34` with default features disabled and `sink`, `std`. Production does not add a
direct `hyper`, `http`, `tokio-tungstenite`, TLS, or CORS crate. Each direct dependency has a
separate registry record covering its locked version, license, MSRV, feature/update policy,
advisory result, and native/build/unsafe implications.

Axum resolves `matchit 0.8.4`, whose conjunctive license expression is `MIT AND BSD-3-Clause`.
Both are OSI-approved; Stage 11 narrowly adds `BSD-3-Clause` to the repository cargo-deny license
allowlist. It adds no source exception, advisory ignore, crate skip, or wildcard allowance.

## Route and protocol surface

The complete Stage 11 route inventory is:

```text
GET  /health/live
GET  /health/ready
GET  /v1/bootstrap
POST /v1/conversations/{conversation_id}/messages
POST /v1/work-items/{work_id}/cancel
GET  /v1/events?after=<cursor>   WebSocket upgrade
```

Only health is unauthenticated. Every `/v1` route requires authentication. There is no WebSocket
mutation path and no diagnostic or admin HTTP route. JSON bodies and envelopes use the public
`protocol_version` integer `1`; internal schema, journal payload, command-hash, and projection
versions never become public protocol versions. Requests reject unknown fields, unsupported
versions, noncanonical UUIDv7 values, and unknown enum values. Responses remain stable and may
gain additive optional fields; clients must ignore response fields they do not understand.

## Authentication and request identity

Each protected request or upgrade carries exactly one `Authorization` field with grammar
`Bearer` (ASCII case-insensitive), exactly one ASCII space, and exactly 64 lowercase hexadecimal
token characters. Tabs, commas, extra segments, duplicate fields, uppercase hexadecimal, and
trailing bytes are invalid. Stage 9 `BearerToken` parsing and `DeviceAuthenticator` remain the
authorities. Missing, malformed, unknown, revoked, digest-mismatched, and credential-store
failures all return the same `401 authentication_failed`, fixed safe message, and
`WWW-Authenticate: Bearer` response. The raw bearer exists only until authentication returns;
extensions retain only `AuthenticatedDevice`/`DeviceId`. It is never forwarded to CommandService,
persisted, or logged, and is forbidden from URL, query, body, and WebSocket subprotocol.

Every HTTP request and WebSocket upgrade receives a fresh server-generated UUIDv7 request ID.
Inbound `X-Request-Id` is ignored. HTTP responses include `X-Request-Id`; the ID is tracing/error
correlation only and is never a durable command correlation identity.

## Command requests, receipts, and admission

Message JSON contains exactly `protocol_version`, canonical UUIDv7 `client_message_id`, and a
nonempty ordered list of text content blocks. The path supplies `ConversationId`, authentication
supplies `DeviceId`, and the server clock supplies `accepted_at`. Exactly one canonical UUIDv7
`Idempotency-Key` must equal `client_message_id`. Transport never creates `MessageId`, `WorkId`,
command hash, correlation ID, causation, or journal IDs. The response exposes only
`protocol_version`, `message_id`, `work_id`, `work_state`, `conversation_work_ordinal`,
`committed_cursor`, and `duplicate`. Fresh and replayed success preserve stored HTTP `202`; exact
replay changes only the convenience `duplicate` value to true.

Cancellation JSON contains exactly `protocol_version` and canonical UUIDv7 `client_command_id`.
The path supplies `WorkId`, authentication supplies `DeviceId`, and the server clock supplies
`requested_at`. `Idempotency-Key` must equal `client_command_id`. The response exposes exactly
`protocol_version`, `work_id`, `work_state`, `committed_cursor`, `duplicate`, and
`cleanup_pending`. A newly active `cancel_requested` response is `202`; queued direct
cancellation and terminal no-op are `200`; replay preserves the stored status; absent Work is
`404`; semantic reuse conflicts are `409`. The Stage 9 stored receipt and hash contract is not
changed.

Bootstrap and replay are admitted after startup recovery in `live_unready` or `ready`. Message
submission requires `ready`. Cancellation is responsibility-reducing and is admitted in
`live_unready` or `ready` after recovery. All mutations reject `draining` and `fatal`. Production
has no real WorkRunner and therefore remains `live_unready`; message submission honestly returns
retryable `503`.

## Errors and transport mapping

There is one public error shape: `protocol_version=1` and `error` containing stable `code`, fixed
safe `message`, `retryable`, and the current server request ID. Central mapping owns at least
`400 invalid_request`, `401 authentication_failed`, `404 not_found`, `405 method_not_allowed`,
`409 idempotency_conflict`, `413 payload_too_large`, `415 unsupported_media_type`, retryable
`503 service_unavailable`/`overloaded`, retryable `504 command_timeout`, and `500 internal_error`.
No internal error string crosses the boundary. Storage consistency/invariant failures presented
as `500` mark health fatal and request controlled shutdown where ownership permits.

## HTTP bounds, middleware, host, and tracing

Mutation routes require `application/json`. The message body limit is 512 KiB and cancellation
body limit is 8 KiB before decoding; decoded combined text retains the existing 64 KiB UTF-8
limit. Active HTTP requests are bounded to 64 and active mutation requests to 16 without adding a
durable queue. Health timeout is two seconds, message/cancellation timeout ten seconds, bootstrap
timeout thirty seconds, and each replay database page timeout five seconds. WebSocket lifetime
has no generic HTTP timeout. A timeout after command commit is repaired by retrying the same
idempotency key.

Every response sets `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
`Authorization` is marked sensitive before trace middleware. Rust adds neither permissive CORS,
HSTS, TLS, nor forwarded-header trust. Host must match the configured loopback bind authority or
configured public authority; forwarded headers never influence auth, routing, URL generation,
request IDs, or authorization.

Trace fields are limited to request ID, method, matched route template, status, latency, replay
counts/ranges, and safe WebSocket outcome. Raw URI/query, authorization, request bodies, raw
headers, ordinary-info `DeviceId`, SQL/binds/path, provider/tool/output/artifact/filesystem data,
and internal errors are forbidden.

## Public bootstrap projection

`GET /v1/bootstrap` returns a new public DTO, never the internal `BootstrapSnapshot`. Its top level
is `protocol_version`, `snapshot_cursor`, `craxii`, `primary_conversation`, `messages`,
`work_items`, and `unresolved_outcomes`. Craxii exposes only ID, display name, and owner label.
Conversation exposes ID, kind, lifecycle, and creation time. Messages expose ID, conversation ID,
conversation stream sequence, role, content, optional client message/work IDs, and commit time.
Work exposes ID, conversation ID, work ordinal, all nine public states, trigger message ID,
lifecycle timestamps, optional safe terminal reason, cleanup pending, and public tool summaries.
It never exposes owner, state version, or current internal attempt IDs.

Tool summaries expose only execution ID, tool name, safe status/result class, timestamps, and
`outcome_unknown`; they omit arguments, output, CWD, paths, normalized raw errors, and runtime
identity. Closed unresolved warnings are `provider_outcome_unknown`, `tool_outcome_unknown`, and
`cleanup_unconfirmed`, each containing only client presentation IDs.

One SQLite read transaction first reads global journal head `H`, then reads every projection and
validates versions, identity, ordering, and nonpartial bounds. It closes before serialization or
network send. Messages are ordered by conversation stream sequence; Work by work ordinal; tools
by agent step then tool ordinal; warnings by work then tool ordinal. Bounds are 2,048 messages,
all queued/active Work plus at most 512 recent terminal Work, 2,048 tool summaries, 12 MiB source
message JSON, and 16 MiB encoded response. Any excess is `bootstrap_limit_exceeded`; nothing is
silently truncated.

## Replay cursor, store, mapping, and paging

`ReplayCursor` is a dependency-neutral wrapper over nonnegative global journal offsets. Wire text
is canonical unsigned decimal from `0` through `i64::MAX`. Zero means replay from the start.
Missing required, negative, signed, padded, floating, junk, overflow, noncanonical, or future
cursors are rejected. A cursor equal to current head is valid.

The replay port exposes only current high-water, an atomic public-bootstrap candidate snapshot,
and a bounded page after a cursor through a fixed high-water. It exposes no SQL, Axum, transport,
pool, or arbitrary query capability. Each SQLite page scans at most 128 underlying journal rows
and returns decoded typed candidates, `scanned_through`, and `has_more`. Scan progress advances
through filtered rows and empty public pages.

The explicit public allowlist is `message.accepted`, `work.queued`, `work.started`,
`work.waiting_on_model`, `work.waiting_on_tool`, `work.cancel_requested`, `work.cancelled`,
`work.completed`, `work.failed`, `work.interrupted`, `tool.execution_dispatching` mapped to
`tool.execution_started`, `tool.execution_completed`,
`tool.execution_interrupted_before_dispatch`, `tool.execution_outcome_unknown` mapped to
`tool.execution_finished`, `assistant.message_committed`, and nonempty public-safe
`runtime.recovery_performed`. Internal `work.resumed` maps to public `work.started` with
`transition_kind="resumed"` if introduced by a later writer. Unknown event kind/version whose
visibility cannot be proven fails closed; it is never silently skipped.

Initialization, runtime start/stop, model events, tool request/internal details, artifact/evidence,
provider bodies, context manifests, filesystem/output payloads, internal runtime/attempt facts,
and every event not allowlisted are omitted deliberately. Durable envelopes expose only
`protocol_version`, `delivery_kind="durable"`, event ID, cursor, public event type, optional
conversation/work IDs, recorded time, and safe payload. They never expose stream, correlation,
causation, state version, runtime/provider-call identity, or hashes. Delivery is ascending global
cursor; visible cursors may have gaps; clients resume strictly after their last applied cursor.

## WebSocket replay/live delivery

`GET /v1/events?after=<cursor>` authenticates only through the upgrade Authorization header.
Server application frames are zero or more durable envelopes, one strict ephemeral
`sync.complete` containing `protocol_version=1`, `delivery_kind="ephemeral"`,
`event_type="sync.complete"`, and `through_cursor`, then later durable envelopes. There is no hello
frame. Ping/Pong WebSocket control frames are the only heartbeat mechanism. Client text or binary
application frames are policy violations and close `1008` without a durable write. Controlled
shutdown uses `1001`; invariant/storage replay failure `1011`; slow consumer or transient overload
`1013`.

The application owns a `tokio::sync::broadcast<ReplayCursor>` of capacity 256. It carries only
committed cursor hints; SQLite remains truth. The reusable postcommit notifier publishes for fresh
message, fresh active cancellation, and fresh direct queued cancellation. Replays publish no new
commit. The SQLite adapter knows no WebSocket connection.

Each connection subscribes to both committed-cursor hints and shutdown before reading high-water
`R`, replays `(after,R]`, drains/discards hints through `R`, sends `sync.complete`, then enters live
mode. High-water read, each replay-page fetch, every frame send, `sync.complete`, and live wait are
shutdown-aware. Any hint, broadcast lag, or one-second fallback tick queries durable journal after
the last scanned cursor. Broadcast loss changes latency only. There is no busy loop.

At most 32 accepted pending upgrades plus active WebSocket connections are admitted. Craxii
reserves an identified pending ownership slot before returning Axum's upgrade response. The Axum
callback transfers that reservation into an active task owned by the connection supervisor, and a
completion guard reports normal return, upgrade failure/cancellation, or callback panic. Registry
ownership clears only after the supervisor observes the callback and active-task outcomes. Each
connection has an outbound queue of 16 frames. One durable public payload is at most 262,144 bytes
and one fully encoded frame at most 270,336 bytes. Queue/send stall is at most five seconds;
sustained pressure closes `1013`. A legitimate event that cannot fit is an invariant failure and
is not truncated. Each page query has a five-second timeout. Persistent replay/storage
inconsistency marks fatal and requests controlled shutdown; individual disconnects do not.
The deterministic transport send-stall seam is the authoritative bounded-pressure test: a finite
real client that merely stops reading cannot portably force `1013` because operating-system TCP
buffer capacity and autotuning are outside the protocol contract, so no timing-sensitive
real-socket pressure assertion is used.

## Server ownership, startup, and shutdown

Bootstrap binds the existing configured loopback `server.bind_address` before runtime creation.
Bind failure returns a redacted startup error and creates no RuntimeInstance. It starts accepting
application traffic only after migration, artifact, identity, consistency, runtime creation, and
recovery are coherent. Bootstrap owns the listener, server task, connection tasks, shutdown/join,
and database lifetime. An explicit supervisor observes the server execution child and distinguishes
controlled completion from unexpected early return, serve failure, and child panic. It installs no
second signal handler; the Stage 10 controller remains the only shutdown authority. Unexpected
shared-server completion marks fatal, closes mutation admission, requests that existing
controller, and preserves the original typed serve or join cause while cleanup proceeds. Ordinary
connection exit is isolated.

Transport mutation admission is distinct from scheduler claim admission. Shutdown latches, marks
nonfatal health draining, stops listener acceptance and new upgrades, closes mutation admission,
awaits explicit async mutation quiescence, then closes/awaits claim admission and commits
`runtime.stopping`. The admitted mutation section covers CommandService commit/no-commit and
postcommit effect ownership. Existing pending upgrades and active WebSockets observe shutdown and
close `1001` where feasible; already entered nonmutation work may finish within the existing
deadline. All callback completion records and server/connection task joins are observed before
SQLite closes. Therefore no command event can commit after `runtime.stopping`.

The repository checker is a structural guard, not a second compiler or runtime proof. It checks
the method/path inventory, `/v1` authentication placement, source-scoped snapshot/replay
relationships, permanent behavioral-test inventory, and Stage 12+ source boundaries. Deterministic
Rust unit and integration tests remain authoritative for snapshot races, replay semantics,
upgrade/connection ownership, failure supervision, and shutdown behavior.

## No-migration decision and rollback

Stage 11 is code, protocol, bounded query, and test work over schema V3. It adds no migration
`0004`, table, index, foreign key, trigger, view, schema rebuild, or journal event kind. The schema
ceiling remains `3`, with exactly three migrations, 17 product tables, 40 indexes, 28 event kinds,
and zero triggers/views. All migration bytes and V1/V2/V3 structural fingerprints remain frozen.

Rollback is code-only after a controlled stop. A Stage 10 binary can read the same V3 database;
it simply does not expose Stage 11 transport. No protocol downgrade or data rewrite is automatic.

## Deferred ownership

- Stage 12 owns workstation capability and `read_file` behavior.
- Stages 13–14 own process execution and Tool Registry/execution.
- Stages 15–17 own provider abstractions, context, real WorkRunner, agent loop, and completion.
- Stage 20 owns ephemeral model draft delivery beyond `sync.complete`.
- Native clients, TLS/Caddy, AWS, deployment, and public network policy remain later stages.
