# Stage 8 evidence and attempt persistence contract

## Status and scope

Accepted for Craxii V0.0.01 Stage 8 on 2026-08-28. This record freezes migration `0003`, the five
new evidence tables, attempt persistence primitives, and the local content-addressed artifact
adapter. It does not authorize Stage 9 command behavior, Stage 10 runtime/recovery behavior, Stage
14 tool execution, Stage 15 provider behavior, Stage 16 context assembly, or Stage 17 agent-loop and
assistant completion behavior.

## Migration identity

- File: `backend/migrations/0003_context_model_tool_artifacts.sql`.
- SQLx version: `3`.
- SQLx description: `context model tool artifacts`.
- SQLx SHA-384 checksum:
  `e2f5cab2ac0921ce54e6ae8a741eb23c11766e847a5d17f21f7381ae4aa1d729287542c1ebaa12d25431f3b277cd5c39`.
- V3 structural schema fingerprint:
  `73ab94c2ec36ef1b09addc475aa6bcf806336612f58fd551fd4648c5a124f5a3`.
- Frozen V1 fingerprint remains
  `f4636df22c635c90ac469f49f2ac3a9ccb38956f1670d26ab566140a137f5521`.
- Frozen V2 fingerprint remains
  `391d9bfb54cf771de1815a3bf54ee4d7d16f1b877acf629cf783ca12dbd37d4d`.
- `PRAGMA user_version` remains zero. SQLx metadata is the only migration-version authority.

Migration `0003` creates exactly `context_manifests`, `context_manifest_sources`,
`model_invocations`, `tool_executions`, and `artifacts`. All use `STRICT, WITHOUT ROWID`. The
migration creates no rows, directories, artifact bytes, custom version table, down migration,
trigger, view, provider-conversation table, chunk table, retention/deletion table, or Stage
9-or-later object.

## Exact table, index, and foreign-key inventory

The exact Stage 8 named indexes are:

1. `ux_context_manifests_logical_invocation`
2. `ix_context_manifests_work_created`
3. `ix_context_manifest_sources_event`
4. `ix_context_manifest_sources_artifact`
5. `ux_model_invocations_logical_attempt`
6. `ux_model_invocations_work_step_attempt`
7. `ux_model_invocations_retry_of`
8. `ux_model_invocations_one_nonterminal_per_work`
9. `ix_model_invocations_runtime_nonterminal`
10. `ix_model_invocations_context_attempt`
11. `ux_tool_executions_execution_id`
12. `ux_tool_executions_work_step_ordinal`
13. `ux_tool_executions_source_ordinal`
14. `ux_tool_executions_source_provider_call`
15. `ux_tool_executions_one_nonterminal_per_work`
16. `ix_tool_executions_runtime_nonterminal`
17. `ix_artifacts_storage_key`
18. `ix_artifacts_content`
19. `ix_artifacts_producing_work`
20. `ix_artifacts_producer_kind_id`

Foreign keys use `ON UPDATE RESTRICT ON DELETE RESTRICT`. `context_manifests` references Work and an
optional rendered-request Artifact. `context_manifest_sources` references its Manifest and optional
Journal Event or Artifact. `model_invocations` references Work, Runtime, Manifest, optional retry
predecessor, and optional request/response Artifacts. `tool_executions` references Work, source
Model Invocation, Runtime, Workstation, Workspace, and optional stdout/stderr Artifacts. `artifacts`
references Craxii and optional producing Work only.

`work_items.current_model_invocation_id` and `current_tool_execution_id` deliberately retain no
physical reverse foreign key. Named transactions maintain them and startup fails closed on any
missing, wrong-Work, wrong-runtime, terminal, non-XOR, or state-incoherent link.

## Context manifest and source representation

The manifest stores the complete neutral target tuple: model target, provider, provider model,
positive target-configuration version, and versioned capability snapshot. It also stores assembler
and policy versions, prompt/toolset fingerprints, a versioned eligibility cutoff, exact source and
byte/token counts, token estimator, context/output limits, deterministic ceiling utilization basis
points, manifest/rendered-request hashes, optional rendered-request Artifact ID, versioned
omissions, and creation time. All JSON fields are bounded objects decoded by private DTOs that deny
unknown fields. A context-limit failure creates no manifest.

