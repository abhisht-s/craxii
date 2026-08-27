# Stage 6 canonical schema contract decision

## Date

2026-08-28

## Status

Accepted

## Context / problem

Stage 6 must turn the Stage 3 and Stage 4 domain contracts into a durable relational constraint
spine without pre-implementing the journal, attempt evidence, command behavior, scheduler, or
bootstrap. The schema must reject malformed scalar values and impossible projection shapes, remain
inspectable for drift, and give later transactions exact concurrency primitives.

## Decision

### Migration and inventory

- Migration `0001`, SQLx version `1`, description `core durable schema`, is the only Stage 6
  production migration. Migrations are forward-only; there is no down migration, `user_version`,
  custom schema-version table, or repair path.
- The exact Stage 6 production table inventory is `craxii_principals`, `workstations`, `workspaces`,
  `conversations`, `runtime_instances`, `client_devices`, `work_items`, `messages`, and
  `client_commands`.
- Every Stage 6 product table is `STRICT, WITHOUT ROWID`. SQLx-owned `_sqlx_migrations` remains in
  SQLx's own format and is not rewritten.
- All nine product tables remain empty after migration. Stage 7 owns the initial principal,
  workstation, workspace, conversation, root links, `work_item_inputs`, `journal_events`, and
  `stream_heads`.
- Stage 8 owns context manifests and sources, model invocations, tool executions, artifacts, and
  authority/evidence tables. Therefore `work_items.current_model_invocation_id` and
  `current_tool_execution_id` are canonical UUIDv7 text with partial uniqueness but deliberately
  have no Stage 6 foreign key.

### Scalar and JSON storage

- Every durable UUID is lowercase canonical hyphenated UUIDv7 `TEXT`. SQL enforces length,
  hyphens, version nibble, RFC variant nibble, and lowercase hexadecimal shape; Rust domain codecs
  perform the complete parse.
- Every timestamp is UTC RFC 3339 `TEXT` with exactly six fractional digits and a trailing `Z`,
  byte length 27. SQL enforces the exact digit/separator shape; `UtcTimestamp::parse_canonical`
  performs calendar validation.
- SHA-256 digests are lowercase 64-character hexadecimal `TEXT` and are validated by SQL and the
  `Sha256Digest` codec.
- JSON is `TEXT` guarded by `json_valid`. Production `serde_json` use is confined to strict,
  adapter-private persistence DTOs with unknown fields denied. It is not a domain or public wire
  dependency, and no SQLite JSON extension or additional SQLx feature is needed.
- Message `content_json` V1 is exactly
  `{"version":1,"blocks":[{"type":"text","text":"..."}]}` with one or more ordered text
  blocks. Text bytes are preserved without normalization. The stored digest is recomputed from the
  Stage 3 binary canonical-content grammar, never from JSON.

### Literals and checks

- Stored work states are `queued`, `running`, `waiting_on_model`, `waiting_on_tool`,
  `cancel_requested`, `completed`, `failed`, `cancelled`, and `interrupted`.
- Work terminal-reason codes are exactly `answered`, `refused`, `definite_normalized_error`,
  `provider_exhausted`, `invalid_model_output`, `lifecycle_limit`, `user_request`,
  `graceful_shutdown`, `runtime_ownership_lost`, `provider_outcome_unknown`,
  `tool_interrupted_before_dispatch`, `tool_outcome_unknown`, and `cleanup_unconfirmed`.
  Cancellation-request reason codes are exactly `user_request` and `graceful_shutdown`.
- Runtime states are `running`, `stopping`, and `stopped`. Stop reasons are `graceful_shutdown` and
  `startup_failure`. A stopping row has no `stopped_at` and may have no reason or
  `graceful_shutdown`; `startup_failure` is terminal-only. A stopped row requires both terminal
  fields; a running row permits neither.
- Client command types are exactly `message` and `cancel`. Message roles remain `user`,
  `assistant`, and `system`; user messages require device plus client-message provenance,
  assistant messages require a producing work, and system messages have neither.
- SQL checks enforce positive versions/ordinals/generations/PIDs/cursors, priority zero, JSON and
  scalar shape, current-attempt XOR, and the complete Stage 4 owner/current-attempt/timestamp/
  cancellation/terminal matrix. Direct `queued -> cancelled` remains legal with `started_at NULL`.
  Failed safe detail is required for `definite_normalized_error`, `invalid_model_output`, and
  `lifecycle_limit`, forbidden for `provider_exhausted`, and JSON-valid when present. Only safe
  NormalizedError fields are persisted; `InternalDetail` is never encoded.

