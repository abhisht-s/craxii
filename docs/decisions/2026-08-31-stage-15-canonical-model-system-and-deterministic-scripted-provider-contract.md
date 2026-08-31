# Stage 15 canonical model system and deterministic scripted provider contract

## Status and sources

Accepted for Craxii V0.0.01 local implementation on 2026-08-31. The normative source remains
[`craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md), and the dependency order remains
[`craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md). This record freezes
the narrow Stage 15 contracts without starting context assembly, model persistence orchestration,
the agent loop, a real provider transport, or client draft streaming.

## Persistence and dependency boundary

Stage 15 uses migrations `0001`–`0003` and schema ceiling `3`. It adds no migration and changes no
V1, V2, V3, journal-kind, or Stage 11 public-protocol contract. Existing model-invocation tables and
State Store primitives are not called by Stage 15 code. No new crate is added. Provider modules do
not import SQLx, State Store, journal, Tool Execution Service, or Workstation types.

The existing global runtime configuration fingerprint is the authoritative semantic fingerprint
for the configured model catalog. No model-catalog fingerprint is introduced. Changes to target
identity, capability, limit, estimator, endpoint/account reference, enabled state, native option,
or default selection change that global fingerprint through the existing config codec.

## Configured target identity and immutable snapshot

Canonical target identity uses the existing `ModelTargetId`, `ProviderId`, `ProviderModelId`, and
positive `TargetConfigurationVersion`. No redundant `ModelId` is introduced. A configured
`ModelTarget` contains that neutral `ProviderModelReference`, enabled state, safe nonsecret
endpoint/account configuration references, requested output tokens, estimator ID/version, and the
closed validated provider-native option set already represented by config. The account reference
is a credential handle name, never credential bytes.

The capability snapshot is exactly:

- `text_input`;
- `text_output`;
- `custom_tool_calling`;
- `streaming`;
- `ordered_output_items`;
- `structured_output`;
- `reasoning_continuation`;
- positive signed-64-bit-safe `context_window_tokens`;
- positive signed-64-bit-safe `max_output_tokens`.

Startup derives one immutable snapshot from validated configuration. Targets are ordered by
ascending `ModelTargetId`; duplicate targets fail construction. Constructor-local mutable vectors
and validation maps end at publication into the snapshot's private `Box<[ModelTarget]>`. The
published snapshot exposes only reads: it has no mutable receiver, mutable storage reference,
replacement collection, setter, lock/cell wrapper, or helper path capable of whole-catalog
replacement. It also has no insert, remove, update, discovery, alias, availability, pricing, or
provider-registration operation. Production target inventory remains configuration-driven;
fixture targets are not live model declarations.

## Required capabilities and deterministic selection

The provider-neutral requirement snapshot can require each boolean capability plus a positive
output-token budget. It does not render history or calculate Stage 16 context size.

`ModelSelectionPolicy` selects exactly an explicitly requested target when present. That target
must exist, be enabled, and satisfy every requirement. Without an explicit target, the policy
selects exactly the configured default, which must likewise exist, be enabled, and satisfy every
requirement. Every failure returns a normalized `model_selection_error`. There is no fallback,
tool removal, capability downgrade, availability probe, cost/latency ranking, alias resolution, or
provider call.

The explicit and configured-default branches pass their exact `ModelTargetId` into one shared
exact-ID lookup/validation helper. Only that lookup may provide the successful selected target;
slice indexes, first/last values, inventory iteration, candidate helpers, and recovery closures are
not valid target provenance. Exact lookup, enabled-state, and capability failures propagate as the
branch-specific error rather than selecting another target.

The result contains the complete selected immutable target, reason `explicit` or
`configured_default`, all considered target IDs in snapshot order, the required-capability
snapshot, and target configuration version. The pure Stage 15 result contains no timestamp;
Stage 17 invocation persistence owns observed lifecycle timestamps.

## Canonical request and input items

`ModelRequest` contains `logical_invocation_id`, the selected target, ordered input items, ordered
instructions, provider-neutral tool definitions, requested output limit, closed tool-choice
policy, typed provider-native options, and `context_manifest_id`. The request always reports
`parallel_tool_calls=false`; callers cannot construct `true`.

