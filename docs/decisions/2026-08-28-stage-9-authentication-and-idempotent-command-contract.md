# Stage 9 authentication and idempotent command contract

## Status and scope

Accepted for Craxii V0.0.01 Stage 9 on 2026-08-28. This record freezes offline device
provisioning/authentication, canonical V1 command hashes, atomic message acceptance, idempotent
cancellation, insert-only command receipts, and V3 startup consistency. It authorizes no Stage 10
runtime/scheduler/recovery behavior, Stage 11 HTTP/WebSocket transport, or Stage 17 provider/loop/
assistant-completion behavior.

## No-migration decision

Stage 9 adds no migration, table, index, foreign key, trigger, view, schema rebuild, or event kind.
The supported schema ceiling remains `3`; production migrations remain exactly `0001`, `0002`, and
`0003`; and the inventory remains 17 product tables, 40 named indexes, 28 journal event kinds, and
zero triggers/views. All three migration byte sequences/checksums and the V1, V2, and V3 structural
fingerprints remain frozen. Stage 9 is behavior, dependency-neutral types/ports, strict codecs,
named SQLite transactions, offline composition, consistency verification, and tests over V3.

## Device credential and raw-secret contract

- Production token entropy is exactly 32 bytes filled by the operating-system CSPRNG through the
  direct `getrandom` `0.4` dependency.
- Textual encoding is exactly 64 lowercase hexadecimal ASCII characters. Input grammar is
  `[0-9a-f]{64}` with no prefix, uppercase, whitespace, embedded ID, or version.
- The persisted digest is lowercase hexadecimal `SHA-256(exact 64 accepted ASCII bytes)`. Neither
  provisioning nor authentication hashes the decoded original random bytes.
- `BearerToken` owns private raw text, has redacted `Debug`, no revealing `Display`, no Serde, and
  no routine `Clone`. Narrow internal operations support hashing and one-time issuance output.
- There is no memory-zeroization claim. Raw bearer material is forbidden from SQLite/WAL command
  data, journal, messages, Work, artifacts, request hashes, telemetry, errors/panics, child
  environments, and diagnostics.
- Production generation has no weak/pseudorandom fallback and never uses a UUID as a credential.
  Deterministic tests inject already validated fixture material without replacing the production
  generator.

`DeviceDisplayName` preserves exact Unicode and internal spacing, is 1 through 128 UTF-8 bytes,
contains no control characters, and has no leading/trailing whitespace. It is not silently trimmed,
case-folded, or Unicode-normalized.

## Provision, list, revoke, and rotation semantics

Provision creates a new canonical `DeviceId`, immutable display name, unique token digest,
`created_at`, null `last_seen_at`, and null `revoked_at`. It commits before writing the raw token
once to the provision command's dedicated stdout result line. Safe metadata may use stderr. If
stdout fails after commit, the secret is unrecoverable and is never reissued from storage; the
operator provisions a replacement and may revoke the unreachable device.

List returns only device ID, display name, active/revoked status, and creation/last-seen/revocation
times. It never returns a token digest. Revoke is one-way: active becomes revoked at the supplied
server timestamp; an already-revoked device returns its original timestamp without rewriting it;
unknown ID is a safe not-found. No device operation emits a journal event or uses
`client_commands`.

Rotation is replacement based: provision a replacement `DeviceId`/token and revoke the old device.
V0 has no in-place `token_hash` or display-name update. Zero devices is valid and bootstrap never
auto-provisions one.

The offline `craxii-admin device provision|list|revoke` composition path validates configuration
and state-root rules, acquires the same exclusive Craxii process lock, opens/migrates/checks V3,
loads or validates the existing canonical bootstrap topology as appropriate, and runs application
consistency. It creates no runtime row, never marks ready, and starts no scheduler, recovery,
provider, tool, or public transport.

## Authentication flow and last-seen evidence