### Relationships, ordering, and indexes

- Every Stage 6 foreign key spells `ON UPDATE RESTRICT ON DELETE RESTRICT`; there is no cascade,
  set-null, or physical-deletion behavior. Principal root links reference the primary conversation
  and default workspace and must be both null or both nonnull. Message device provenance has a
  real foreign key.
- V0 `work_items.conversation_id` and `conversation_work_ordinal` are nonnull. A conversation's
  ordinal is unique, and the queue index is
  `(conversation_id, state, conversation_work_ordinal, work_id)` to serve the Stage 10 smallest-
  ordinal claim path.
- The explicit production indexes are exactly the Stage 6 named set. Partial unique indexes enforce
  one active work per conversation, globally unique current model/tool identity while present,
  unique client-message provenance, and at most one assistant message per work.
- `ix_messages_conversation` is a membership/access index only. Canonical message ordering remains
  journal-derived in Stage 7. Timestamp and UUID order are explicitly non-authoritative.

### Guarded updates and containment

- Adapter-private conversation advancement guards conversation ID, expected `state_version`, and
  expected `next_work_ordinal`, increments both exactly once, and reloads in the same transaction
  to classify missing, stale version, or stale ordinal after a zero-row update.
- Adapter-private work mutation guards work ID, expected state/version/runtime owner/current model/
  current tool with NULL-safe exact comparisons, validates the next Stage 4 lifecycle shape,
  increments the version exactly once, prevents terminal resurrection, and reloads in the same
  transaction to classify the losing dimension. Neither helper commits or enters `StateStore` in
  Stage 6. There is no attempt-update primitive before Stage 8.
- Runtime SQLx `query` plus `Row::try_get` is the row boundary. SQLx rows, UUID/time codecs,
  `FromRow` domain implementations, query macros, and offline metadata do not escape the adapter.
  Malformed stored data fails closed without defaults or coercion.

### Schema compatibility and drift

- Both schema compatibility constants advance to `1`. A valid version-1 database classifies as
  `current`; fresh and Stage 5 metadata-only databases migrate to current; reopening current is
  idempotent. A valid contiguous migration history extending above 1 is `newer_schema`.
- SQLx migration metadata is authoritative for history. Version 1 must have the embedded
  description, canonical success value, and exact embedded 48-byte checksum established through
  SQLx's migration manifest. Gaps, dirty rows, malformed metadata, or checksum mismatch are
  inconsistent.
- Current-schema validation requires `user_version = 0`, the exact product table and named-index
  sets, no triggers/views/unexpected product objects, and a deterministic structural fingerprint
  covering normalized `sqlite_schema` SQL plus `table_xinfo`, `foreign_key_list`, `index_list`,
  `index_xinfo`, strictness, and without-rowid state. Drift is inconsistent and is never repaired.

## Rationale

The database is the last local concurrency and corruption boundary. Strong SQL shapes make legal
Stage 4 transitions representable while excluding silent enum/default coercion. Adapter-private
codecs preserve the richer Rust validation layer, and explicit deferrals keep Stage 6 from
manufacturing journal or attempt authority before those records exist.

## Consequences / tradeoffs

- The migration is verbose because every UUID and timestamp column carries a local shape check.
  That cost buys corruption detection without SQLite extension or SQLx UUID/time coupling.
- Exact structural drift validation intentionally rejects manual schema edits, including edits that
  seem compatible. The bundled SQLite version and forward migration are the supported change path.
- `WITHOUT ROWID` makes every product primary key explicit and compact but prevents reliance on
  hidden rowids. Message order must come from the Stage 7 journal as intended.
- Cyclic principal root references require Stage 7 bootstrap to insert the principal with both
  links null, insert the referenced rows, then set both links in its journal-aware transaction.

## Rollback / change path

- Migration `0001` is immutable after release. A durable change requires an architecture amendment,
  a new forward migration, compatibility-ceiling update, embedded checksum/manifest update, and
  drift/migration tests. Destructive change uses copy-and-verify plus the architecture's backup
  precondition; it never edits or rolls back version 1.
- New message block versions use a new private storage DTO and content-version grammar while
  retaining V1 decode. Stage 7/8 foreign keys are added only when their owned tables arrive.
- New states, reasons, command types, indexes, delete behavior, or ordering authority are durable
  compatibility changes and therefore require the same architecture-first path.
