# Worker HTTPS transport v1

These closed JSON wrappers adapt the immutable distribution bundle 1.0.0 and worker protocol 1.0. They do not alter the bundle (digest `5800dbae670b0cff7eb4c9e7b82572ccf81866d63afb0ca0f36490aa54b7ecdb`). All inner messages use the frozen decoder, canonical digests and signing preimages. JSON rejects unknown properties, duplicate decoded keys, invalid UTF-8/BOM/surrogates, depth above 32, and oversized structures before side effects.

| POST path | Request | Response |
| --- | --- | --- |
| `/v1/worker/poll` | `{contract_version:1,request_id:U4,authority:Authority,boot_id:Uuid,power_operation_id:U4,max_items:1..4,wait_ms:0..20000}` | `{contract_version:1,request_id:U4,items:[WorkDelivery|CancelDelivery|ExecutionProfile],retry_after_ms:0..20000}` |
| `/v1/worker/requests/challenge` | `{contract_version:1,request:WorkReceipt|CancelReceipt|ResultExport}` | Frozen `SensitiveSigned` template |
| `/v1/worker/receipts` | `{contract_version:1,request:WorkReceipt,proof:SensitiveRequestProof}` | Frozen `ReceiptStatus` with committed original `Receipt`, after database commit |
| `/v1/worker/control-receipts` | `{contract_version:1,request:CancelReceipt,proof:SensitiveRequestProof}` | `{contract_version:1,request_id:U4,control_id:U4,control_digest:Digest,accepted:true}`, after database commit |
| `/v1/worker/results` | `{contract_version:1,request:ResultExport,proof:SensitiveRequestProof}` | Frozen `ResultAcceptance`, after atomic import/outbox commit |
| `/v1/worker/result-status` | Frozen `ResultQuery` | Original frozen `ResultAcceptance` or bounded reconciliation/conflict error |
| `/v1/worker/receipt-status` | Frozen `ReceiptQuery` | Frozen `ReceiptStatus`; uncertain missing admission is `reconciliation_required` |

No URL/query contains credentials or identities; query properties are forbidden. Exactly one `Authorization: CraxiiWorker <credential>` header is required. TLS verifies chain and hostname; redirects, proxy discovery and forwarded identity are not trusted. Responses are no-store. Auth requests retain finite five-second timeouts. Polling currently supports only `wait_ms=0`, advertised as such during negotiation. Requests are sequential with bounded reconnect backoff and jitter. No SQL transaction waits on network I/O.

Poll requests and receipt/control/challenge wrappers are limited to 16,384 bytes / 4,096 nodes. Work deliveries are limited to 524,288 bytes; poll responses to 1,048,576 bytes / 32,768 nodes and four total items, counting profiles. Each inner message is independently validated. Decoded work content is 1–64 blocks and at most 65,536 UTF-8 bytes. Errors are at most 4,096 bytes. The server may return fewer items (including zero). An inseparable profile/work pair or control/material reconciliation pair requires capacity for two items.

A signed, current session authenticates server-selected polling. Polling issues a finite transport lease under customer/agent/epoch/installation/binding/session locks, followed by a fresh authorization decision before COMMIT. It grants no execution or spending authority. Receipt and control import separately require a fresh one-use request challenge, Ed25519 proof over the full current request, current scopes, and the same transaction's final authority/time check after writes, audit and deferred constraints. No guarantee about challenge freshness through physical WAL persistence is made.

Admission-capable isolated installations require `status_v1`, `signed_request_v1`, `admission_v1`, `cancel_v1` and SQLite schema 8 or 9. They receive exactly `status`, `receipt_commit`, `cancel` scopes. Older status-only peers retain their existing handler and receive no work. Production/default composition cannot select fixture trust or profiles and has no delivery routes or ordinary readiness.

The server derives ownership and the current recipient. Boot must match the negotiated boot context; power lineage must match the owned fixture/profile registry. Issuance rows retain installation/generation/session/binding/epoch/boot/power/fence/deadline evidence. A current lease can be returned unchanged to the same session; expiry/session/generation replacement issues a higher fence for the same work identity. A lease expiry is not evidence that admission or execution failed. At most one unresolved normal delivery per conversation advances; controls can reconcile its pinned predecessor.

