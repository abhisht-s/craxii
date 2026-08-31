# Stage 16: Causal Context Assembler and exact manifest construction

## Status and sources

Accepted for Craxii V0.0.01 local implementation on 2026-08-31. The normative source remains
[`craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md), and the dependency order remains
[`craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md). This record freezes
Stage 16 only. Stage 17 model invocation, retry, agent-loop, tool-loop, completion, scheduler, and
provider orchestration remain absent.

## Persistence, migration, and ownership

Stage 16 adds no migration. Migrations remain exactly `0001`–`0003` and the schema ceiling remains
3. No new dependency is required. A successful assembly prepares an immutable
`PreparedContextManifest` and its ordered sources in memory. It does not persist them by itself.
Stage 17 owns the first-attempt transaction that atomically persists the manifest, source rows,
model invocation intent, Work transition, and journal events. A context-limit failure creates no
manifest row, no model invocation row, and no provider call; Stage 17 later owns the atomic
`work.failed` transition.

## Context Assembler responsibility and read boundary

`ContextAssembler` receives a `WorkId`, immutable `ModelSelectionResult`, explicit versions, a
narrow read-only `ContextSourceStore`, the immutable V0 tool registry and instruction snapshot,
the selected target's exact `TokenEstimator`, and a clock used only for nonsemantic `created_at`.
It does not receive a selector, mutable catalog, fallback list, mutating State Store, Workstation,
Tool Execution Service, provider, provider gateway, or provider wire DTO.

`ContextSourceStore` loads one Work's eligibility facts under one bounded SQLite read transaction.
Every query uses that transaction; SQLx remains inside the adapter. Durable Workstation capability
and logical workspace rows are sources. Assembly performs no live Workstation query.

## Causal frontier and isolation

For active Work ordinal `N`, prior eligibility is exactly `conversation_work_ordinal < N`. The
active trigger is loaded separately through the exact `work_item_inputs` `trigger` relationship at
ordinal one and its referenced accepted-message event. Queries never use `<= N`, latest message,
latest timestamp, `created_at`, or journal offset as eligibility authority. Every history query
binds the active `conversation_id` and proves Work ownership.

A later accepted Work/message at ordinal `N+1` never enters Work `N`, even if it commits before
later evidence for `N`. When `N+1` assembles it may observe a durable completed result of prior Work
`N` even when `N+1` was accepted first. The current trigger is rendered exactly once; duplicate
logical source identities fail closed. Every accepted prior user message remains eligible after
its Work fails, is interrupted, or is cancelled unless that message itself is invalid or deleted.

## Source eligibility and canonical rendering

Completed committed normalized output is eligible: ordered text, complete tool calls, structured
data, refusal, provider-exposed reasoning summary, and compatible provider continuation. Partial
or draft content, incomplete arguments, uncommitted output, provider-outcome-unknown content,
client drafts/caches, and unknown correctness-bearing output are not ordinary history.

For prior Work with a committed final assistant Message, that Message is the authoritative final
conversational output and is rendered once. Terminal model text for the same final answer is
excluded. Intermediate completed model output that produced tool calls remains eligible in its
original item order.

An ordinary tool result requires definite observed Stage 14 evidence paired uniquely by Work,
conversation, source model invocation, agent step, tool ordinal, provider call ID, and tool name.
Orphan, duplicate, mismatched, cross-Work, and cross-conversation results fail closed. A durable
`outcome_unknown` record becomes a synthetic uncertainty item: execution may have occurred, the
outcome is unknown, and repetition must not be assumed safe. It is never an ordinary ToolResult and
implies no retry. Interrupted model attempts contribute no partial output. A cancelled prior Work
gets no invented cancellation item; its accepted input and durable observed outputs remain.

The V0 instruction snapshot has explicit template version, ordered system and developer blocks,
canonical bytes, and SHA-256 fingerprint. It contains no current time, secret, credential, or
provider wire syntax. Historical reasoning summary is a distinct provider-neutral item containing
only provider-exposed summary; it is never ordinary assistant text or hidden/private reasoning.

The Stage 14 tool registry projects exactly `read_file`, then `run_shell`, with name,
implementation/schema version, description, and canonical input schema, never handler identity.
Assembly recomputes the semantic projection fingerprint and requires it to equal the registry
fingerprint.

Canonical order is: system instructions, developer instructions, durable Workstation capability
summary, logical workspace identity, tool definitions, prior Work ordinal ascending, active
trigger, then active Work completed trace. Within Work, agent step, original model output position,
tool ordinal, journal offset tie-breaker, and stable durable ID define order. A paired observed tool
result immediately follows its call. Wall-clock timestamps and `HashMap` iteration never order
sources.

## Provider opaque continuation

Opaque bytes are never interpreted. Continuation is rendered only when its durable source exists;
provider ID, provider-model ID, and target configuration version equal the selection; selected
capability and typed native option permit it; it originates from the immediately preceding
completed logical invocation in the same permitted Work/agent-step chain; it is not the sole
semantic history; and it has crossed no interrupted or `provider_outcome_unknown` model-attempt
barrier or `outcome_unknown` tool-execution barrier. The eligibility snapshot carries completed,
interrupted, and provider-unknown model boundaries plus definite and unknown tool boundaries in
durable causal order; completed output rows alone are insufficient.
Otherwise it is excluded from continuation rendering while durable evidence remains preserved.

## Tool projections and artifacts

Normal context uses the bounded Stage 14 model-visible projection and artifact descriptors/hashes:
call/tool identity, result kind, safe summary/fields, stream/file projections,
observed/captured/returned/omitted counts, artifact IDs/hashes, projection flag, and definite
failure details. It excludes storage paths, raw traces, PIDs/PGIDs, unbounded bytes, and secrets.
Arbitrary full 8 MiB artifacts are not inlined. Artifact bytes are read only for explicitly
required content or exact verification/reconstruction through the verified artifact API. Missing,
corrupt, or partial content fails closed.

