# Stage 14 tool registry, authority seam, and tool execution service contract

## Status and sources

Accepted for Craxii V0.0.01 local implementation on 2026-08-30. The normative source remains
[`craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md), and the dependency order remains
[`craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md). This record fixes
Stage 14 choices without introducing Stage 15 model/provider/context/agent-loop behavior.

## Persistence and dependency boundary

Stage 14 uses migrations `0001`–`0003`, schema ceiling `3`, and the existing V3
`tool_executions`, artifacts, work, journal, uniqueness, and recovery contracts. It adds no migration
and does not change V1, V2, V3, or Stage 11 protocol fingerprints. There is no new crate. The
toolset fingerprint is a distinct semantic identity from the general runtime configuration
fingerprint. Effective model-visible tool defaults and limits are sourced from the validated typed
configuration and therefore intentionally affect both the canonical tool definitions/toolset
fingerprint and the distinct general configuration fingerprint.

## Registry identity and canonical definition

Production constructs one immutable startup registry containing exactly, in order,
`read_file` then `run_shell`. Each has implementation version `1.0.0` and schema version `1`.
`ToolName` is 1–64 bytes, lowercase ASCII, begins with an ASCII alphanumeric, and thereafter permits
only lowercase letters, digits, `.`, `_`, and `-`. `ToolVersion` is 1–64 visible non-whitespace ASCII
bytes. Schema versions are positive integers. Names and versions are exact identities with no alias,
normalization, or dynamic rename.

A definition serializes only stable semantic facts in fixed field order: name, implementation
version, schema version, description, canonical input schema, required Workstation capability,
side-effect classification, supported privilege modes, default/hard timeout policy, and
output/capture policy. Object keys in schema and definition JSON are recursively ordered and output
is compact UTF-8 JSON. The registry SHA-256 is over the compact canonical JSON array in registry
order. Handler references, `Arc` identity, addresses, function pointers, workstation/runtime IDs,
PID, timestamps, and configuration fingerprints are excluded. Construction rejects duplicate names
and exposes definitions/lookup/fingerprint but no runtime mutation or plugin API. The fingerprint is
deterministic for a given validated semantic policy; changing an effective default or limit changes
it by design.

## Typed inputs and validation

The completed encoded-argument envelope limit is 524,288 bytes. It is deliberately larger than the
65,536-byte command field so worst-case JSON escaping does not make the advertised field boundary
unreachable. Validation order is raw size, duplicate-key scan, JSON
syntax, top-level object, exact typed decode with unknown fields denied, semantic bounds/path
construction, default injection, compact recursive key ordering, and SHA-256. Duplicate keys are
rejected at every object depth by a Serde visitor before ordinary typed decoding; last-key-wins is
never accepted. Persisted accepted argument JSON is the normalized object with explicit defaults,
not caller formatting. Because the V3 requested row must exist before a model-visible rejection,
unknown-tool and invalid-input attempts persist a fixed redacted sentinel object and its hash; raw
malformed or unknown arguments are never persisted as if canonical.

Every schema-accepted value must pass the typed decoder in the same semantic context. Executable
golden tables cover grammar, enum, unknown-field, NUL, multibyte, and byte/range boundaries.
Byte-bounded paths and commands use a printable-ASCII schema/decoder contract so schema character
counts cannot undercount UTF-8 bytes. Relative/absolute path-kind branches encode the exact
deterministic no-NUL, no-backslash, no-empty/dot/traversal-segment syntax; existence, symlink, and
containment remain semantic filesystem validation.

`read_file` accepts required `path`, optional `path_kind` (`workspace_relative` default or
`absolute`), and optional positive `max_bytes` (configured default; configured maximum no greater
than 8,388,608).
`run_shell` accepts required nonempty ASCII `command` of at most the configured 65,536-byte hard
ceiling and without NUL,
optional logical `cwd` (workspace root default), optional `privilege` (`user` default or
`administrative`), and optional positive `timeout_seconds` (configured whole-second default and
maximum, never above 900 seconds). It has no
environment, workstation selector, effective privilege, credential, output widening, or provider
field. Hand-authored canonical schemas are provider-neutral. Tests independently evaluate the
actual emitted schema AST without decoder grammar helpers, fail on unknown schema keywords, cover
the full configured/hard boundary matrix, and mutate emitted limits/patterns to prove divergence
detection; `schemars` is not added.