Sources have a positive stored position and exactly one identity family: `event_id`, `artifact_id`,
or the pair `source_record_kind`/`source_record_id`. The closed source kinds are
`system_instruction`, `developer_instruction`, `workstation_capability_summary`,
`workspace_identity`, `tool_definition`, `user_message`, `active_trigger`, `assistant_message`,
`completed_model_output`, `observed_tool_result`, `artifact_content`, `synthetic_failure`,
`synthetic_interruption`, `synthetic_outcome_unknown`, `synthetic_draft_status`, and
`provider_native_continuation`. Typed record kinds are `instruction_version`, `workstation`,
`workspace`, `tool_definition`, `message`, `model_invocation`, `tool_execution`, and `work`.
Private decode validates the kind-specific identifier, source hash, optional role/item class,
rendered contribution, transformation object, positions, and parent source-count reconciliation.
Stage 16, not Stage 8, chooses or renders sources.

## Model identity, lifecycle, and retry contract

Each row stores one canonical Model Invocation ID, logical invocation, Work, Runtime, Manifest,
positive agent step and attempt, optional predecessor, exact target/provider/model/config/capability
snapshot, closed selection reason, versioned required-capability and provider-neutral option
snapshots, request hash/optional Artifact, state, optional response hash/Artifact, versioned ordered
provider-neutral output envelope, safe provider identifiers, timestamps, usage, stop/tool counts,
draft exposure, and a safe normalized error.

States are exactly `requesting`, `streaming`, `completed`, `failed`, `cancelled_locally`, and
`provider_outcome_unknown`. SQL state-shape checks reject terminal fields on nonterminal rows,
require terminal time and the appropriate output/error evidence on terminal rows, and preserve
boolean/count/timestamp coherence. Private codecs reconstruct Stage 4 state and fail closed on an
unknown literal or contradictory row. Terminal rows have no generic update API.

Attempt one has no predecessor; every later attempt names one predecessor. Uniqueness prevents a
logical attempt duplicate, Work/step/attempt duplicate, retry branch, and a second nonterminal model
attempt for one Work. Application/startup validation additionally requires a terminal predecessor,
same logical invocation, Work, Manifest, and step, predecessor attempt exactly `N-1`, and a
contiguous chain.

## Tool identity, lifecycle, and count contract

Each row stores one Tool Execution ID and unique stable Execution ID, Work/source Model/Runtime,
positive step and ordinal, optional provider call ID, bounded tool/version/schema identity,
canonical argument JSON and hash, Workstation generation, Workspace, requested/resolved cwd,
requested/effective privilege, optional strict authority snapshot, bounded timeout/output policy,
exact state/timestamps, terminal process/result/cleanup fields, optional stdout/stderr Artifacts,
the four counts for each stream, truncation, and safe normalized error.

`requested_cwd` is nonnullable and always contains the concrete effective requested
`LogicalPathReference`. A caller that omits an explicit cwd resolves the Work/workspace default
before Stage 8 persistence. Stage 8 does not preserve absence for Stage 14 to reinterpret;
`resolved_cwd` alone remains null until dispatch intent.

States are exactly `requested`, `dispatching`, `completed`, `interrupted_before_dispatch`, and
`outcome_unknown`. The legal transitions remain `requested -> dispatching|completed|
interrupted_before_dispatch` and `dispatching -> completed|outcome_unknown`. Requested state needs
no authority. Dispatching requires a complete allow snapshot, effective privilege, resolved cwd,
and dispatch time. Interrupted-before-dispatch proves no dispatch fields or side effect. Outcome
unknown is reachable only from dispatching, requires cleanup unconfirmed and normalized-error
certainty `outcome_unknown`, interrupts Work, and has no retry.

For stdout and stderr independently, `observed >= captured >= returned_inline`, and `omitted =
observed - returned_inline`. All four counts for one stream are absent or present together.
`truncated` is coherent with at least one observed count exceeding captured count. No process bytes
are stored directly in SQLite.

## Journal additions and ordering

The registry grows from 26 to exactly 28 kinds by adding only:

- `model.invocation_streaming`, Work stream, version 1, state-bearing, Stage 8 owner.
- `tool.execution_interrupted_before_dispatch`, Work stream, version 1, state-bearing, Stage 8
  owner.

Both use provider/process-neutral typed payloads and contain no wire body or raw output. Model begin
orders any `artifact.recorded` events before `model.invocation_started`, then appends caused
`work.waiting_on_model`. Streaming appends its new event. Model terminal orders artifact events,
the exact terminal model event, then a caused legal Work event if supplied. Tool request appends
`tool.execution_requested` then caused `work.waiting_on_tool`; dispatch appends
`tool.execution_dispatching`; terminal orders artifact events, the exact completed,
interrupted-before-dispatch, or outcome-unknown event, then a caused legal Work event. There is no
context-manifest event.

One semantic Artifact ID receives exactly one `artifact.recorded` event when its metadata commits.
Shared physical bytes still produce separate rows/events because provenance belongs to the semantic
occurrence.

## Safe JSON and evidence encodings

All Stage 8 JSON is canonical bounded UTF-8 object text behind adapter-private versioned DTOs with
unknown fields denied. Decoders are symmetric with encoders and reconstruct canonical IDs and
existing checked domain types; persisted strings, nested JSON, literals, and counts are not trusted
merely because deserialization succeeded. Model capabilities enforce positive context windows and
output limits no greater than the window. Eligibility cutoffs validate canonical Conversation and
Journal Event IDs, reject duplicate inputs, and preserve their ordinal/offset relations. Provider
options apply the same sorted unique key grammar and value bounds in both directions. Normalized
model output validates every ordered nested item, tool and provider identity, canonical argument
JSON, Artifact ID, count, and closed kind. Tool results use closed result kinds with per-kind
required/forbidden process fields. Model capability and required-capability envelopes use the
existing neutral capability vocabulary; no provider wire structs enter storage.

Authority JSON contains only version, `allow|deny`, the existing exact privilege literal, policy
`v0-development-workstation`, and exactly `registered_tool`, `policy_denied`,
`malformed_request`, `limit_exceeded`, `work_cancelled`, or `scope_denied`; a denial cannot claim
effective `admin`. Normalized error JSON reuses the safe persisted envelope and excludes
`InternalDetail`, raw SQLx/parser/provider/process text, credentials, paths, headers, commands, and
output. Certainty `outcome_unknown` is accepted only for the corresponding model/tool unknown state.
A definite completed-before-dispatch authority rejection may retain a Deny snapshot while all
dispatch, resolved-path, process, and output fields remain absent. Interruption before dispatch
retains no authority snapshot.

## Artifact identity and storage contract

`ArtifactId` is semantic identity; `sha256/<two lowercase hex>/<64 lowercase hex>` is the physical
storage key. Multiple semantic rows may share one key and one canonical file, so key/content
uniqueness is forbidden. `producer_kind`/`producer_id` preserves a typed model/tool producer without
a cyclic foreign key; startup validates producer existence, type, Work, and referenced hash.

The local layout is `<artifact_root>/tmp/<artifact-id>.partial` and
`<artifact_root>/sha256/<two-hex>/<digest>`. Directories are 0700, files 0600, names are generated or
digest-derived, and logical names/MIME values never affect paths. Root/tmp/sha256/shards/finals must
be local accepted filesystem objects, non-symlink, safely owned/mode-hardened, same-device for temp
and final, and regular with link count one where applicable. Opens use create-new, no-follow, and
close-on-exec semantics through the existing `nix`/`libc` surface.

Capture is always bounded. It counts every observed byte, hashes/writes only the captured prefix,
continues draining caller-supplied chunks after the limit, and marks truncation without pretending
observed equals captured. Finalization performs exclusive temp creation, streaming/hash/count,
flush, file sync, shard creation/verification and directory sync, platform no-replace publication,
post-publish directory sync, full reopened object verification, then returns a finalized descriptor.
Linux uses `renameat2(RENAME_NOREPLACE)`; macOS uses `renamex_np(RENAME_EXCL)`. An existing target is
fully verified and reused only if exact; it is never overwritten. Concurrent equal bytes converge
on one physical object while retaining distinct semantic Artifact IDs.

