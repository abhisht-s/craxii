# Public protocol overview

Craxii currently implements protocol version `1`. The Rust DTOs in `backend/src/protocol.rs` and language-neutral JSON in `backend/tests/fixtures/protocol-v1/` are the executable contract; this document is a contributor-oriented overview.

## Transport and authentication

| Method | Path | Authentication | Purpose |
| --- | --- | --- | --- |
| `GET` | `/health/live` | None | Process liveness. |
| `GET` | `/health/ready` | None | Application readiness/draining/fatal state. |
| `GET` | `/v1/bootstrap` | Bearer token | Canonical snapshot and snapshot cursor. |
| `POST` | `/v1/conversations/{conversation_id}/messages` | Bearer token | Commit a user message and queue work. |
| `POST` | `/v1/work-items/{work_id}/cancel` | Bearer token | Request or observe cancellation. |
| `GET` | `/v1/events?after={cursor}` | Bearer token, WebSocket upgrade | Replay and live event delivery. |

Protected requests require exactly one `Authorization: Bearer <token>` header. Mutation bodies use `Content-Type: application/json` and exactly one `Idempotency-Key` header. Responses are JSON except after a successful WebSocket upgrade.

## Bootstrap

Bootstrap returns the canonical Craxii identity, primary conversation, messages, work items with public tool summaries, unresolved ambiguous outcomes, and a `snapshot_cursor`. A client atomically replaces its canonical projection with this snapshot, then opens event delivery after that cursor.

Bootstrap is bounded. Oversized or internally inconsistent state fails instead of returning a partial snapshot.

## Message submission

The request is closed and versioned:

```json
{
  "protocol_version": 1,
  "client_message_id": "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa001",
  "content": [
    { "type": "text", "text": "Hello, Craxii." }
  ]
}
```

The `Idempotency-Key` header must equal `client_message_id`. On success, HTTP `202` returns the durable message ID, work ID/state/ordinal, committed cursor, and whether the response is a duplicate replay. Retrying the exact command is safe; using the key for different command material returns `idempotency_conflict`.

## Cancellation

Cancellation posts a versioned body to a specific work item:

```json
{
  "protocol_version": 1,
  "client_command_id": "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa004"
}
```

The `Idempotency-Key` must equal `client_command_id`. The response reports resulting work state, committed cursor, duplicate status, and whether cleanup is still pending. Cancellation never fabricates a terminal outcome when external cleanup is unconfirmed.

## Durable replay and live delivery

A nonnegative canonical decimal `after` cursor selects durable events strictly after that point. Cursor `0` means replay from the beginning. Values with signs, leading zeroes, non-digits, or values beyond signed 64-bit range are rejected.

On connection, the server captures a durable high-water mark, replays public durable events through it in order, then emits:

```json
{
  "protocol_version": 1,
  "delivery_kind": "ephemeral",
  "event_type": "sync.complete",
  "through_cursor": 8
}
```

After `sync.complete`, durable commits continue on the same stream. Broadcast notifications are only latency hints; the database journal remains the replay authority. The WebSocket is server-delivery-only, and client text/binary data is rejected.

Durable event envelopes include protocol version, `delivery_kind: "durable"`, event ID, monotonically increasing cursor, event type, optional conversation/work IDs, recorded timestamp, and a bounded public payload. Published types cover accepted messages, work lifecycle, tool execution summaries, committed assistant messages, and relevant recovery summaries.

Clients deduplicate durable events by event ID/cursor, apply only increasing cursors, and bootstrap again if their local binding or projection is invalid.

## Ephemeral drafts

Draft events have `delivery_kind: "ephemeral"` and a null cursor. The implemented types are:

- `assistant.draft_started`;
- `assistant.draft_delta`, with ordered text or refusal deltas; and
- `assistant.draft_abandoned`, with a bounded reason.

Drafts are lossy, connection-local presentation state. They are not replayed and must be discarded after reconnect, abandonment, or a canonical terminal/assistant event. Only `assistant.message_committed` is durable answer state.

## Errors and compatibility

Public errors contain protocol version, a stable code and safe message, retryability, and a server-generated request ID. Internal database, provider, credential, content, path, and execution details are not serialized into errors.

Protocol-v1 request DTOs reject unknown fields, unsupported versions, malformed UUIDs, and unsupported content kinds. Consumers must not assume unknown future event types are safe to apply. Additive optional response fields may be introduced only when older clients can ignore them safely; breaking request, identity, cursor, or event-semantic changes require a new protocol version.