## Handler and machine boundary

The internal typed `ToolHandler` receives only a validated typed input, an injected execution
context, and the Workstation dependency required for its action. The `read_file` handler calls
`Workstation.read_file` once. The `run_shell` handler calls `Workstation.execute` once with the
service-generated durable `ExecutionId`. Handlers do not receive StateStore, SQLx, SQLite, a journal
writer, Axum/provider types, raw secrets, or a generic environment. They do not persist, journal,
authorize, retry, transition Work, choose a workspace/generation, widen privilege/timeout/output, or
use direct filesystem/process/cgroup fallbacks. The service owns validation, authority,
preparation, deadlines, persistence, artifacts, cancellation orchestration, and outcome
classification.

## Non-side-effecting preparation contract

The five-method public Workstation trait remains frozen. A separate provider-neutral
`WorkstationPreparation` port prepares cwd evidence after requested intent commits and before
dispatch intent. LocalWorkstation validates workstation ID, generation, workspace, required
capability, and effective privilege feasibility; resolves the requested cwd through the same
adapter-owned resolver; opens the resolved directory non-mutatingly; verifies its file type; and
returns bound `PreparedCwdEvidence` containing canonical `ResolvedPathEvidence`, device, inode, and
the expected directory type.
It does not spawn, execute, read requested tool content, create a cgroup/artifact, mutate, persist,
or journal. The actual Workstation call revalidates identity/path and owns any OS handle, making the
prepared path evidence observational rather than an authority token.

For `read_file`, the effective cwd is the workspace root and preparation proves the dispatch
workstation/workspace binding; the requested file path is resolved only by `read_file` itself so
preparation never reads or pre-opens requested content. For `run_shell`, preparation resolves the
normalized requested cwd. The dispatch evidence stored in the existing V3
`authority_decision_json` column is versioned dispatch evidence and includes both complete authority
facts and the prepared cwd path/object identity; the historical column name does not narrow that
dispatch contract. Execution receives the exact committed `PreparedCwdEvidence`, opens that
committed directory without following a final symlink, obtains metadata from the exact descriptor,
and before spawn requires the logical binding, directory type, device, and inode to match. The child
uses that same validated descriptor for cwd. Drift, retargeting, same-path replacement,
disappearance, generation mismatch, or workspace mismatch is a definite pre-start failure;
execution never switches to a newly resolved target. No raw descriptor is persisted or exposed.
Preparation failure is definite and remains before dispatch.

## V0 authority contract

`AuthorityEvaluator` is typed and provider/model independent. Its input binds Craxii ID, Work ID,
runtime owner, workspace, registered tool identity, normalized argument summary/hash, requested
privilege, explicit structured constraints available from the caller, current Workstation
capabilities, required capability, and expected workstation generation. Its output is allow/deny,
effective privilege, policy `v0-development-workstation`, a stable reason code, and bounded canonical
evidence.

It denies unknown/malformed tools before invocation, wrong workspace, stale generation, cancelled
Work, missing capability, over-limit timeout/output/arguments, administrative requests for a tool
that does not support them or a workstation that does not advertise them, and injected attempts to
widen authority. Administrative execution requires all four facts: tool support, caller request,
policy allow, and administrative capability. Local macOS therefore denies admin before dispatch.
Capabilities are evidence, never authority.

## Identity, ordering, and transactions

The Tool Execution Service owns `ToolExecutionId`; one is generated for each accepted complete
logical call. It also generates one `ExecutionId` before the requested transaction and passes it
unchanged to `run_shell`. Each Workstation call receives a fresh service-owned `OperationId`.
Uniqueness of execution ID, `(work_id, agent_step_no, tool_ordinal)`, source invocation plus ordinal,
source invocation plus provider call ID, and one nonterminal attempt per Work is durable duplicate
dispatch prevention. A repeated logical request cannot reset or redispatch a row and there is no
HTTP idempotency surface.