Input items preserve exact vector order and distinguish system/developer/user message content,
prior assistant content, canonical tool call, paired canonical tool result, historical refusal,
structured data, synthetic runtime status, and provider-guarded opaque continuation. Stage 15 does
not query history, select eligible events, prune tokens, or construct a context manifest. Stage 16
will construct the fully rendered request and owns its final authoritative rendered-request hash.

Instructions are ordered provider-neutral text parts. Model-facing tool definitions project the
Stage 14 name, implementation/schema version, description, and canonical input schema without
provider function DTOs. Tool choice is the closed `automatic` or `none` policy; it is not an
arbitrary provider string.

## Ordered output and complete tool calls

A canonical response contains selected-target identity, ordered `output_items`, stop/incomplete
reason, usage, optional bounded provider request/response IDs, optional provider continuation, and
bounded typed metadata. Output items are exactly `text`, `tool_call`, `structured_data`, `refusal`,
`reasoning_summary`, `provider_opaque`, and `unknown_provider_item`.

Text holds ordered parts and is never destructively concatenated. A tool call preserves the exact
bounded provider/canonical call ID, syntactically validated `ToolName`, complete raw argument
string, and canonical parsed JSON when parsing succeeds. Malformed completed JSON remains
represented with its raw bytes and is rejected explicitly before tool eligibility. Semantic Tool
Registry resolution and tool execution remain Stage 17 responsibilities. Duplicate call IDs in one
response fail closed. Multiple calls may be represented but remain in provider order and are not a
request for parallel execution.

Structured data is bounded canonical JSON. Refusal is explicit semantic output, not a transport
failure. Reasoning summary contains only provider-exposed summary text; there is no hidden/private
chain-of-thought type. Provider-opaque evidence records provider, type/version label, bounded exact
opaque value, and SHA-256. Unknown provider items retain the same bounded diagnostic evidence and
cause supported-semantics validation to fail; nothing is silently dropped.

`ModelResponse::try_new` immutably validates the observed-order `output_items` vector and moves the
same vector unchanged into the canonical response. A provider normalization seam must map each
observed semantic item through a one-input to `Result<one output, error>` function and append each
successful result in observed order. It cannot expose the output vector as a mutable vector/slice,
reconstruct it from itself, or use filtering, folding, partitioning, continuation, or subset
helpers to decide survival. Unknown input therefore produces retained unknown evidence or an
explicit unsupported-item error.

The constructor freezes the complete terminal matrix. Completed requires text or structured data,
forbids tool/refusal/unknown items, and permits reasoning or opaque evidence only alongside normal
answer content. Tool continuation requires a complete tool call, forbids refusal/structured/unknown
items, and may retain ordered text/reasoning/opaque evidence. Refusal requires refusal content and
forbids text/structured/tool/reasoning/unknown items. Incomplete/provider-limited output may retain
non-executable text/structured/reasoning/opaque evidence but forbids tool/refusal/unknown items.
Cancellation has no exposed semantic output and may retain only opaque evidence. Provider failure
has no usable answer, tool, structured data, refusal, or reasoning; it may retain opaque/unknown
diagnostic evidence, with unknown evidence still rejected by supported-semantics validation.
Permanent validation declares an independent six-stop-reason by seven-semantic-class oracle and
executes all 42 single-semantic cells, plus explicit mixed-item contradiction and coexistence cases.
The expected table does not call the response constructor or a production semantic classifier.

Empty normal completion, contradictory terminal combinations, duplicate call IDs, malformed
completed arguments at eligibility, and unknown correctness-bearing items fail closed without
dropping an item. Incomplete/provider-limited output is a definite semantic provider failure, not
transport ambiguity.

## Usage, canonical serialization, and limits

Usage is nonnegative and signed-64-bit-compatible for input, cached input, output, reasoning, and
total tokens. It requires cached input not greater than input, reasoning not greater than output,
and total equal to input plus output with checked arithmetic. Zero and exact `i64::MAX` boundaries
are valid where the relationships hold; overflow and contradictions fail.

Provider-neutral request/response hashing uses compact UTF-8 JSON, recursively ordered object keys,
stable snake-case tags, preserved list order, and SHA-256. It hashes semantic values only—never a
memory address, timestamp, secret, provider wire DTO, or runtime identity unrelated to the request.
Limits measure the actual compact serialized bytes, not a pre-escaped estimate:

- at most 64 ordered output items;
- at most 65,536 UTF-8 bytes in one complete raw tool argument string, inclusive;
- at most 262,144 bytes in the canonical serialized normalized response envelope.

An overflow rejects the whole semantic output. No item, argument, or part is truncated or dropped.

## Canonical stream events and terminality

The internal stream inventory is provider-neutral: response started with target/ID evidence, text
delta, reasoning-summary delta, tool-call started, tool-argument delta, tool-call completed,
refusal delta/completed, structured data, usage, explicit usage-unavailable, completed response,
closed provider error, and bounded unknown-provider-event evidence. It contains no HTTP, SSE, or
provider event name.

An explicit state machine preserves item ordinals and call IDs. It requires one initial started
event, no earlier/duplicate start, bounded ordinals, unique call IDs, exact tool
start/delta/completed identity, the aggregate 65,536-byte argument ceiling, no incomplete call at
successful completion, exactly one usage result immediately before exactly one terminal, and no
event after usage or terminality. Successful completion requires one usage value equal to terminal
response usage. Provider-error completion requires exactly one usage or usage-unavailable result;
zero usage is never synthesized. Started and terminal-response target identities must match.

Terminal state distinguishes completed, definite provider failure, cancellation, outcome unknown,
timeout before output, timeout after output, and malformed/unsupported protocol failure. Definite
authentication/request failures, transient pre-output failures, cancellation, ambiguity, both
timeout certainties, and protocol failures never collapse to completed. Contradictory before/after-
semantic timeout classifications fail validation. Unknown provider events count as semantic
evidence and are never retry-safe by omission.

## Provider and estimator ports

`ModelProvider` is object-safe, provider-neutral, and receives an already-selected canonical
request, positive attempt number, immutable absolute deadline/idle timeout, cancellation token, and
optional fixture key. It exposes provider identity/capabilities and returns an owned event stream.
It does not select, persist, journal, execute tools, or translate client protocol.

`TokenEstimator` is object-safe and returns its exact estimator ID/version plus a conservative
signed-64-bit-safe estimate for caller-rendered text, structured, tool-definition, and opaque byte
units. It neither queries history nor assembles context. The deterministic scripted estimator is a
fixture seam for Stage 16 tests. Its immutable lookup key is estimator identity plus selected target
identity and exact normalized unit sequence. Lookup never consumes a value, equal input always
returns the same estimate, missing/duplicate keys fail deterministically, and programmed output is
at least the checked sum of its nonnegative byte units.

## Provider failure, certainty, and retry

The stable provider categories are authentication, authorization, invalid request, unknown model,
rate limited, temporarily unavailable, transport before response, transport after possible
processing, timeout before output, timeout after output, malformed response, malformed completed
tool arguments, output too large, unsupported response item, context error, safety/refusal,
cancelled, provider outcome unknown, internal provider error, deterministic script mismatch, and
invalid scripted program.
Stage 19 maps HTTP/network/provider conditions into these categories without storing raw provider
messages. Generic provider code contains no HTTP status switch.

Outcome certainty is exactly definitely not sent, definite provider rejection/failure, semantic
output observed, or provider outcome unknown. Ambiguous/possibly processed outcomes are never
automatically retried and will later map to durable `provider_outcome_unknown`.

The classifier allows no more than three attempts total: initial plus two retries. Only rate limit,
temporary unavailable/5xx-equivalent, transport/connect/reset before response, and idle timeout
before output are eligible, and only before semantic output with definite outcome evidence and
remaining attempt/deadline/cancellation budget. Authentication, authorization, invalid request,
unknown model, schema/context error, refusal, malformed completed arguments, oversized output,
unknown semantic item, cancellation, ambiguity, deadline exhaustion, and every condition after
semantic output are nonretryable.

The pure backoff policy uses a 250 ms base, exponential local ceiling capped at 5 seconds,
provider Retry-After ceiling capped at 30 seconds, and injected full jitter from zero through the
selected ceiling. It never sleeps. Cancellation and insufficient remaining absolute deadline veto
the delay.

## Deadline and cancellation contract