The worker verifies delivery context/deadline again after waiting for SQLite, then resolves an existing receipt before requiring a new profile. Exact replay returns the original UUIDv7 receipt/local IDs and original installation/generation; the current signed transport is independent. New fixture admission requires pinned profile signature, exact work/profile/config/tool/deadline binding and real owned period/reservation lineage. Fixture power/config/grant references are synthetic, not running authorization. Receipt-only and production profiles keep the distribution claim gate closed. Only an explicitly selected, signed `terminal_text_v1` profile in the debug failpoint fixture can claim its exact work through the existing claim/AgentLoop path; its tools are disabled and its provider is network-free.

Cancellation pins one target once. Never-leased work is cancelled centrally without wake. A possibly delivered target receives a durable control, even after entitlement closure. An absent local work commits a cancellation tombstone. When sequence evidence is needed, the server returns that control before the original WorkDelivery in one response: the worker processes controls first and reconciles the immutable material/sequence without creating a message or work receipt. Control acknowledgment waits for this reconciliation for absent work; the material does not authorize execution. This uses existing frozen messages without an added cancellation-sequence field.

Queued local work cancels through the existing mutation; active work requests cancellation and reports cleanup pending. Terminal work absorbs cancellation. The stable first control receipt is history, not a claim that an external effect was reversed. Local runtime recovery retains queued work and conservatively handles uncertain attempts. Replacement/reconnection never changes the dedupe key or requeues terminal/interrupted work. A missing historical sequence or contradictory identity quarantines/holds; it does not authorize re-execution. Receipts, dedupe fingerprints and control tombstones have no automatic expiry purge.

Result-capable isolated installations additionally negotiate `result_v1` and SQLite schema 9, receiving exactly `status`, `receipt_commit`, `cancel`, `result_export`. Schema 8 admission and status-only peers remain supported. A cleanup session grants no new chargeable execution. The current installation, generation, key, session scope and authorization epoch are checked through the existing guard; entitlement expiry alone does not erase completed-result cleanup authority.

Result inner requests are bounded to 524,288 bytes / 32,768 nodes. Result upload and result challenge wrappers are bounded to 557,056 bytes / 36,864 nodes. Depth remains 32. The route-aware raw parser retains strict duplicate-key, scalar, UTF-8 and BOM rejection; ordinary receipt/control wrappers remain 16,384 bytes. Proof, result query and acceptance responses remain 16,384 bytes / 4,096 nodes; errors remain 4,096 bytes. Result content is version 1, 1–64 nonempty text blocks and at most 65,536 decoded UTF-8 bytes. Unicode, controls including escaped NUL, whitespace and normalization form are preserved. HTML/JSON-looking strings stay opaque text. Nonempty artifact references are unsupported; no URL or file is fetched.

Export UUIDv7, logical upload UUIDv4, terminal revision, original receipt/work/message identities, original installation provenance, content and both wire digests are immutable. Native message hashes use their existing binary content framing; wire content hashes use cjson and the export digest uses the frozen result domain. The worker reconstructs successful content from the original immutable assistant message. Notices do not create assistant messages. Transfer never reruns model/tools or allocates a new export after losing a response.

The central service derives destination from the immutable owned work/ingress/route link. Workers submit only the original opaque binding reference. Accepted result metadata, stable acceptance UUIDv4/time and exactly one pending original-route outbound source UUIDv4 commit together. Content is stored once as canonical escaped JSON TEXT; PostgreSQL JSONB is not used for delivery text. No external sender is mounted and no duty is marked delivered. Exact duplicates recover the original acknowledgment; altered content, identity, upload or digest conflicts. Pending content has no automatic purge.

Result query requires current authentication and `result_export` scope but no mutation proof. Missing owned import returns `reconciliation_required`; conflicting final identity/digest returns `result_conflict`. Current same-agent replacements can query/replay accepted imports. A first import of old-origin work requires a server-recorded work/receipt/material/origin/current-installation recovery grant. Workers cannot mint routes or grants. Stale workers lose submit/query authority.

Route fixtures require the marked isolated test database, allowlisted canonical customer, authenticated customer principal and sealed fixture capability. Explicit `synthetic_v1` admission mints a binding/account centrally; compatibility fixtures preserve their original opaque binding. Destinations are fixed to the synthetic owner and never supplied by workers. Fixture provenance is constrained to test and no production constructor or ingress producer exists here. Real ingress authorization belongs to its later owning stage.

The existing process scans at most four pending exports, queries before retry and persists the original acknowledgment in SQLite before considering transfer complete. Pending, quarantined, inconsistent and in-flight state blocks export drain. The observation uses the SQLite write barrier and reports durable pending/accepted/unresolved counts plus process in-flight count. Graceful shutdown may retain pending rows; export drain is only one prerequisite of future safe sleep, never a cloud stop authorization.

