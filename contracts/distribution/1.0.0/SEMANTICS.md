# Normative distribution semantics

## Authority and identity

Schema validity, a digest, an ID, a Boolean claim or a fixture is never
credentials or authority. Derive a principal from verified customer tokens,
worker key/session proof or separately scoped operator authorization. Recheck
ownership, current generation and applicable fences at acceptance and commit.
The trusted worker is distinct from model-controlled shell execution.

Central customer, logical agent, installation, canonical work/conversation,
obligation, operation, attempt, reservation and period IDs are lowercase RFC UUIDv4.
Existing Rust local user/agent/conversation/work/message/receipt/export and model
logical/physical attempt IDs are lowercase RFC UUIDv7. Never cast, silently convert
or weaken local UUIDv7 validation. Persist an explicit central-to-local mapping.
OS boot/filesystem IDs are non-nil RFC-variant UUID versions 1 through 8 and grant
no account authority. Logical customer/agent/work survive replacement; immutable
installation changes and central generation monotonically increases. Revoked keys,
sessions or restored old generations never regain authority.

Cognito ownership is the exact verified issuer/subject pair, never email.
Telegram user/message IDs are positive decimal strings, update IDs nonnegative,
chat IDs signed nonzero strings. Uniqueness includes environment/bot/chat as
applicable. A username, supplied destination, heartbeat or process ID proves no
ownership. The worker sees an immutable opaque reply-binding ID/version, never a
bot credential or destination. A relink cannot retarget an old result.

Durable operation intent, physical effect attempt, transport request, lease owner,
job fence, resource fence/revision, worker generation, charge epoch, conversation
sequence, local journal offset, work revision and export revision are distinct.
Counters never wrap. Enrollment replacement checks the prospective authorization
epoch before changing any projection field. At signed i64 maximum it rejects with
`support_needed/manual_resolution/support`, preserving the complete prior state
and issuing no generation or revocation commit. New transport authority can use a new request ID/proof while
preserving canonical work/control/export/upload/model-attempt identity. A new
operation cannot evade conflicts or unresolved external effects.

## Wire encoding and limits

Keys are ASCII snake_case. Versioned messages include protocol, exact version and
closed type/body. Authenticated families also carry request ID, environment and
customer/agent/installation/generation/session lineage. Tokens/signatures remain
separate proof objects. Enrollment and session bootstrap do not authenticate merely
by decoding. `sensitive_request_proof` binds the current exact request digest.

Raw JSON is valid scalar UTF-8 without BOM, duplicate keys at any depth, invalid
escapes/lone surrogates, trailing garbage or ambiguous numbers. Decode escaped keys
before duplicate detection. Depth limit 32; control node limit 4096; large limit
32768. Count raw bytes before parse, canonical bytes after parse and decoded text
bytes separately. Unknown core fields/enums reject. Absent fields stay absent;
null is allowed only by an explicit schema, and no wire field currently permits it.
No default insertion, coercion or additional-property removal.

Control JSON is at most 16384 bytes; safe error/rejection 4096; enrollment 65536;
work delivery/result export 524288; model invocation 1048576; model event/outcome
65536. Poll batches later allow four deliveries and 1048576 total bytes, maximum
wait 20000 ms. No poll route is implemented by this contract library.

Content has 1–64 nonempty text blocks and at most 65536 total decoded UTF-8 bytes.
Each model text/opaque/tool-schema/argument component is at most 65536 bytes;
aggregate 524288. Model input items max256, instructions16, tools32, output items64;
item ordinal is zero-based 0–63, event sequence and physical attempt ordinal positive.
Transport stream later enforces 4096 events and 2097152 cumulative bytes. Opaque JSON
is exact bounded text, not an unconstrained outer object or executable authority.

## Canonical bytes and digest domains

`craxii.cjson/1` permits null, Boolean, Unicode-scalar strings, arrays, ASCII-keyed
objects and lexically canonical nonnegative JSON integers at most 2147483647.
Domain counters, money, tokens, durations and provider numeric IDs are strings.
Programmatic inputs must be plain data, with no getters, prototypes, toJSON,
symbols, undefined, sparse arrays, cycles, Date or BigInt serialization.

Sort keys by unsigned ASCII bytes, preserve arrays and exact Unicode without
normalization or trimming. Emit no whitespace. Escape quote/backslash and
BS/TAB/LF/FF/CR as their short JSON escapes; other control scalars use lowercase
`\u00xx`. Slash, noncontrol Unicode, U+2028/U+2029 and astral scalars emit UTF-8.
NFC and NFD remain different bytes. Money never uses floating point.

The exact digest preimage is:

```text
ASCII("CRAXII-DIGEST") || 0x00 || ASCII("1") || 0x00
|| ASCII(domain) || 0x00 || cjson1(material)
```

Domains are `work`, `cancel`, `result`, `model-invocation`,
`authenticated-request`, `execution-profile`, `compute-grant`,
`normalized-ingress`, `neutral-evidence`. Store version metadata separately;
SHA-256 is lowercase hex. Immutable inner material excludes current transport
session/generation/delivery authorization. Authenticated-request includes the
entire current envelope, authority and annotations, excluding separate proof/token.
Work hashes the complete WorkMaterial; cancel hashes CancelMaterial; result hashes
ResultSnapshot; model-invocation hashes ModelInvocationMaterial; profile/grant
hash their respective complete signed material. Neutral ingress/evidence hash their
complete named records. Receivers recompute asserted hashes.

Content SHA-256 hashes cjson1 of the complete content object; opaque/artifact hashes
hash exact bytes. Manifest per-file hashes hash exact file bytes. Bundle digest
hashes cjson1 of `{bundle_version,files}` sorted by path, excluding manifest itself.
These differ from semantic digests and existing native binary/internal operation
hashes. A digest supplies neither authentication, encryption nor anonymization.
Never log content or its digest as harmless diagnostics.

## Numeric, calendar and cryptographic representations

D0/D+ are canonical decimal strings, bounded by signed i64 max9223372036854775807;
D+ excludes zero. Full u64 max18446744073709551615 is a distinct diagnostic
primitive, never a central counter. Chat permits i64 minimum and excludes zero.
Reject signs except chat, negative zero, leading zeros, whitespace and exponents.
USD uses nonnegative integer microunits, 1000000/USD; billing minor units use scale2
and checked multiplication by10000. Rational category costs are
ceil(quantity*numerator/denominator), denominator positive, maximum32 categories,
checked u128 intermediate and i64 final sum. Negative, overflow or unpriced rejects.

UTC is exactly `YYYY-MM-DDTHH:MM:SS.ffffffZ`, years0001–9999, real Gregorian dates,
no leap seconds/offsets/24:00. Preserve six fractional digits; clocks do not supply
causal sequence. Named issuance/acceptance/observation clocks have separate owners.
Strictly future deadlines expire at equality. Explicit maximum TTL is required.

Base64url is canonical unpadded RFC4648 with decode/re-encode equality. Public
keys/nonces/session credentials are32 raw bytes, Ed25519 signatures64. Key ID is
SHA256(raw public key). Exact signing input is:

```text
ASCII("CRAXII-SIGNATURE") || 0x00 || ASCII("1") || 0x00
|| ASCII(purpose) || 0x00 || cjson1(signed_object)
```

Purposes: enrollment, session, sensitive_request, compute_authorization,
execution_profile. Sign exact bytes with Ed25519, without prehash. Signed objects
exclude their own signature. Enrollment signs expected instance evidence hashes
and selected protocol; session signs approved scopes/revocation epoch/selection;
sensitive proof signs nonce/current request/authority/audience/deadline. Grant and
profile accept only pinned central trust, never a payload-selected public key.
Nonces are fresh, bounded, atomically consumed; exact cached acknowledgement after
consumption requires authorized reconciliation. Static cloud documents alone are
not fresh eligibility proof. Public test seed/signatures grant no live trust.

## Durable work, cancellation and results

WorkMaterial contains stable customer/agent/work/conversation/ingress/sequence,
immutable reply binding, content and creation evidence, never installation/session,
entitlement period or delivery deadline. Dedupe excludes generation. Commit the
central work once, then locally map to distinct Rust identities and commit message,
FIFO trigger, journal and immutable receipt atomically before acknowledgement.
Same ID/material returns original receipt/current state, never another execution.
Changed digest/route/content rejects. Lost ack queries or redelivers same material
under current authority. A restored missing centrally acknowledged interval holds
for reconciliation; not_found alone cannot authorize rerun.

Cancellation has a stable control ID/digest and target resolved once. Persist a
pre-admission tombstone; queued cancellation is terminal, active cancellation only
requested until cleanup. Completion/cancellation linearize at the guarded local
transaction: completion wins preserves exact final; cancellation wins blocks final
assistant completion. Uncertain cleanup is interrupted, never fabricated cancelled.
All completed/failed/cancelled/interrupted states are absorbing. Crash does not
requeue active work or repeat tools.

Publish immutable content file bytes with no-replace/fsync before a transaction
references them. Terminal result, exact final snapshot, export ID/revision1/digests
and export obligation commit together. Postcommit retries resend the same snapshot
and upload identity; never reread a mutable workspace or regenerate an answer.
Completed answers require their local assistant message; truthful terminal notices
use state-specific reasons. Revision0/2 or changed content/lineage rejects.