`FinalizedArtifact` is a sealed durable-publication capability with private fields and no public
constructor. The local adapter alone mints it after successful durable publication and
verification. SQLite reconstructs only a distinct read-verification reference from persisted
metadata and cannot manufacture this capability.

Verified read maps only a canonical storage key beneath the configured root, opens no-follow,
checks regular type/mode/link count/size, computes full SHA-256, and returns bytes only after all
checks pass. No public port value exposes an absolute path.

## ArtifactStore and StateStore boundaries

`ArtifactStore` is dependency-neutral and owns bounded begin/write/finalize, verify, verified read,
and orphan scan operations. The local adapter lives outside SQLite. A finalized descriptor contains
all metadata needed by a later transaction but no physical path. No database reference may exist
before finalization returns.

SQLite exposes no generic artifact insert. Its adapter-private
`insert_artifact_metadata(transaction, finalized_metadata)` helper does not commit. Named model/tool
transactions compose metadata insertion, `artifact.recorded`, detail mutation, Work mutation/event,
and one commit. Stage 8 StateStore methods are `begin_model_invocation`, `mark_model_streaming`,
`finish_model_invocation`, `request_tool_execution`, `commit_tool_dispatch_intent`, and
`finish_tool_execution`. Inputs contain precomputed decisions and finalized descriptors. They do no
provider I/O, authority evaluation, registry lookup, process/filesystem side effect, context
assembly, or agent-loop work. `CompletionStateStore` belongs to Stage 17.

Before any artifact row or event is inserted, each owning transaction derives the distinct set of
Artifact IDs referenced by its model, tool, manifest, and context-source facts and requires exact
equality with the supplied finalized descriptors. Missing, extra, and duplicate descriptors are
rejected. Incompatible semantic reuse is rejected; the one explicit model-begin reuse is an
identical attempt-one invocation request and rendered manifest artifact with equal ID, digest,
count, producer, and Work. Every descriptor is checked against referenced ID, digest,
captured/observed counts, truncation, producer identity, and producing Work. Existing
context-source artifacts must already be durable same-Work metadata with the same digest. Only the
fully validated set reaches metadata/event insertion, owning-row and Work/journal mutation, and one
commit.

## Startup integrity and orphan policy

Stage 8 startup orders configuration; SQLite lock/migration V3/integrity; artifact-root hardening;
Stage 7 identity bootstrap/load; journal/application comparison; relational attempt checks;
referenced-artifact verification; orphan scan/report; bootstrap snapshot; remain `live_unready`.
Every distinct referenced key must exist and agree across all metadata rows on key derivation,
digest, captured size, safe mode/type/link count, and producer/reference facts. Missing, corrupt, or
unsafe referenced evidence is fatal and never repaired.

Temp partials and valid final canonical files with no metadata reference are expected nonfatal
residue. Ordinary startup counts/reports and never deletes them. The pure maintenance eligibility
rule is: final canonical, currently unreferenced, age at least 24 hours, and still unreferenced on an
immediate database recheck. Stage 8 implements no scheduled or automatic deletion.

## Deferred ownership boundaries

- Stage 9: authentication, device tokens, message/cancel commands, command idempotency.
- Stage 10: runtime row lifecycle, scheduler, recovery execution, readiness true.
- Stage 14: tool registry resolution, authority evaluation, Workstation/process/filesystem action.
- Stage 15: provider port/adapters, canonical provider semantics, pricing/cost.
- Stage 16: context eligibility queries, ordering, rendering, estimation, and budgeting.
- Stage 17: provider calls, model gateway/agent loop, and terminal assistant completion.

## Tradeoffs and change path

The typed producer reference sacrifices immediate SQLite enforcement of the artifact-to-attempt
reverse edge to avoid a fragile cyclic insertion graph. The forward attempt-to-artifact edge remains
physical, and named transaction plus startup checks make the reverse edge explicit and testable.
Per-semantic metadata duplicates small rows while preserving provenance and allowing safe physical
deduplication. Full digest verification costs startup/read latency but makes referenced evidence
fail closed. A later external backend may retain Artifact IDs, hashes, counts, and opaque storage
keys while replacing only `ArtifactStore`; a later retention stage may add explicit maintenance
records only through a new architecture decision and forward migration.