After syntactic parsing determines the requested/default timeout, the service freezes the one
absolute monotonic deadline as the minimum of invocation start plus the bounded tool timeout, the
already-absolute upstream Work deadline, and an already-active shutdown deadline. The requested
transaction commits the normalized arguments/hash, exact tool/work/runtime/source
identity, workstation/generation/workspace, effective requested cwd, requested privilege, timeout
and output policy; inserts state `requested`; binds the current tool attempt; changes Work from
`running` to `waiting_on_tool`; and appends `tool.execution_requested` then caused
`work.waiting_on_tool`. No Workstation or preparation call precedes that commit.

After cancellation/current ownership recheck, capability observation, authority evaluation, and
non-side-effecting preparation, the dispatch transaction changes `requested` to `dispatching` and
persists resolved cwd path and stable object-identity evidence, authority allow evidence, effective privilege, effective timeout,
output policy, dispatch time, and `tool.execution_dispatching`. The authority field is the exact
canonical full evaluator evidence, including stable policy/decision/reason, privilege,
capability/bounded-request, topology, and tool identity facts; it excludes raw arguments, command,
content/output, secrets, and runtime pointers. Work remains waiting. This commit
precedes the sole handler/Workstation call. Database operations may be retried by their own adapter
rules, but a Workstation operation is never automatically retried.

For definite observations, required artifact bytes are finalized first. One outcome transaction
validates current attempt/runtime/work state, inserts and binds exact artifact metadata, completes
the tool attempt with canonical result/error/count evidence, appends required `artifact.recorded`
events then `tool.execution_completed`, clears the current attempt, resumes Work, appends caused
`work.resumed`, and commits. Only that commit makes the bounded result model-visible. Finalized bytes
left by a failed DB transaction remain unreferenced orphans and are not deleted or fabricated into
references.

## Deadlines, cancellation, panic, and uncertainty

Effective execution time is the minimum of the already-absolute Work deadline, invocation start
plus requested/default tool timeout bounded by tool/configuration limits, and the already-latched
Stage 10 shutdown deadline when applicable. That minimum is frozen once as an absolute
process-local monotonic deadline before requested persistence or capability acquisition and is
carried unchanged through authority, preparation, dispatch, handler, and Workstation. A later
Workstation capability limit can only shorten the persisted duration budget against that same
absolute deadline; it cannot reconstruct the deadline as a later `now + original_budget`.
Expiration during requested persistence, capability acquisition, preparation, dispatch, or
immediately before handoff performs no machine operation. The persisted policy contains bounded
duration facts, never the monotonic instant. Deadlines and cancellation never widen caller
authority.

Before requested commit, cancellation creates no attempt. Cancellation while `requested`, or after
dispatch commit while the execution lifecycle is still explicitly `PreHandoff`, drops the unpolled
handler future, completes with definite no-side-effect evidence, and never calls Workstation or
uses `cancel_execution(NotFound)` as proof of safety. The handoff state changes when that one future
is first polled. Once
`run_shell` handoff may have begun, the service calls `cancel_execution` with the exact
`ExecutionId`, continues owning and awaiting the original execute future, and persists only its
definite final observation or unknown. Dropping the execute future is not cancellation. In-flight
`read_file` has no invented cancel primitive and is classified only from honest Workstation
semantics. A handler panic before handoff is definite pre-action failure; a panic after possible
handoff is conservative `outcome_unknown`. No handler task is detached.

`requested` with no dispatch commit becomes `interrupted_before_dispatch`, with normalized reason,
no dispatch/machine/artifact evidence, `tool.execution_interrupted_before_dispatch`, interrupted
Work, and `work.interrupted`. Once dispatch committed, unconfirmed action/cleanup/panic/storage
continuity becomes `outcome_unknown`, with no fabricated result/streams, cleanup unconfirmed,
`tool.execution_outcome_unknown`, interrupted Work, and `work.interrupted`. Neither state is ever
automatically repeated with the same or a new execution ID.