## Central model transport extension 1 (isolated implementation candidate)

The frozen worker messages remain version 1.0; the bundle and SQLite schema are unchanged.
This lane is available only through explicit isolated model-service construction. Default
and production startup/readiness remain closed. Real Foundry API/authentication/billing
qualification and ordinary agent activation are separate gates.

`POST /v1/worker/requests/challenge` accepts `{contract_version:1,request:ModelInvoke}`
only with an explicitly admitted model service and model-capable installation/release.
It signs the existing `model_invoke` purpose over the whole current authenticated request.
`POST /v1/worker/model-invoke` accepts `{contract_version:1,request:ModelInvoke,proof}`.
Both use the existing single `Authorization: CraxiiWorker <credential>` header; query
parameters are rejected. Inner invocation bytes are at most 1048576; outer wrapper at
most 1081344 bytes / 36864 nodes / depth 32; the proof retains its 16384-byte limit.
The inner message is independently decoded under the frozen limits. Duplicate decoded
keys and invalid UTF8 are rejected before JSON parsing loses evidence.

A newly admitted original returns `application/x-ndjson`: one canonical frozen ModelEvent
per LF-terminated line. Sequence starts at 1 with `response_started`, followed by ordered
semantic events and a single `completed` or `outcome_unknown` terminal event. A complete
answer requires coherent terminal output; EOF, partial tokens and partial tool arguments
never constitute success. Each JSON frame is at most 65536 bytes; LF overhead counts
inside the 2097152-byte stream bound. At most 4096 events and 131072 queued bytes are
permitted. Parser/transport failure or missing terminal evidence interrupts the attempt.
The Rust translator also waits for bounded EOF after terminal evidence and rejects
post-terminal bytes. Existing gateway validation remains authoritative for assembled
semantic/tool-call consistency.

An identical authenticated physical attempt returns the original committed NDJSON
stream when bounded output is available. Otherwise it returns `application/json` with
the original `attempt_id`, `invocation_digest`, `state`, `reservation_id`, `operation_id`,
`output_recoverability`, nullable `settlement_id`, nullable `evidence_id`, and
`retry_allowed`, plus `contract_version:1`. Known cost and recoverable output are
separate facts; unavailable output must interrupt work. Different invocation material
cannot reuse the physical UUIDv7. A fresh HTTP request, proof, session or controller
lease never creates permission to redispatch that attempt.

`POST /v1/worker/model-status` accepts the closed wrapper
`{contract_version:1,request_id:UUIDv4,attempt_id:UUIDv7,invocation_digest:SHA256}` under a
current owned status session. It returns the same bounded JSON projection, with no
reserve/dispatch/settlement side effect. Expired entitlement does not itself prohibit
this lookup; stale/revoked customer, key or generation still denies it. No worker-facing
usage/settlement endpoint exists.

Local synthetic bounds: four outstanding model liabilities globally, one per customer
or installation; at most 1024 retained model attempts, with no automatic purge. Shared
JobRunner slots additionally constrain active execution. Connect 2s, first event 5s,
idle 5s, total 30s, slow consumer 2s; work/profile and current compute cutoffs may be
earlier. Authentication/control requests retain 5s. Disconnect detaches delivery but
cannot refund or prevent available trustworthy terminal usage from being recorded.

The only current provider manifest is `azure_foundry_synthetic` /
`synthetic-responses-1`. It serializes fixed central deployment/model/options and the
complete neutral request. Its explicitly synthetic byte tokenizer bounds every serialized
input byte; input/cached input/output/reasoning/request/other are all priced with checked
integer rational ceilings and immutable model/price versions. This proves no live Azure
token bound, provider idempotency, credit eligibility or production capacity. SDK/HTTP
inference retries and redirects are disabled. No worker endpoint/header/price/credential
passthrough or direct paid-provider fallback is present.

The installed debug + test-failpoints fixture can explicitly select signed
`proxy_loop_v1`. It uses the existing guarded SQLite claim, AgentLoop, context assembler,
ModelGateway, tool service and atomic terminal/export owner. Trusted injected
ModelProxySettings must match the signed model/price pins. The protected installed
compute permit is required; the tool registry and execution lookup are restricted to
`run_shell`, with its original five-second maximum. Existing durable controls are polled
during the loop. Ordinary composition remains receipt/export-only with readiness closed.
Legacy native development providers remain separately selected; distribution never
falls back to a provider credential loader.

