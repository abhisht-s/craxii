# Stage 20 live event delivery decision

Status: accepted for V0.0.01 Stage 20.

Assistant drafts are ephemeral presentation data, never canonical state, journal records,
projection input, bootstrap content, or reconnect recovery state. The production
`ModelGateway` offers only provider-neutral text and refusal deltas to one bounded in-process
broker. Reasoning, tool formation and arguments, structured output, provider wire data, opaque
continuation state, usage, and raw errors are not public draft material.

Each physical model invocation has at most one `draft_id`. The first safe delta produces
`assistant.draft_started` followed by `assistant.draft_delta`; sequences begin at one and may
have gaps because deltas are lossy. A visible draft may end with
`assistant.draft_abandoned` using only the safe reasons `tool_continuation`, `superseded`,
`cancelled`, `failed`, `interrupted`, or `delivery_limit`. There is no draft finalized,
replace, or reset event.

The existing authenticated `GET /v1/events?after=<cursor>` WebSocket remains the only live
route. A connection subscribes to drafts only after durable catch-up and successful
`sync.complete`; no draft snapshot or replay exists. Disconnect drops connection-local draft
state. Restart drops the broker and all drafts. A newly connected client cannot join a draft
already in progress.

The durable `assistant.message_committed` event is the only final answer authority and causes
all drafts for that Work to be discarded. This remains correct if the process fails after the
assistant transaction commits but before ephemeral cleanup. Tool continuation abandons mixed
preliminary text before tool execution, and the next physical invocation uses a new draft.
Durable redacted tool start/finish projections remain the source of high-level tool progress.

The broker never awaits socket work. Each eligible connection has an independent 16-frame
ephemeral queue. Adjacent unsent same-draft deltas may coalesce, safe deltas may drop, and
structural start/abandon frames evict deltas. If structural delivery still cannot fit, only that
client closes with retryable WebSocket code 1013. Durable SQLite replay remains independent,
lossless-or-disconnect, and higher priority. Drafts are limited to 256 KiB and 4,096 deltas per
invocation; encoded frames retain the existing 270,336-byte ceiling.

The additive protocol remains version 1 and is sufficient for Stage 21 transport/projection
work without implementing a native client. Stage 20 adds no migration and no dependency.