The provider invocation default is five minutes and stream idle timeout ceiling is 60 seconds.
The frozen attempt deadline is the earliest absolute Work deadline, shutdown deadline, provider
invocation deadline, and retry-budget deadline. Later code passes that absolute value unchanged and
must not reconstruct a narrower-known deadline from a fresh relative timeout. Cancellation is an
awaitable provider-neutral token. `ScriptedProvider` retains an injected monotonic clock, the
absolute deadline, idle timeout, last valid activity, cancellation token, and semantic-output
observation. Before each emission and after each release barrier it checks cancellation, then
overall and idle expiry. Cancellation wins a same-observation race and `now >= threshold` is
expired. Pre-output timeout is retry-eligible only with definite evidence; timeout after text,
tool, refusal, structured, reasoning, or unknown semantic evidence is nonretryable. Tests advance
the injected clock and use barriers, never wall-clock sleeps.

## Deterministic ScriptedProvider

`ScriptedProvider` is the only concrete Stage 15 provider adapter and is fixture-only; it is not a
live provider identity and production does not compose it. Programs are consumed in deterministic
queue order and can match selected target, canonical request SHA-256 or fixture key, required prior
tool-result call ID, invocation ordinal, and attempt. Mismatch is a fixed redacted failure.

Programs emit provider-neutral events, terminal classified errors, or wait at an injected release
barrier. A step after the first emitted terminal or terminal failure makes the whole program
invalid before first emission. They never use a network or sleep. Capture records exact canonical
request, selected target, request hash, invocation/attempt count, emitted event order, cancellation
observation, and exact completion/provider-failure/cancellation/outcome-unknown/timeout/script-
mismatch/invalid-program terminal classification. Capture `Debug` exposes only IDs, hashes,
counts, and classifications—not request content, instructions, tool arguments/results, refusal
text, or opaque bytes.

Permanent scenarios cover text; one/mixed/multiple tools; refusal; structured data; reasoning
summary; opaque continuation; usage/IDs; pre-output transient failure; post-output failure;
malformed/oversized/duplicate calls; before/after-output timeout; cancellation; unknown item;
request mismatch; prior-tool matching; and a deterministic machine-inspection answer. The same port
and stream contract will apply to the Stage 19 real adapter. A provider-neutral reusable harness
drives only `ModelProvider`, `ModelProviderStream`, and a narrow fixture factory; it never reads
ScriptedProvider fields, stream internals, program queues, or captures. Scripted-only tests remain
in addition to that shared suite.

The Stage 15 structural checker freezes the repository-specific published catalog, selector, and
canonical response-construction shapes. Its bounded alias/helper analysis rejects mutable catalog
storage or escape after construction; the selector topology permits successful provenance only
from the explicit/default exact-ID helper; and canonical response construction permits immutable
validation plus unchanged ownership transfer only. These structural proofs make neutral helper
names and alternate mutation/filter syntax irrelevant.

Every permanent Stage 15 semantic checker mutation first changes an isolated temporary repository,
passes `cargo check --locked --workspace --all-targets`, and only then must fail the checker. A
noncompiling mutation fails the probe harness. One reusable incremental target keeps the suite
bounded, and compilation-gated legitimate controls retain constructor-local building, considered-ID
iteration, model-target sorting, canonical JSON-key sorting, and unrelated fixture filtering.

## Redaction and provider-wire boundary

Stage 15 code does not trace raw content. Safe diagnostics are target/provider/model IDs, canonical
hashes, counts, capability flags, retry category, and durations. Raw user content, instructions,
tool arguments/results, refusal text, provider opaque values, credentials, headers, and native
option secrets are forbidden from logs/debug capture. Canonical options contain no secret value.
Provider wire structs, Authorization handling, request bodies, response bodies, HTTP/SSE decoding,
and raw provider errors may exist only in the future Stage 19 adapter.

## Later-stage ownership

- Stage 16 owns eligible-history queries, causal frontier, context projection, token-budget
  pruning, context manifests, and the final rendered-request hash.
- Stage 17 owns `ModelGateway`, invocation persistence lifecycle, retries/backoff execution, the
  explicit model/tool agent loop, production `WorkRunner`, scheduler activation, and final answer
  persistence.
- Stage 18 owns model crash failpoints and crash/recovery validation at model boundaries.
- Stage 19 owns OpenAI wire types, Reqwest, credentials, HTTP/SSE, live keys, fixtures, and live
  provider verification.
- Stage 20 owns client draft streaming.

Production remains `live_unready`; no Scheduler or WorkRunner is activated. Deferred Stage 13
Ubuntu verification status is unchanged.
