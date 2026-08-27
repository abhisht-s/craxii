# Stage 7 journal and projector contract decision

## Date

2026-08-28

## Status

Accepted

## Context / problem

Stage 7 must add durable transition evidence, deterministic reconstruction, and the one-time V0
root topology without pre-implementing attempts, client commands, scheduling, recovery, or public
replay. The implementation must distinguish global replay order from per-aggregate order, make
bootstrap atomic and idempotent, and fail closed when journal evidence contradicts current rows.

## Decision

### Migration and inventory

- Migration `0002`, SQLx version `2`, description `journal and work inputs`, creates exactly
  `journal_events`, `stream_heads`, and `work_item_inputs`. The compatibility ceiling is `2`.
- There is no bootstrap metadata table, projector checkpoint table, trigger, view, hash chain,
  public visibility/payload column, custom schema-version table, or Stage 8-or-later table.
- Migration-only state remains empty. Bootstrap is one later runtime transaction.
- The SQLx SHA-384 checksum of `0002_journal_and_work_inputs.sql` is
  `677379cfb19c61d45c6a61bdeb978539490adcee97f57e51cab8794e63038b70950d715a90e7e524397007a97f875ebf`.
  The post-`0002` structural fingerprint produced by the bundled SQLite engine is
  `391d9bfb54cf771de1815a3bf54ee4d7d16f1b877acf629cf783ca12dbd37d4d`.

### Journal identity, allocation, and envelope

- `event_id` is a canonical UUIDv7 event identity. `journal_offset` is the positive global
  `AUTOINCREMENT` cursor. `stream_seq` is the positive contiguous position within one stream.
  Global cursor gaps are allowed; code never validates global contiguity.
- Stream identities are exactly `craxii:<uuidv7>`, `conversation:<uuidv7>`, `work:<uuidv7>`, and
  `runtime:<uuidv7>`, with lowercase prefix, one colon, and canonical lowercase UUIDv7.
- `stream_heads` is allocated inside the caller's existing `BEGIN IMMEDIATE` transaction by
  `INSERT ... ON CONFLICT ... DO UPDATE ... RETURNING`. The first value is one, subsequent values
  increment exactly once, and overflow fails. The allocator neither opens nor commits a
  transaction and never uses `MAX + 1`.
- The envelope fields are the architecture's exact V0 set: global/event/stream identities and
  sequence; type/version; Craxii, optional conversation/Work/runtime links; optional causation;
  correlation; typed actor kind/ID; payload text/digest; and recorded/optional occurred time.
- Actors are exactly `user`, `craxii`, `model`, `tool`, `runtime`, and `client`. Actor IDs are typed
  entity IDs where possible. Causation must point to a previously observed lower-offset event and
  cannot self-link. Correlation continuity is event-specific, not a blanket per-stream rule.

### Payloads, versions, and registry

- Adapter-private V1 DTOs deny unknown object fields. The envelope carries type and version; the
  payload does not repeat a version. Trusted domain events contain typed payloads, never raw JSON.
- The stored digest is SHA-256 over the exact stored UTF-8 `payload_json` bytes. Load recomputes and
  compares before decode and never verifies by reserialization. Payloads are objects bounded to
  256 KiB.
- Work-lifecycle V1 facts contain from/to state plus expected and resulting projection version,
  runtime owner, current attempt, cancellation classification, terminal classification, and the
  transition timestamp. Replay must match the expected facts before applying the Stage 4 rule;
  stream sequence is not projection version.
- Event type meaning is stable. Incompatible required fields, meanings, units, or enum changes need
  a new version. All initial kinds support exactly version one. Unknown kinds and unsupported
  required versions fail closed. Evidence-only skipping requires an explicit future architecture
  declaration and produces a structured warning; no current unknown is skipped.
- The registry is exactly: `craxii.initialized`, `conversation.created`, `message.accepted`,
  `work.queued`, `work.started`, `work.waiting_on_model`, `work.waiting_on_tool`, `work.resumed`,
  `work.cancel_requested`, `work.cancelled`, `work.completed`, `work.failed`, `work.interrupted`,
  `model.invocation_started`, `model.invocation_completed`, `model.invocation_failed`,
  `model.invocation_interrupted`, `tool.execution_requested`, `tool.execution_dispatching`,
  `tool.execution_completed`, `tool.execution_outcome_unknown`, `assistant.message_committed`,
  `artifact.recorded`, `runtime.started`, `runtime.recovery_performed`, and `runtime.stopping`.
- Each registry entry freezes version, state-bearing classification, primary stream family,
  first-emitting stage, and an internal public-candidate classification. Stage 7 emits only
  `craxii.initialized` and `conversation.created`; this metadata is not a Stage 11 wire contract.

### Inputs, ordering, and projection

- `work_item_inputs` has exactly `work_id`, `input_event_id`, `relationship`,
  `ordinal_within_work`, `attached_at`, and `attached_by_actor`; its primary key is
  `(work_id, input_event_id)` and its named unique access path is `(work_id,
  ordinal_within_work)`. It has no `message_id`.