Startup recovery maps stale requested attempts to `interrupted_before_dispatch` and stale
dispatching attempts to `outcome_unknown`, interrupts owning Work, leaves terminal attempts
unchanged, and never calls a handler, preparation, inspect, cancel, or Workstation execute/read.
Repeated recovery is idempotent and contradictory rows fail closed.

## Canonical results and artifacts

The provider-neutral result envelope binds tool/execution identities, exact tool version/schema,
terminal classification, the frozen result class, effective privilege when dispatched, bounded
summary and tool fields, duration/evidence when observed, artifact IDs, byte/truncation counts, safe
normalized error projection, and a bounded model projection. It never exposes PID/PGID/cgroup,
physical artifact/DB paths, secrets, raw stack traces, or environment.

`read_file` returns requested logical path, adapter-resolved evidence, byte length, SHA-256, UTF-8
projection, and `truncated=false` for the complete Workstation observation. At most 32,768 UTF-8-safe
projection bytes enter result JSON. Content beyond that projection is finalized as a generic
canonical-evidence artifact and referenced in canonical result JSON plus the same terminal artifact
transaction; stdout/stderr columns are not misused. Startup consistency recognizes this generic
role and requires the exact result reference, tool producer/work provenance, hash, size,
MIME/encoding, and metadata cardinality, rejecting foreign or stream-role artifacts. Invalid UTF-8,
NUL policy, missing, size, path,
symlink/outside, and changed-during-read behavior remains the Workstation's honest Stage 12 result.

`run_shell` maps exit zero to completed/success; nonzero to process_exit; confirmed signal to
signal_termination; confirmed timeout to timeout; confirmed cancellation to cancellation;
`start_observed=false` spawn failure to spawn_failure; and definite pre-spawn timeout/cancel to the
matching definite class. Cleanup failure or any unconfirmed post-dispatch cleanup is
`outcome_unknown`, not ordinary cleanup failure evidence. Stream bytes already finalized by
LocalWorkstation are verified and bound with exact producer, stream role/logical name, hash, captured
and observed sizes, inline and omitted counts, and truncation. Nonzero shell exit is a tool result,
not an infrastructure failure. Shell projection is serialization-aware: the service measures actual
canonical JSON bytes and deterministically shrinks UTF-8-safe inline stdout/stderr fields until the
V3 262,144-byte ceiling is met. Full stream artifacts, hashes, counts, omitted-byte truth, and
definite process classification remain intact even for quote, backslash, control, or multibyte
expansion.

## Failpoints, recovery, logging, and deferrals

Test-only hooks are activated after requested commit, after dispatch-intent commit, after process
spawn, after terminal observation and cleanup classification but before outcome commit, and after
artifact rename before DB commit. Permanent subprocess workers build and call the production
ToolExecutionService/LocalWorkstation/artifact paths; tests never invoke those five hooks directly
or duplicate production transitions. They use durable SQLite and markers across all five windows
to verify exact pre-recovery rows, conservative restart classification, coherent
Work/journal projection, zero-or-one side effect, no duplicate terminal outcome, no redispatch, and
orphan-byte behavior. Tests also use deterministic markers/barriers to prove no Workstation action before both required commits,
zero-or-one but never two dispatches, truthful recovery, durable outcome visibility, orphan-byte
behavior, and no automatic redispatch. Release exclusion remains enforced by the existing
debug-assertion compile guard and release scanner; no production failpoint control surface exists.

Logging permits stable IDs, tool name/version, state/class, bounded durations/counts/truncation,
privilege, capability/authority reason codes, and cleanup classification. Raw arguments, command,
paths, read/output content, physical artifact locations, headers, tokens, credentials, and raw
source/stack errors are redacted.

Stage 15 and later remain deferred: no OpenAI/provider type, Model Gateway/selection/capability
registry, context assembler, agent loop, assistant completion/draft streaming, client tool endpoint,
MCP/browser/cloud/database tool, credential injection, dynamic plugin, RemoteWorkstation, or
parallel tool call is added. Production remains `live_unready`; Stage 17 will consume the composed
service. Stage 13 Ubuntu/systemd/cgroup/sudo live target verification remains
`DEFERRED_BY_USER_TO_LATER_STAGE` and does not block Stage 14 local implementation.