Central acceptance validates current principal separately from origin receipt/
installation/generation and recorded recovery lineage. Commit one unique final
import plus one original-route outbox source before ack. Identical retries return
original acceptance/outbox IDs; conflicts create no new duty. Transfer is distinct
from Telegram send/acceptance/reading. Expired entitlement may allow narrow cleanup
export/receipt/cancel/status, never new chargeable work.

## External effects, power and accounting

Persist attempt before possible Telegram/cloud/model effect. Timeout, disconnect,
malformed success or post-intent crash remains outcome_unknown with same identity
and retained liability. Reconcile/query; never blind resend/reallocate. Definitely
not sent plus separately authorized retry may create a new physical attempt.
Earlier uncertain/permanent multipart part blocks subsequent parts. Telegram API
accepted is not read; provider silence is not confirmed failure.

Desired power, cloud power and recovered guest readiness are separate. Running,
enrolled, heartbeat or fixture-ready never implies ordinary readiness. Check current
boot/power/generation, exact mounts/recovery/service and qualified model. Drain
closes claims and proves no work/descendants/uncommitted/unacked state; central
revision recheck precedes stop. Ingress before stop invalidates drain, after dispatch
creates durable wake-after-stop. Unreachable is not idle. Snapshot completion is not
restore proof; destructive cleanup needs explicit operator scope and replay evidence.
Keys and session secrets never belong in persistent customer snapshots.

Trial fixed allowances: seven UTC calendar days,43200 running seconds,5000000 USD
micros,no card/auto-charge. Paid verified interval:4900 USD minor units,648000 running
seconds,20000000 USD micros. No automatic overages/daily cutoff/rollover. Missing
approved policy/current proof/consent denies real activation. Fixtures are synthetic
only and never production trust. Payments require verified provider evidence;
browser returns never grant periods. Failed renewal preserves paid-through access,
then denies; cancel-future-renewal is separate from early closure/refund.

Reserve when consumed+outstanding+proposed<=allowance. Each physical model attempt
has one reservation, grouping retries under a separate logical invocation. Same
attempt/digest never forwards twice. Unknown usage retains liability; worker counts
cannot settle. New period does not release or transfer old liabilities. Settlement
is unique/idempotent and retains its original price/period. Costs beyond reservation
are support incidents, never quiet overages.

Compute grant binds period/reservation/install/generation/boot/power/epoch/work,
absolute expiry and bounded max elapsed. Grant cannot exceed funded period/envelope;
expired/revoked or wrongboot denies new tools/model/wake while scoped cleanup remains.
Watchdog monotonic enforcement and actual cloud interval metering are separate future
integration responsibilities. Reconnect/clock rollback never extends authorization.
Revocation validates its requested epoch as D+, rejects a lower epoch with
`dispatch_disabled/authorization_epoch_stale/hold` and preserves the prior state.
An equal epoch closes a non-revoked charge gate; equal replay once revoked is a
no-op with hold disposition and no additional commit. A newer valid epoch revokes
and records the new epoch, including at i64 maximum. Revocation and its replay
never grant compute authority or extend an expired deadline.

Model streams have one start, consecutive same-attempt sequence, consistent item/
call bindings, exact assembled terminal arguments and one terminal event. Replay is
same-sequence dedupe only, no duplicate tool execution. Unknown output cannot forward
again. Semantic items preserve text/refusal/reasoning/tool/structured/opaque kinds;
unknown provider evidence never grants successful completion or tool authority.

Development fixture and qualified-candidate profiles bind exact acceptance account,
work/material/customer/install/generation/period/grant/config/price/tools/deadline.
They require normal identity, entitlement and reservations, are centrally signed,
never worker-selected, never grant ordinary readiness and reject in production.

## Errors and conformance boundaries

Safe errors reuse central AuthorityError roots with a closed reason/disposition.
No arbitrary message/stack/provider body or foreign identifier echo is allowed.
Duplicate receipt/import/reservation is success with original IDs. Unknown effects
use reconcile; stale authority rejects; expired authorization holds; pre-dispatch
backend failure may retry_same_identity; terminal cancellation never revives work.
Errors cannot promise a failed effect from a transport503 or lost commit response.

Fixtures assert initial state, synthetic context, event, exact next projection,
required commits, forbidden effects and disposition. They are pure deterministic
specification models, not evidence of actual SQL/SQLite transactions, authentication,
nonce consumption, provider acceptance, concurrency, isolation, shutdown or physical
restore. Owning integrations must prove those facts with actual components and
fault/recovery tests before activation. Contract libraries mount no business routes.