## Selection, output, estimation, and limits

Selection occurs before rendering. Target identity, full capabilities, limits, configuration
version, typed provider-native options, and estimator identity/version are immutable inputs. The
native options pass unchanged. Requested output and `reserved_output_tokens` are exactly the
selected target's configured `requested_output_tokens`; Stage 16 has no additive safety reserve and
never lowers this limit.

The selected estimator's returned identity/version must exactly match configuration. There is no
fallback or dynamic alternate. Complete deterministic units cover instruction/text framing,
structured data, tool definitions, tool calls/results, opaque continuation, and conservative
provider-native overhead. Eligibility never depends on the estimate.

Fit is checked arithmetic over
`estimated_input_tokens + requested_output_tokens <= context_window_tokens`. Equality passes; one
token over returns `context_limit_exceeded`. Compact canonical provider-neutral `ModelRequest`
bytes must be at most 16,777,216. Stage 16 fully constructs and canonically serializes the request,
measures actual UTF-8 bytes, and applies the ceiling before estimator identity validation or
estimator invocation. Exactly 16 MiB passes; one byte over returns `context_limit_exceeded` with
zero estimator calls even if the estimator would mismatch, fail, or overflow.

All causally eligible V0 history is mandatory. There is no windowing, pruning, retrieval,
summarization, compression, clipping, tool/instruction removal, output-reserve reduction, or target
fallback to fit. `context_limit_exceeded` is a definite `ContextError`, retryability `never`, with
safe evidence: estimate, reserve, window, request bytes/limit, target, estimator ID/version, source
count, toolset fingerprint, and prompt fingerprint. Evidence/logging contains no user content.

## Package, sources, cutoffs, and hashes

Assembly generates UUIDv7 `ContextManifestId` and `LogicalInvocationId`. Immutable
`ContextPackage` binds them to target snapshot, ordered sources/items/instructions/tools, native
options, tool choice, requested output, estimator, byte contributions, and hashes. Retry reuses it
unchanged. A new step after newly durable tool evidence creates a new assembly and identities.

Every prepared source has a contiguous one-based position, exact kind, exactly one durable identity
family, role/item class, original semantic source SHA-256, rendered byte contribution, and explicit
transform. Identities cover instruction version, Workstation, workspace, tool definition,
MessageId, model invocation plus item position, ToolExecutionId, ArtifactId, Work synthetic status,
and continuation. Anonymous sources are forbidden.

The cutoff contains conversation ID, active Work ordinal, highest prior terminal ordinal, exact
input event IDs, active output IDs, and maximum journal offset observed in the read snapshot; the
manifest already contains WorkId. Source hashes cover canonical durable semantics before transform:
message content, normalized model output, tool evidence, finalized artifact digest, or versioned
instruction/tool/synthetic envelope. A projection does not replace its source hash.

The authoritative request hash is SHA-256 of the final compact canonical `ModelRequest`, after
generated IDs, target, instructions, ordered input, exact tools, tool choice, fixed
`parallel=false`, requested output, and native options are present. It has no timestamp.

The manifest hash covers a canonical semantic envelope plus ordered sources, including immutable
generated IDs, target/provider/model/config and full capabilities, prompt/tool fingerprints and
versions, assembler/policy versions, cutoff, estimator ID/version, byte/token/limit/utilization,
transformations/omissions, and request hash. It excludes its own hash, `created_at`, assembly
latency, storage paths, telemetry, and nonsemantic SQLite details. Reconstruction reloads only the
exact durable identities named by the prepared/stored source rows in manifest position order,
verifies ownership, type, hashes, target/config/prompt/toolset/estimator identities, rebuilds using
committed IDs, and requires exact request bytes, request hash, and manifest hash. It never starts
from current Work eligibility, substitutes a newer equivalent source, or re-sorts current database
state. An old retry package remains exactly reconstructable after later eligible model/tool evidence
or later Work acceptance changes a fresh assembly. Missing/corrupt sources or drift fail closed.

## Structural checker boundary

The Stage 16 checker freezes strict prior-history cutoff shape and exact active-ordinal binding and
traverses assembler-reachable helpers with a bounded, alias-aware provenance analysis. The single
final-request constructor conserves the complete frozen canonical input, complete stable Stage 14
tool projection, and exact selected configured requested-output limit; canonical serialization, the
byte gate, estimation, hashing, packaging, and return all use that final request and its unchanged
sources. The checker follows the `OutcomeUnknown` return arm through helpers and conservative macro
boundaries so it can reach only synthetic uncertainty, never ordinary `ToolResult`; an unused
synthetic marker cannot satisfy the invariant. Compilation-gated mutations cover broader, removed,
offset, and application-filtered ordinal predicates; direct/helper/alias reselection; input/byte/
tool/output shortening; and direct/helper/alias/wrapper unknown-result conversion. Positive controls
retain immutable selected-target/input/tool inspection, exact full-value moves, bounded Stage 14
projections, definite tool results, synthetic unknown helper chains, complete-request byte
inspection, and diagnostics-only sampling.

## Later-stage boundary and deferred verification

No Model Gateway, provider call/network I/O, invocation persistence orchestration, retry loop,
Agent Loop, tool loop, production Work Runner, scheduler activation, readiness promotion, OpenAI
adapter, Reqwest, SSE, provider credential, public model endpoint, or client draft is introduced.
Production remains `live_unready`.

`STAGE_13_UBUNTU_TARGET_VERIFICATION: DEFERRED_BY_USER_TO_LATER_STAGE`