The application order is exact grammar validation, SHA-256 of accepted bearer-text bytes, lookup by
digest, full-length XOR-accumulating comparison of the returned 32-byte digest, rejection of a
revoked row, and return of `AuthenticatedDevice { device_id }`. Authentication always precedes
idempotency lookup. Missing, malformed, unknown, and revoked credentials collapse to the same safe
`authentication_failed` result without a credential oracle or database detail.

After successful authentication, the authenticator may best-effort touch `last_seen_at` outside the
command transaction. The SQLite update is monotonic and writes only when the new timestamp is
later. Failure neither revokes authentication success nor changes command correctness and creates
no journal event. This evidence-only write is permitted alongside the one-way `revoked_at` update;
no other device update is a production capability.

## Command identities and hash version

`IdempotencyKey` accepts only canonical lowercase UUIDv7 text. Scope is the pair
`(DeviceId, IdempotencyKey)` and spans both durable kinds, exactly `message` and `cancel`. Different
devices may reuse the UUID. A message key equals its `ClientMessageId`; a cancellation key equals
its `ClientCommandId`. Key/body mismatch is validation failure before any command lookup/write.
Cross-kind or changed-material reuse under one scoped key is conflict.

`CommandRequestHash` is semantically separate from token and message-content digests.
`CommandHashEncodingVersion` is exactly 1 and independent of schema version. V1 bytes are:

1. ASCII `craxii.command`.
2. One byte `0x01`.
3. For every following field, an unsigned 64-bit big-endian length and the exact field bytes.

The message fields are the existing protocol version as exactly eight unsigned 64-bit big-endian
bytes, ASCII `message`, canonical `ConversationId` text, canonical `ClientMessageId` text, and exact
`MessageContent::canonical_bytes()`. Cancellation fields are protocol version in the same form,
ASCII `cancel`, canonical `ClientCommandId` text, and canonical target `WorkId` text. SHA-256 of the
whole sequence is the request hash.

The encoding excludes device identity, a separately repeated key, bearer/token hash, server time,
server-generated message/work/event IDs, message Work ID, HTTP request ID/body, and JSON shape.
Known vectors permanently pin V1. A future V2 must retain V1 replay/recomputation.

## Insert-only client-command semantics

`client_commands` has no pending state and no production update/delete capability. A row is inserted
last in the same transaction as a successful mutation or accepted valid no-op; presence means the
logical response is final and durable. No row is stored for authentication/validation failure,
message key mismatch, invalid content, idempotency conflict, unknown/foreign cancellation target,
transient database error, or detected storage inconsistency.

On a scoped-key hit, exact kind/hash match runs strict versioned decoding and returns
`CommandOutcome::Replay`; mismatch returns `idempotency_conflict`. A new committed command returns
`CommandOutcome::Committed`. The stored DTO never contains a duplicate flag; Stage 11 may later add
one to a convenience projection. A uniqueness loser outside the normal write coordinator reloads
the winner and applies the same exact rule.

The strict message V1 receipt fields are `version`, `conversation_id`, `message_id`, `work_id`,
`work_ordinal`, `work_state` (`queued` only), and positive `committed_cursor`, stored with status
`202`. The cancellation V1 fields are `version`, `work_id`, `resulting_work_state`,
`cleanup_pending`, and positive `committed_cursor`; its state/status/pending combinations are
closed. The adapter rejects unknown fields/versions/kinds/states, malformed IDs/digests/timestamps,
invalid cursors, status contradictions, or row/JSON cursor mismatch as `storage_inconsistent`.

## Atomic message transaction and event order

The application validates authenticated device, protocol, key/`ClientMessageId`, conversation, and
message content, computes V1 hash, supplies a command timestamp, and pre-generates candidate
`MessageId`, `WorkId`, two `JournalEventId` values, and explicit Work-derived `CorrelationId`.
`CorrelationId::for_work` copies the Work UUID bytes into a distinct semantic type; there is no
general interchangeability.