Status replies additionally bind `request_id`, `attempt_id` and `invocation_digest` and
report `state` (reserved/dispatch_intended/uncertain/settled/released/support_needed),
`dispatch_state` (not_dispatched/possible_effect), `provider_acceptance`
(not_dispatched/unknown/accepted/rejected_no_charge/rejected_usage_unknown), `output_state`
(unavailable/complete), `output_recoverability` (unavailable/available), original reservation and
operation IDs, and nullable evidence/settlement IDs with `retry_allowed`. Complete
financial evidence is separate from recoverable output. Accepted evidence is persisted
before forwarding semantic provider output. A complete validated response and its
bounded semantic events are stored as immutable escaped canonical text in the same
transaction as provider usage and the original-period financial projection. The
existing 4096-event / 2097152-byte framed stream and 65536-byte frame bounds also
apply to persisted replay. If a coherent provider result exceeds frozen worker
framing, its normalized completed output and known usage still persist; status
reports output complete with wire replay unavailable. This cannot turn known usage
into provider uncertainty. Status cannot dispatch, reserve or settle.

`POST /v1/worker/model-result` accepts the same closed 16384-byte request as model-status
under current authenticated ownership and returns the original committed ModelEvent
stream as NDJSON. Request/session correlation is rebound to the current retrieval;
physical attempt, output, event ordering and evidence remain the original facts.
Retrieval always selects the first committed replayable response, including its
original terminal usage fact. Late usage may settle the backend ledger and update
financial status; it cannot replace replay frames Rust already consumed.
HTTP replay readers share the four-stream capacity and have an absolute 30-second
transport bound. This read does not need new spending/compute authority and cannot
reserve, settle or dispatch. A duplicate model-invoke returns this stream when output is available;
otherwise it returns the existing JSON status projection. Revoked/foreign/stale
identity is denied. Missing usage retains the original liability even when output
can be retrieved. Conflicting output cannot replace an earlier completed response.

Rust can retrieve the original committed result after a transport interruption,
within the original monotonic model and compute deadlines. At most two stream
recoveries use read-only status/result requests, never inference transmission. Each
status recovery has at most 30 checks spaced by 100ms, clamped by the original
deadline. Already-consumed frames must match the immutable replay prefix exactly
and are not delivered twice. Completion still requires a validated terminal frame,
clean EOF and the existing gateway's semantic checks. Parser contradictions remain
errors. A restarted worker may retrieve past output under the original attempt;
existing SQLite interruption rules do not automatically rerun interrupted work/tools.

Worker transactions acquire shared gates before owner, agent, epoch, installation
and session/work locks. Model admission/dispatch uses gates, global admission limits,
owner, agent, epoch, sorted periods/balances, prerequisites, then worker/attempt and
operation/reservation records. Past-fact settlement takes exclusive gates first
because incidents can close admission. Database recording retries at most three
transactions over the same completed fact and unique provider digest, inspecting
durable evidence after an ambiguous commit. A database rollback cannot substitute
an unknown provider result for available completed output. Provider I/O never occurs
inside these transactions and is never retried by this mechanism.

Only an exact released predecessor, explicit retry evidence and an unchanged logical
request authorize a successor through the existing gateway's three-attempt bound.
HTTP retry retains the physical ID and digest; a successor uses a new committed physical
ID, ordinal, reservation and central operation with the original predecessor/evidence.
Unknown, partial or unavailable output never authorizes inference replay. Permanent
zero-charge rejection is definitive but nonretryable. Only synthetic throttling with
explicit no-charge evidence supplies the current transient retry authorization.

Internal optional provider lookup is read-only and bound to the original physical and
central attempt. Available trusted late usage settles the original period. Duplicate
facts do not charge twice. Conflicting late facts append an incident and close admission
without overwriting an immutable settlement or forgiving uncertain liability. There is
no worker provider-evidence or financial-settlement endpoint.

Conflicting trusted evidence sets `reconciliation_required:true` in the status projection, including after immutable settlement/release. It vetoes `retry_allowed` without rewriting the financial history. The incident disables the affected configuration and closes the model-spending gate with audit evidence. Original residual exposure remains in global admission holds; all same-owner unresolved model reservations, including legacy reservations and old periods, prevent new model work. An affected configuration cannot be re-enabled while conflict evidence exists.