- Relationship literals are `trigger`, `steering`, `supplemental`, `scheduled_trigger`,
  `external_trigger`, and `recovery_instruction`; actor literals are `user`, `craxii`, `system`,
  and `recovery`.
- A V0 conversational Work has exactly one user `trigger` at ordinal one, pointing to its
  same-conversation `message.accepted` cause with the Work correlation. Later messages are not
  implicit inputs. Conversation message order uses only conversation `stream_seq` for
  `message.accepted` and `assistant.message_committed`, never timestamps, UUIDs, or cross-stream
  offsets.
- The pure projector is `application/projector.rs`. It has no SQLx, I/O, clock, randomness, global
  state, or correctness-critical logging. It reuses Stage 4's legal Work transition matrix and
  reconstructs root/conversation/message/Work state plus typed model/tool/artifact/runtime
  references without creating absent Stage 8 rows.
- Replay rejects duplicate IDs, non-increasing global offsets, per-stream gaps, missing/forward
  causation, required correlation mismatch, stream/link/actor mismatch, stale or illegal Work
  transitions, terminal resurrection, owner/current-attempt contradictions, and duplicate roots.
  Global offset gaps are accepted.

### Bootstrap, authority, and ports

- First bootstrap pre-generates principal/workstation/workspace/conversation IDs, two event IDs,
  and one correlation ID outside the transaction. One coordinated `BEGIN IMMEDIATE` transaction
  inserts the four root rows, sets both principal roots, then appends `craxii.initialized` followed
  by caused `conversation.created`. Both use the Craxii actor and one correlation. There is no
  runtime, device, Work, message, command, or input row.
- Defaults are `Craxii`, `local-owner`, active, `V0.0.01`, schema revision two, local workstation,
  and provider `unclassified` with provider facts null. The primary conversation starts active at
  ordinal/version one. Initialization stores the complete stable root snapshot and capability
  digest; conversation creation stores the full initial conversation projection.
- The observation uses validated configuration, runtime architecture/OS-family, configured shell,
  logical workspace, capability bounds, and administrative flag. Stage 7 records no execution
  capabilities, cloud metadata, hostname, PID, or state-root-derived identity. The portable
  `std::env::consts::OS` family string is the narrow honest value available without introducing
  Stage 12 workstation inspection. Reopen leaves `last_seen_at` unchanged.
- Complete reopen loads the same IDs and appends nothing. Any partial/duplicate/mismatched
  root-event-head topology, configuration/workspace/capability contradiction, or revision mismatch
  fails closed. Bootstrap never repairs.
- Journal evidence is authoritative for transition/order, immutable rows for canonical detailed
  evidence bytes, and mutable rows for current truth. A bounded deferred read replays, validates
  heads/causation/codecs, and compares semantic state. Contradiction fails startup; no side is
  rewritten.
- State persistence is split into `BootstrapStateStore`, `CommandStateStore`,
  `SchedulerStateStore`, `ModelStateStore`, `ToolStateStore`, `CompletionStateStore`,
  `ReplayStateStore`, and `RecoveryStateStore`. The SQLite facade implements only the Stage 7
  bootstrap capability. No port exposes SQLx, generic CRUD, generic transactions, callbacks, raw
  append, or unsupported placeholders.
- Append and Work-input insertion are adapter-private and require an existing write transaction.
  Allocation, row insertion, current projection mutation, and returned commit receipt belong to
  one owning transaction. There is no production journal/input update or delete helper.

### Deferred ownership

- Stage 8 owns attempt/context/artifact/authority tables and model/tool/completion behavior.
- Stage 9 owns device authentication, request hashes, command idempotency, and the message/Work
  transaction.
- Stage 10 owns runtime rows, heartbeat, scheduler, cancellation execution, recovery, and readiness.
- Stage 11 owns HTTP/WebSocket payloads, public replay projection, and the snapshot/replay race.

## Rationale

Separate global and per-stream positions preserve both total replay order and aggregate-local
causality. Exact-byte payload hashing detects stored-byte changes before interpretation. A pure
projector and fail-closed semantic comparison make the journal useful without pretending all
detailed evidence is event sourced. Runtime bootstrap provides stable local identity while a
capability-split port avoids dishonest future method stubs.

## Consequences / tradeoffs

- `AUTOINCREMENT` costs a small metadata write and permits gaps, but gives a never-reused cursor.
- No triggers means privileged direct SQL can mutate history; containment, invariants, review, and
  permanent tests are the V0 enforcement boundary.
- Requiring observation agreement may reject startup after a meaningful machine/configuration
  change. That is preferable to silently changing identity evidence; a later explicit migration or
  workstation-generation workflow is the change path.
- Full startup replay is intentionally uncheckpointed at V0 scale. A checkpoint is a later durable
  contract, not an optimization smuggled into Stage 7.

## Rollback / change path

Migration `0002` is immutable after release. Changes require an architecture amendment, a new
forward SQLx migration, registry/version update where applicable, regenerated structural
fingerprint, golden codec updates, and compatibility/replay tests. Public projection, checkpointing,
repair, new event versions, new stream families, actors, relationships, or bootstrap topology are
durable changes and cannot be introduced by adapter-only edits.