Under `WriteCoordinator` plus `BEGIN IMMEDIATE`, a new message transaction:

1. Performs scoped command replay/conflict lookup.
2. Loads and verifies the sole principal, active primary conversation, and default workspace.
3. Reads conversation next ordinal `N` and state version `V`.
4. Constructs/inserts the immutable user message with exact content/hash, authenticated
   device/client-message identity, and command timestamp.
5. Appends `message.accepted` in the conversation stream, caused by `conversation.created`, actor
   `user`/authenticated `DeviceId`, with Work-derived correlation.
6. Constructs/inserts one conversational queued Work at ordinal `N`, version `1`, priority `0`,
   default workspace, command `created_at == queued_at`, null runtime/current-attempt/cancellation/
   terminal facts, and the same correlation.
7. Inserts exactly one trigger input at ordinal `1`, attached by `user`, referencing the acceptance
   event.
8. Appends `work.queued` in the Work stream, caused by `message.accepted`, actor
   `craxii`/`CraxiiId`.
9. Guardedly advances conversation ordinal `N -> N+1` and version `V -> V+1`.
10. Builds the receipt at the `work.queued` journal offset, inserts the command row last, and
    commits once.

The existing Message and Work constructors/codecs remain authoritative. Candidate IDs lost on
rollback are harmless; replay uses persisted winner IDs. Ordinals follow transaction serialization,
are contiguous across committed commands, and do not follow timestamps/UUIDs. Each Work has only
its own acceptance trigger, so a later message cannot leak into an earlier Work input set. The
client-message uniqueness index remains defense in depth and an alternate duplicate fails closed
without an orphan command receipt.

Every precommit failure boundary rolls back Message, Work, input, both events, stream-head changes,
conversation ordinal/version, and command row. AUTOINCREMENT offset gaps are allowed. Message and
queue event payload meanings/versions are unchanged.

## Cancellation matrix and receipts

Cancellation reloads authoritative Work inside the transaction and delegates the state decision to
the existing Stage 4 cancellation logic. Authorization is the sole V0 principal topology: any
active authenticated device may control the principal's Work, regardless of message-creating
device.

| Current Work state | Stage 9 decision | Event | Status | Cleanup pending |
| --- | --- | --- | ---: | ---: |
| `queued` | Direct `cancelled`, version +1, terminal reason `user_request` | `work.cancelled` | 200 | false |
| `running` | `cancel_requested`, version +1, preserve runtime/attempt | `work.cancel_requested` | 202 | true |
| `waiting_on_model` | `cancel_requested`, version +1, preserve runtime/model | `work.cancel_requested` | 202 | true |
| `waiting_on_tool` | `cancel_requested`, version +1, preserve runtime/tool | `work.cancel_requested` | 202 | true |
| `cancel_requested` | Accepted no-op | none | 202 | true |
| `completed` | Accepted stable no-op | none | 200 | false |
| `failed` | Accepted stable no-op | none | 200 | false |
| `cancelled` | Accepted stable no-op | none | 200 | false |
| `interrupted` | Accepted stable no-op | none | 200 | false |

Unknown or foreign Work returns `target_not_found` and persists nothing. Event-bearing transitions
use user/authenticated-Device actor, existing Work correlation, and the latest authoritative Work
stream event as causation. Queued cancellation leaves `started_at`/runtime/attempt fields null.
Active cancellation sets the existing `user_request` request timestamp/reason but does not signal a
runner, cancel a provider, kill a process, mutate an attempt, clear ownership, classify an outcome,
or complete Work.

A new valid no-op key is committed even without Work/event mutation. Its cursor is the positive
journal high-water observed inside the same transaction. Exact replay preserves that original
cursor. No-op startup proof is limited by V3: require that cursor to name an existing journal row no
later than current head, require the Work, and require receipt state compatible with immutable
terminal current state or reconstructable cancel-requested history. Event-bearing receipts require
the exact cancellation event/cursor. This is the strongest sound invariant without a new schema.

