//! Bounded in-process fan-out for provider-neutral, noncanonical live drafts.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::application::model_gateway::{
    CanonicalDraftDelta, DraftAbandonCause, DraftExposure, DraftIdentity, DraftSink,
};
use crate::domain::{DraftId, ModelInvocationId, WorkId};
use crate::protocol::{
    DraftAbandonReason, DraftDeltaKind, DraftEventPayload, EphemeralDraftEnvelope,
    MAX_DRAFT_EVENTS, MAX_DRAFT_TEXT_BYTES, MAX_WEBSOCKET_FRAME_BYTES, WEBSOCKET_OUTBOUND_FRAMES,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LiveEventMetricsSnapshot {
    pub active_connections: u64,
    pub accepted_connections: u64,
    pub disconnected_connections: u64,
    pub replay_connections: u64,
    pub replayed_durable_events: u64,
    pub maximum_replay_lag: u64,
    pub drafts_started: u64,
    pub draft_deltas: u64,
    pub drafts_abandoned: u64,
    pub abandoned_tool_continuation: u64,
    pub abandoned_superseded: u64,
    pub abandoned_cancelled: u64,
    pub abandoned_failed: u64,
    pub abandoned_interrupted: u64,
    pub abandoned_delivery_limit: u64,
    pub coalesced_deltas: u64,
    pub dropped_deltas: u64,
    pub queue_high_water: u64,
    pub slow_client_disconnects: u64,
}

#[derive(Default)]
struct LiveEventMetrics {
    active_connections: AtomicU64,
    accepted_connections: AtomicU64,
    disconnected_connections: AtomicU64,
    replay_connections: AtomicU64,
    replayed_durable_events: AtomicU64,
    maximum_replay_lag: AtomicU64,
    drafts_started: AtomicU64,
    draft_deltas: AtomicU64,
    drafts_abandoned: AtomicU64,
    abandoned_tool_continuation: AtomicU64,
    abandoned_superseded: AtomicU64,
    abandoned_cancelled: AtomicU64,
    abandoned_failed: AtomicU64,
    abandoned_interrupted: AtomicU64,
    abandoned_delivery_limit: AtomicU64,
    coalesced_deltas: AtomicU64,
    dropped_deltas: AtomicU64,
    queue_high_water: AtomicU64,
    slow_client_disconnects: AtomicU64,
}

struct ActiveDraft {
    identity: DraftIdentity,
    next_sequence: u32,
    text_bytes: usize,
    delta_events: u32,
    started: bool,
}

struct Subscriber {
    queue: VecDeque<EphemeralDraftEnvelope>,
    eligible_drafts: HashMap<DraftId, u32>,
    notify: Arc<tokio::sync::Notify>,
    overloaded: bool,
}

struct BrokerState {
    accepting: bool,
    next_subscriber_id: u64,
    drafts: HashMap<ModelInvocationId, ActiveDraft>,
    subscribers: HashMap<u64, Subscriber>,
}

struct BrokerInner {
    state: Mutex<BrokerState>,
    closed: AtomicBool,
    metrics: LiveEventMetrics,
}

/// Application-level broker. It owns no task and has no persistence authority.
#[derive(Clone)]
pub struct LiveEventBroker {
    inner: Arc<BrokerInner>,
}

impl Default for LiveEventBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveEventBroker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BrokerInner {
                state: Mutex::new(BrokerState {
                    accepting: true,
                    next_subscriber_id: 1,
                    drafts: HashMap::new(),
                    subscribers: HashMap::new(),
                }),
                closed: AtomicBool::new(false),
                metrics: LiveEventMetrics::default(),
            }),
        }
    }

    /// Registers only a connection that has already received `sync.complete`.
    #[must_use]
    pub fn subscribe(&self) -> Option<LiveEventSubscription> {
        let mut state = self.lock_state();
        if !state.accepting {
            return None;
        }
        let id = state.next_subscriber_id;
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);
        let notify = Arc::new(tokio::sync::Notify::new());
        state.subscribers.insert(
            id,
            Subscriber {
                queue: VecDeque::with_capacity(WEBSOCKET_OUTBOUND_FRAMES),
                eligible_drafts: HashMap::new(),
                notify: Arc::clone(&notify),
                overloaded: false,
            },
        );
        self.inner
            .metrics
            .active_connections
            .fetch_add(1, Ordering::AcqRel);
        self.inner
            .metrics
            .accepted_connections
            .fetch_add(1, Ordering::Relaxed);
        Some(LiveEventSubscription {
            id,
            broker: self.clone(),
            notify,
        })
    }

    /// Stops new drafts/subscriptions and wakes every live connection for owned shutdown.
    pub fn close_admission(&self) {
        self.inner.closed.store(true, Ordering::Release);
        let mut state = self.lock_state();
        state.accepting = false;
        state.drafts.clear();
        for subscriber in state.subscribers.values_mut() {
            subscriber.eligible_drafts.clear();
            subscriber.queue.clear();
            subscriber.notify.notify_one();
        }
    }

    /// Removes presentation state after any authoritative terminal Work fact.
    pub fn finalize_work(&self, work_id: WorkId) {
        let mut state = self.lock_state();
        let draft_ids = state
            .drafts
            .values()
            .filter(|draft| draft.identity.work_id == work_id)
            .map(|draft| draft.identity.draft_id)
            .collect::<Vec<_>>();
        state
            .drafts
            .retain(|_, draft| draft.identity.work_id != work_id);
        for subscriber in state.subscribers.values_mut() {
            subscriber.queue.retain(|event| event.work_id != work_id);
            for draft_id in &draft_ids {
                subscriber.eligible_drafts.remove(draft_id);
            }
        }
    }

    pub fn observe_replay(&self, replayed_events: u64, lag: u64) {
        self.inner
            .metrics
            .replay_connections
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .metrics
            .replayed_durable_events
            .fetch_add(replayed_events, Ordering::Relaxed);
        self.inner
            .metrics
            .maximum_replay_lag
            .fetch_max(lag, Ordering::Relaxed);
    }

    pub fn observe_slow_disconnect(&self) {
        self.inner
            .metrics
            .slow_client_disconnects
            .fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn metrics(&self) -> LiveEventMetricsSnapshot {
        let metrics = &self.inner.metrics;
        LiveEventMetricsSnapshot {
            active_connections: metrics.active_connections.load(Ordering::Acquire),
            accepted_connections: metrics.accepted_connections.load(Ordering::Relaxed),
            disconnected_connections: metrics.disconnected_connections.load(Ordering::Relaxed),
            replay_connections: metrics.replay_connections.load(Ordering::Relaxed),
            replayed_durable_events: metrics.replayed_durable_events.load(Ordering::Relaxed),
            maximum_replay_lag: metrics.maximum_replay_lag.load(Ordering::Relaxed),
            drafts_started: metrics.drafts_started.load(Ordering::Relaxed),
            draft_deltas: metrics.draft_deltas.load(Ordering::Relaxed),
            drafts_abandoned: metrics.drafts_abandoned.load(Ordering::Relaxed),
            abandoned_tool_continuation: metrics
                .abandoned_tool_continuation
                .load(Ordering::Relaxed),
            abandoned_superseded: metrics.abandoned_superseded.load(Ordering::Relaxed),
            abandoned_cancelled: metrics.abandoned_cancelled.load(Ordering::Relaxed),
            abandoned_failed: metrics.abandoned_failed.load(Ordering::Relaxed),
            abandoned_interrupted: metrics.abandoned_interrupted.load(Ordering::Relaxed),
            abandoned_delivery_limit: metrics.abandoned_delivery_limit.load(Ordering::Relaxed),
            coalesced_deltas: metrics.coalesced_deltas.load(Ordering::Relaxed),
            dropped_deltas: metrics.dropped_deltas.load(Ordering::Relaxed),
            queue_high_water: metrics.queue_high_water.load(Ordering::Relaxed),
            slow_client_disconnects: metrics.slow_client_disconnects.load(Ordering::Relaxed),
        }
    }

    fn offer(&self, identity: DraftIdentity, delta: CanonicalDraftDelta) -> DraftExposure {
        if self.inner.closed.load(Ordering::Acquire) {
            return DraftExposure::NotExposed;
        }
        let (kind, text) = match delta {
            CanonicalDraftDelta::Text { text } => (DraftDeltaKind::Text, text),
            CanonicalDraftDelta::Refusal { text } => (DraftDeltaKind::Refusal, text),
        };
        let text_bytes = text.len();
        let mut state = self.lock_state();
        if !state.accepting {
            return DraftExposure::NotExposed;
        }

        let superseded = state
            .drafts
            .iter()
            .filter(|(invocation_id, draft)| {
                draft.identity.work_id == identity.work_id
                    && **invocation_id != identity.invocation_id
            })
            .map(|(invocation_id, _)| *invocation_id)
            .collect::<Vec<_>>();
        for invocation_id in superseded {
            if let Some(draft) = state.drafts.remove(&invocation_id) {
                self.broadcast_abandoned_draft(&mut state, draft, DraftAbandonCause::Superseded);
            }
        }

        let draft = state
            .drafts
            .entry(identity.invocation_id)
            .or_insert_with(|| ActiveDraft {
                identity,
                next_sequence: 0,
                text_bytes: 0,
                delta_events: 0,
                started: false,
            });
        if draft.identity != identity || draft.next_sequence == u32::MAX {
            return DraftExposure::Exposed;
        }
        draft.next_sequence += 1;
        let sequence = draft.next_sequence;
        let over_limit = draft
            .text_bytes
            .checked_add(text_bytes)
            .is_none_or(|bytes| bytes > MAX_DRAFT_TEXT_BYTES)
            || draft.delta_events >= MAX_DRAFT_EVENTS;
        if over_limit {
            let identity = draft.identity;
            let was_started = draft.started;
            if !was_started {
                draft.started = true;
            }
            self.broadcast_started_and_abandoned(
                &mut state,
                identity,
                !was_started,
                DraftAbandonCause::DeliveryLimit,
            );
            state.drafts.remove(&identity.invocation_id);
            return DraftExposure::Exposed;
        }
        draft.text_bytes += text_bytes;
        draft.delta_events += 1;
        let identity = draft.identity;
        let first = !draft.started;
        draft.started = true;
        let delta = EphemeralDraftEnvelope::delta(
            identity.conversation_id,
            identity.work_id,
            identity.invocation_id,
            identity.draft_id,
            sequence,
            kind,
            text,
        );
        if match serde_json::to_vec(&delta) {
            Ok(encoded) => encoded.len() > MAX_WEBSOCKET_FRAME_BYTES,
            Err(_) => true,
        } {
            self.broadcast_started_and_abandoned(
                &mut state,
                identity,
                first,
                DraftAbandonCause::DeliveryLimit,
            );
            state.drafts.remove(&identity.invocation_id);
            return DraftExposure::Exposed;
        }
        if first {
            self.inner
                .metrics
                .drafts_started
                .fetch_add(1, Ordering::Relaxed);
        }
        self.inner
            .metrics
            .draft_deltas
            .fetch_add(1, Ordering::Relaxed);
        for subscriber in state.subscribers.values_mut() {
            if first {
                let started = EphemeralDraftEnvelope::started(
                    identity.conversation_id,
                    identity.work_id,
                    identity.invocation_id,
                    identity.draft_id,
                );
                if enqueue_structural(subscriber, started, &self.inner.metrics) {
                    subscriber.eligible_drafts.insert(identity.draft_id, 0);
                }
            }
            let Some(last_sequence) = subscriber.eligible_drafts.get_mut(&identity.draft_id) else {
                continue;
            };
            if sequence <= *last_sequence {
                continue;
            }
            *last_sequence = sequence;
            enqueue_delta(subscriber, delta.clone(), &self.inner.metrics);
        }
        DraftExposure::Exposed
    }

    fn abandon(&self, invocation_id: ModelInvocationId, reason: DraftAbandonCause) {
        let mut state = self.lock_state();
        let Some(draft) = state.drafts.remove(&invocation_id) else {
            return;
        };
        self.broadcast_abandoned_draft(&mut state, draft, reason);
    }

    fn broadcast_abandoned_draft(
        &self,
        state: &mut BrokerState,
        draft: ActiveDraft,
        reason: DraftAbandonCause,
    ) {
        if !draft.started {
            return;
        }
        observe_abandon(&self.inner.metrics, reason);
        let event = EphemeralDraftEnvelope::abandoned(
            draft.identity.conversation_id,
            draft.identity.work_id,
            draft.identity.invocation_id,
            draft.identity.draft_id,
            public_abandon_reason(reason),
        );
        for subscriber in state.subscribers.values_mut() {
            if subscriber
                .eligible_drafts
                .remove(&draft.identity.draft_id)
                .is_some()
            {
                enqueue_structural(subscriber, event.clone(), &self.inner.metrics);
            }
        }
    }

    fn broadcast_started_and_abandoned(
        &self,
        state: &mut BrokerState,
        identity: DraftIdentity,
        send_started: bool,
        reason: DraftAbandonCause,
    ) {
        if send_started {
            self.inner
                .metrics
                .drafts_started
                .fetch_add(1, Ordering::Relaxed);
        }
        observe_abandon(&self.inner.metrics, reason);
        let started = EphemeralDraftEnvelope::started(
            identity.conversation_id,
            identity.work_id,
            identity.invocation_id,
            identity.draft_id,
        );
        let abandoned = EphemeralDraftEnvelope::abandoned(
            identity.conversation_id,
            identity.work_id,
            identity.invocation_id,
            identity.draft_id,
            public_abandon_reason(reason),
        );
        for subscriber in state.subscribers.values_mut() {
            if send_started && enqueue_structural(subscriber, started.clone(), &self.inner.metrics)
            {
                subscriber.eligible_drafts.insert(identity.draft_id, 0);
            }
            if subscriber
                .eligible_drafts
                .remove(&identity.draft_id)
                .is_some()
            {
                enqueue_structural(subscriber, abandoned.clone(), &self.inner.metrics);
            }
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BrokerState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl DraftSink for LiveEventBroker {
    fn offer(&self, identity: DraftIdentity, delta: CanonicalDraftDelta) -> DraftExposure {
        self.offer(identity, delta)
    }

    fn abandon(&self, invocation_id: ModelInvocationId, reason: DraftAbandonCause) {
        self.abandon(invocation_id, reason);
    }

    fn finalize_work(&self, work_id: WorkId) {
        self.finalize_work(work_id);
    }
}

const fn public_abandon_reason(reason: DraftAbandonCause) -> DraftAbandonReason {
    match reason {
        DraftAbandonCause::ToolContinuation => DraftAbandonReason::ToolContinuation,
        DraftAbandonCause::Superseded => DraftAbandonReason::Superseded,
        DraftAbandonCause::Cancelled => DraftAbandonReason::Cancelled,
        DraftAbandonCause::Failed => DraftAbandonReason::Failed,
        DraftAbandonCause::Interrupted => DraftAbandonReason::Interrupted,
        DraftAbandonCause::DeliveryLimit => DraftAbandonReason::DeliveryLimit,
    }
}

fn enqueue_delta(
    subscriber: &mut Subscriber,
    event: EphemeralDraftEnvelope,
    metrics: &LiveEventMetrics,
) {
    if subscriber.queue.len() < WEBSOCKET_OUTBOUND_FRAMES {
        subscriber.queue.push_back(event);
        update_queue_high_water(subscriber.queue.len(), metrics);
        subscriber.notify.notify_one();
        return;
    }
    if let Some(last) = subscriber.queue.back_mut()
        && last.draft_id == event.draft_id
        && let (
            DraftEventPayload::Delta {
                kind: last_kind,
                text: last_text,
            },
            DraftEventPayload::Delta {
                kind: next_kind,
                text: next_text,
            },
        ) = (&last.payload, &event.payload)
        && last_kind == next_kind
    {
        let mut candidate = last.clone();
        let DraftEventPayload::Delta {
            text: candidate_text,
            ..
        } = &mut candidate.payload
        else {
            unreachable!("matched delta payload above");
        };
        candidate_text.reserve(last_text.len().saturating_add(next_text.len()));
        candidate_text.push_str(next_text);
        candidate.delta_sequence = event.delta_sequence;
        candidate.event_id = event.event_id;
        if serde_json::to_vec(&candidate)
            .is_ok_and(|encoded| encoded.len() <= MAX_WEBSOCKET_FRAME_BYTES)
        {
            *last = candidate;
            metrics.coalesced_deltas.fetch_add(1, Ordering::Relaxed);
            subscriber.notify.notify_one();
            return;
        }
    }
    metrics.dropped_deltas.fetch_add(1, Ordering::Relaxed);
}

fn enqueue_structural(
    subscriber: &mut Subscriber,
    event: EphemeralDraftEnvelope,
    metrics: &LiveEventMetrics,
) -> bool {
    while subscriber.queue.len() >= WEBSOCKET_OUTBOUND_FRAMES {
        let Some(position) = subscriber.queue.iter().position(|queued| queued.is_delta()) else {
            subscriber.overloaded = true;
            subscriber.notify.notify_one();
            return false;
        };
        subscriber.queue.remove(position);
        metrics.dropped_deltas.fetch_add(1, Ordering::Relaxed);
    }
    subscriber.queue.push_back(event);
    update_queue_high_water(subscriber.queue.len(), metrics);
    subscriber.notify.notify_one();
    true
}

fn update_queue_high_water(depth: usize, metrics: &LiveEventMetrics) {
    metrics
        .queue_high_water
        .fetch_max(u64::try_from(depth).unwrap_or(u64::MAX), Ordering::Relaxed);
}

fn observe_abandon(metrics: &LiveEventMetrics, reason: DraftAbandonCause) {
    metrics.drafts_abandoned.fetch_add(1, Ordering::Relaxed);
    let reason_counter = match reason {
        DraftAbandonCause::ToolContinuation => &metrics.abandoned_tool_continuation,
        DraftAbandonCause::Superseded => &metrics.abandoned_superseded,
        DraftAbandonCause::Cancelled => &metrics.abandoned_cancelled,
        DraftAbandonCause::Failed => &metrics.abandoned_failed,
        DraftAbandonCause::Interrupted => &metrics.abandoned_interrupted,
        DraftAbandonCause::DeliveryLimit => &metrics.abandoned_delivery_limit,
    };
    reason_counter.fetch_add(1, Ordering::Relaxed);
}

pub enum LiveEventReceive {
    Event(EphemeralDraftEnvelope),
    Overloaded,
    Closed,
}

/// Connection-owned handle. Dropping it removes all connection-local draft state.
pub struct LiveEventSubscription {
    id: u64,
    broker: LiveEventBroker,
    notify: Arc<tokio::sync::Notify>,
}

impl LiveEventSubscription {
    pub async fn recv(&mut self) -> LiveEventReceive {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.broker.lock_state();
                let Some(subscriber) = state.subscribers.get_mut(&self.id) else {
                    return LiveEventReceive::Closed;
                };
                if subscriber.overloaded {
                    return LiveEventReceive::Overloaded;
                }
                if let Some(event) = subscriber.queue.pop_front() {
                    return LiveEventReceive::Event(event);
                }
                if !state.accepting {
                    return LiveEventReceive::Closed;
                }
            }
            notified.await;
        }
    }
}

impl Drop for LiveEventSubscription {
    fn drop(&mut self) {
        let mut state = self.broker.lock_state();
        if state.subscribers.remove(&self.id).is_some() {
            self.broker
                .inner
                .metrics
                .active_connections
                .fetch_sub(1, Ordering::AcqRel);
            self.broker
                .inner
                .metrics
                .disconnected_connections
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ConversationId, DraftId, ModelInvocationId, WorkId};

    fn identity() -> DraftIdentity {
        DraftIdentity {
            conversation_id: ConversationId::generate(),
            work_id: WorkId::generate(),
            invocation_id: ModelInvocationId::generate(),
            draft_id: DraftId::generate(),
        }
    }

    #[tokio::test]
    async fn one_invocation_has_started_ordered_delta_and_safe_abandonment() {
        let broker = LiveEventBroker::new();
        let mut subscriber = broker.subscribe().unwrap();
        let identity = identity();
        assert_eq!(
            DraftSink::offer(
                &broker,
                identity,
                CanonicalDraftDelta::Text {
                    text: "hello".to_owned(),
                },
            ),
            DraftExposure::Exposed
        );
        let LiveEventReceive::Event(started) = subscriber.recv().await else {
            panic!("started event expected");
        };
        assert_eq!(started.event_type, "assistant.draft_started");
        let LiveEventReceive::Event(delta) = subscriber.recv().await else {
            panic!("delta event expected");
        };
        assert_eq!(delta.delta_sequence, Some(1));
        DraftSink::abandon(&broker, identity.invocation_id, DraftAbandonCause::Failed);
        let LiveEventReceive::Event(abandoned) = subscriber.recv().await else {
            panic!("abandon event expected");
        };
        assert_eq!(abandoned.event_type, "assistant.draft_abandoned");
        assert_eq!(broker.metrics().drafts_abandoned, 1);
    }

    #[tokio::test]
    async fn subscriber_joining_mid_draft_gets_no_snapshot_or_later_delta() {
        let broker = LiveEventBroker::new();
        let identity = identity();
        assert_eq!(
            DraftSink::offer(
                &broker,
                identity,
                CanonicalDraftDelta::Text {
                    text: "before sync".to_owned(),
                },
            ),
            DraftExposure::Exposed
        );
        let mut late = broker.subscribe().unwrap();
        assert_eq!(
            DraftSink::offer(
                &broker,
                identity,
                CanonicalDraftDelta::Text {
                    text: "after sync".to_owned(),
                },
            ),
            DraftExposure::Exposed
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), late.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn pressure_is_bounded_and_structural_events_evict_deltas() {
        let broker = LiveEventBroker::new();
        let mut subscriber = broker.subscribe().unwrap();
        let identity = identity();
        for index in 0..64 {
            assert_eq!(
                DraftSink::offer(
                    &broker,
                    identity,
                    CanonicalDraftDelta::Text {
                        text: format!("{index},"),
                    },
                ),
                DraftExposure::Exposed
            );
        }
        DraftSink::abandon(
            &broker,
            identity.invocation_id,
            DraftAbandonCause::Cancelled,
        );
        let mut saw_started = false;
        let mut saw_abandoned = false;
        for _ in 0..WEBSOCKET_OUTBOUND_FRAMES {
            match subscriber.recv().await {
                LiveEventReceive::Event(event) => {
                    saw_started |= event.event_type == "assistant.draft_started";
                    saw_abandoned |= event.event_type == "assistant.draft_abandoned";
                    if saw_started && saw_abandoned {
                        break;
                    }
                }
                LiveEventReceive::Overloaded | LiveEventReceive::Closed => break,
            }
        }
        assert!(saw_started && saw_abandoned);
        let metrics = broker.metrics();
        assert!(metrics.queue_high_water <= WEBSOCKET_OUTBOUND_FRAMES as u64);
        assert!(metrics.coalesced_deltas + metrics.dropped_deltas > 0);
    }

    #[tokio::test]
    async fn finalization_and_disconnect_discard_all_ephemeral_state() {
        let broker = LiveEventBroker::new();
        let subscriber = broker.subscribe().unwrap();
        let identity = identity();
        DraftSink::offer(
            &broker,
            identity,
            CanonicalDraftDelta::Refusal {
                text: "cannot comply".to_owned(),
            },
        );
        broker.finalize_work(identity.work_id);
        drop(subscriber);
        let mut reconnect = broker.subscribe().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), reconnect.recv())
                .await
                .is_err()
        );
        let metrics = broker.metrics();
        assert_eq!(metrics.active_connections, 1);
        assert_eq!(metrics.disconnected_connections, 1);
    }

    #[test]
    fn production_sink_is_conservatively_exposed_without_a_live_client() {
        let broker = LiveEventBroker::new();
        assert_eq!(
            DraftSink::offer(
                &broker,
                identity(),
                CanonicalDraftDelta::Text {
                    text: "accepted without subscriber".to_owned(),
                },
            ),
            DraftExposure::Exposed
        );
        broker.close_admission();
        assert_eq!(
            DraftSink::offer(
                &broker,
                identity(),
                CanonicalDraftDelta::Text {
                    text: "after shutdown".to_owned(),
                },
            ),
            DraftExposure::NotExposed
        );
    }

    #[tokio::test]
    async fn fast_and_slow_subscribers_are_pressure_isolated() {
        let broker = LiveEventBroker::new();
        let mut fast = broker.subscribe().unwrap();
        let mut slow = broker.subscribe().unwrap();
        let identity = identity();
        DraftSink::offer(
            &broker,
            identity,
            CanonicalDraftDelta::Text {
                text: "0".to_owned(),
            },
        );
        assert!(matches!(fast.recv().await, LiveEventReceive::Event(_)));
        assert!(matches!(fast.recv().await, LiveEventReceive::Event(_)));
        for index in 1..64 {
            DraftSink::offer(
                &broker,
                identity,
                CanonicalDraftDelta::Text {
                    text: index.to_string(),
                },
            );
            let LiveEventReceive::Event(event) = fast.recv().await else {
                panic!("fast subscriber must remain live");
            };
            assert_eq!(event.event_type, "assistant.draft_delta");
        }
        DraftSink::abandon(
            &broker,
            identity.invocation_id,
            DraftAbandonCause::Superseded,
        );
        let LiveEventReceive::Event(fast_abandon) = fast.recv().await else {
            panic!("fast subscriber abandonment expected");
        };
        assert_eq!(fast_abandon.event_type, "assistant.draft_abandoned");
        let mut slow_frames = 0;
        while let Ok(LiveEventReceive::Event(_)) =
            tokio::time::timeout(std::time::Duration::from_millis(10), slow.recv()).await
        {
            slow_frames += 1;
        }
        assert!(slow_frames <= WEBSOCKET_OUTBOUND_FRAMES);
        assert!(broker.metrics().coalesced_deltas + broker.metrics().dropped_deltas > 0);
    }

    #[tokio::test]
    async fn structural_pressure_marks_only_that_connection_overloaded() {
        let broker = LiveEventBroker::new();
        let mut subscriber = broker.subscribe().unwrap();
        for _ in 0..=WEBSOCKET_OUTBOUND_FRAMES {
            DraftSink::offer(
                &broker,
                identity(),
                CanonicalDraftDelta::Text {
                    text: "x".to_owned(),
                },
            );
        }
        assert!(matches!(
            subscriber.recv().await,
            LiveEventReceive::Overloaded
        ));
    }
}