Different new keys against one queued Work produce one `work.cancelled`; later commands see terminal
no-op. Different keys against active Work produce one `work.cancel_requested`; later commands see
already-requested no-op. Same key/same material replays; same key/changed Work conflicts.

Stage 9 is the first emitter of `work.cancelled` for queued direct cancellation. Stage 10 may emit
it only after confirmed cleanup of active `cancel_requested` Work. Existing event meanings and
versions do not change.

## Failpoint and lost-response boundary

Stage 9 does not call the Stage 2 production names `after_message_transaction_commit` or
`after_cancel_requested_commit`; Stage 10.4 owns activation. Adapter-private `cfg(test)` rollback
hooks cover every precommit boundary without expanding production vocabulary. The exact future
callsite is immediately after a successful command-store return/commit and before postcommit hint
delivery/transport response.

Lost-response tests commit, discard the application result, close/reopen a file-backed store, and
resend exact device/key/request. Message, queued cancellation, and active cancellation must replay
the identical stored IDs/state/status/cursor without another row or event.

## Startup consistency additions

Every `client_devices` row must decode a canonical `DeviceId`, valid display name, canonical SHA-256
digest, and canonical timestamps; optional last-seen/revoked times cannot precede creation. Every
`client_commands` row must name an existing device, use canonical UUIDv7 key and known command kind,
decode a canonical request hash/creation time/positive existing cursor no later than journal head,
and pass the strict receipt codec.

Message commands additionally require key-as-`ClientMessageId`, exact device/message identity,
matching receipt Message/Work/conversation/ordinal, exact trigger input, acceptance/queue events and
causation/correlation/actors, hash recomputation from stored content, and cursor equal to the queue
event. Cancellation commands require key-as-`ClientCommandId`, existing target Work, hash
recomputation from key/Work/protocol, coherent receipt status/state/pending, exact event cursor when
event-bearing, and the strongest sound no-op invariant above. Any contradiction fails startup
without repair.

## Dependency decision

`getrandom` `0.4` is already locked transitively through `uuid`; making it a direct dependency is
the smallest OS-CSPRNG surface and is expected to resolve to `0.4.3` without graph expansion. Use
is feature-minimal and limited to one exact 32-byte fill. Its registry source, dual MIT/Apache-2.0
license, Rust compatibility, platform implementation, lack of a project-specific build script or
native library, and advisory status are governed by the existing Cargo lock/deny policy. No
alternative randomness/secret crate is added.

## Tradeoffs and change path

The V3 schema cannot prove the precise historical head observation of a no-op cancellation beyond
an existing cursor plus compatible reconstructable topology; adding a dedicated no-op event or
observation table would make that stronger but is deliberately rejected for Stage 9. Provision
stdout loss can strand a committed digest because raw recovery would violate the storage contract;
replacement provisioning is the safe operator path. Best-effort last-seen can be stale because it
is evidence, not authentication or command truth. SHA-256 bearer digest storage is intentionally a
high-entropy bearer-token design, not a human-password design.

Any later change to token grammar/hash, command canonical bytes, response DTOs, command-row
mutability, cancellation receipts, or emitter ownership requires an architecture amendment and a
new versioned compatibility path before implementation. Migration `0004` is not implicitly
authorized by this record.

## Deferred ownership

- Stage 10 owns runtime rows, FIFO claims, runner tasks, active cancellation signaling/cleanup,
  graceful shutdown/recovery, readiness, and named crash-failpoint activation.
- Stage 11 owns Axum/Tower, bearer extraction/HTTP `401`, command endpoints and JSON/status mapping,
  public bootstrap/replay, duplicate convenience flags, and WebSocket delivery.
- Stage 17 owns provider calls, agent-loop decisions, terminal assistant messages, and
  `CompletionStateStore`.

Stage 9 performs no scheduler claim, runtime ownership mutation, provider/tool effect, public
transport, or assistant completion.
