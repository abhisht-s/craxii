//! Client-safe bootstrap and durable replay projection.

use std::collections::HashMap;
use std::fmt;

use serde_json::json;

use crate::domain::{
    JournalEvent, JournalEventPayload, JournalWorkTerminalReason, ToolExecutionState, WorkId,
    WorkState,
};
use crate::ports::state_store::{
    ClientBootstrapCandidate, ListPublicJournalRequest, PublicJournalPage, ReplayStateStore,
    StateStoreError, StateStoreErrorKind,
};
use crate::protocol::{
    BootstrapResponse, DeliveryKind, DurableEventEnvelope, MAX_BOOTSTRAP_JSON_BYTES,
    MAX_BOOTSTRAP_MESSAGES, MAX_BOOTSTRAP_SOURCE_MESSAGE_JSON_BYTES, MAX_BOOTSTRAP_TOOL_SUMMARIES,
    MAX_DURABLE_PAYLOAD_BYTES, MAX_WEBSOCKET_FRAME_BYTES, ProtocolVersion, PublicContentBlock,
    PublicConversation, PublicCraxii, PublicMessage, PublicToolSummary, PublicWorkItem,
    REPLAY_PAGE_ROWS, ReplayCursor, UnresolvedOutcome, UnresolvedOutcomeKind,
};

pub struct PublicStateService<'a, S> {
    store: &'a S,
}

impl<'a, S> PublicStateService<'a, S>
where
    S: ReplayStateStore,
{
    #[must_use]
    pub const fn new(store: &'a S) -> Self {
        Self { store }
    }

    pub async fn current_high_water(&self) -> Result<ReplayCursor, PublicationError> {
        self.store
            .current_journal_high_water()
            .await
            .map(ReplayCursor::from_journal_offset)
            .map_err(PublicationError::from_store)
    }

    pub async fn bootstrap(&self) -> Result<BootstrapResponse, PublicationError> {
        let source = self
            .store
            .load_client_bootstrap_snapshot()
            .await
            .map_err(PublicationError::from_store)?;
        project_bootstrap(source)
    }

    pub async fn replay_page(
        &self,
        after: ReplayCursor,
        through: ReplayCursor,
    ) -> Result<PublicReplayPage, PublicationError> {
        let through_offset = through
            .as_journal_offset()
            .ok_or_else(PublicationError::invariant)?;
        if after > through {
            return Err(PublicationError::invariant());
        }
        let page = self
            .store
            .list_public_journal_replay_candidates(ListPublicJournalRequest {
                after: after.as_journal_offset(),
                through: through_offset,
                limit: REPLAY_PAGE_ROWS,
            })
            .await
            .map_err(PublicationError::from_store)?;
        project_replay_page(after, through, page)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublicReplayPage {
    pub events: Vec<DurableEventEnvelope>,
    pub scanned_through: ReplayCursor,
    pub has_more: bool,
}

fn project_bootstrap(
    source: ClientBootstrapCandidate,
) -> Result<BootstrapResponse, PublicationError> {
    if source.messages.len() > MAX_BOOTSTRAP_MESSAGES
        || source.tool_summaries.len() > MAX_BOOTSTRAP_TOOL_SUMMARIES
        || source.source_message_json_bytes > MAX_BOOTSTRAP_SOURCE_MESSAGE_JSON_BYTES
    {
        return Err(PublicationError::limit());
    }

    let mut tools_by_work: HashMap<WorkId, Vec<PublicToolSummary>> = HashMap::new();
    let mut cleanup_by_work: HashMap<WorkId, bool> = HashMap::new();
    let mut unresolved = Vec::new();
    for tool in source.tool_summaries {
        let outcome_unknown = tool.state == ToolExecutionState::OutcomeUnknown;
        let cleanup_pending = outcome_unknown || tool.cleanup_confirmed == Some(false);
        cleanup_by_work
            .entry(tool.work_id)
            .and_modify(|pending| *pending |= cleanup_pending)
            .or_insert(cleanup_pending);
        if outcome_unknown {
            unresolved.push((
                tool.work_ordinal,
                tool.tool_ordinal,
                UnresolvedOutcome {
                    kind: UnresolvedOutcomeKind::ToolOutcomeUnknown,
                    work_id: tool.work_id,
                    tool_execution_id: Some(tool.tool_execution_id),
                },
            ));
        }
        if cleanup_pending {
            unresolved.push((
                tool.work_ordinal,
                tool.tool_ordinal,
                UnresolvedOutcome {
                    kind: UnresolvedOutcomeKind::CleanupUnconfirmed,
                    work_id: tool.work_id,
                    tool_execution_id: Some(tool.tool_execution_id),
                },
            ));
        }
        tools_by_work
            .entry(tool.work_id)
            .or_default()
            .push(PublicToolSummary {
                tool_execution_id: tool.tool_execution_id,
                tool_name: tool.tool_name.as_str().to_owned(),
                status: tool.state.as_str().to_owned(),
                result_class: tool.result_class.map(|value| value.as_str().to_owned()),
                requested_at: tool.requested_at,
                started_at: tool.started_at,
                finished_at: tool.completed_at,
                outcome_unknown,
            });
    }

    let mut work_items = Vec::with_capacity(source.work_items.len());
    for work in source.work_items {
        if work.terminal_reason == Some(JournalWorkTerminalReason::ProviderOutcomeUnknown) {
            unresolved.push((
                work.conversation_work_ordinal,
                crate::domain::ToolOrdinal::try_new(1)
                    .map_err(|_| PublicationError::invariant())?,
                UnresolvedOutcome {
                    kind: UnresolvedOutcomeKind::ProviderOutcomeUnknown,
                    work_id: work.work_id,
                    tool_execution_id: None,
                },
            ));
        }
        if work.terminal_reason == Some(JournalWorkTerminalReason::CleanupUnconfirmed) {
            unresolved.push((
                work.conversation_work_ordinal,
                crate::domain::ToolOrdinal::try_new(1)
                    .map_err(|_| PublicationError::invariant())?,
                UnresolvedOutcome {
                    kind: UnresolvedOutcomeKind::CleanupUnconfirmed,
                    work_id: work.work_id,
                    tool_execution_id: None,
                },
            ));
        }
        let cleanup_pending = work.state == WorkState::CancelRequested
            || work.terminal_reason == Some(JournalWorkTerminalReason::CleanupUnconfirmed)
            || cleanup_by_work.get(&work.work_id).copied().unwrap_or(false);
        work_items.push(PublicWorkItem {
            work_id: work.work_id,
            conversation_id: work.conversation_id,
            conversation_work_ordinal: work.conversation_work_ordinal,
            state: work.state,
            trigger_message_id: work.trigger_message_id,
            created_at: work.created_at,
            queued_at: work.queued_at,
            started_at: work.started_at,
            cancel_requested_at: work.cancel_requested_at,
            terminal_at: work.terminal_at,
            terminal_reason: work.terminal_reason,
            cleanup_pending,
            tool_summaries: tools_by_work.remove(&work.work_id).unwrap_or_default(),
        });
    }
    if !tools_by_work.is_empty() {
        return Err(PublicationError::invariant());
    }
    unresolved.sort_by_key(|(work_ordinal, tool_ordinal, _)| (*work_ordinal, *tool_ordinal));

    let response = BootstrapResponse {
        protocol_version: ProtocolVersion,
        snapshot_cursor: source.snapshot_cursor,
        craxii: PublicCraxii {
            craxii_id: source.principal.craxii_id(),
            display_name: source.principal.display_name().to_owned(),
            owner_label: source.principal.owner_label().to_owned(),
        },
        primary_conversation: PublicConversation {
            conversation_id: source.primary_conversation.conversation_id(),
            kind: "primary",
            lifecycle: source.primary_conversation.lifecycle(),
            created_at: source.primary_conversation.created_at(),
        },
        messages: source
            .messages
            .into_iter()
            .map(|candidate| PublicMessage {
                message_id: candidate.message.message_id(),
                conversation_id: candidate.message.conversation_id(),
                conversation_sequence: candidate.conversation_sequence,
                role: candidate.message.role(),
                content: candidate
                    .message
                    .content()
                    .blocks()
                    .iter()
                    .map(PublicContentBlock::from_domain)
                    .collect(),
                client_message_id: candidate.message.client_message_id(),
                work_id: candidate.message.produced_by_work_id(),
                committed_at: candidate.message.committed_at(),
            })
            .collect(),
        work_items,
        unresolved_outcomes: unresolved
            .into_iter()
            .map(|(_, _, warning)| warning)
            .collect(),
    };
    let encoded = serde_json::to_vec(&response).map_err(|_| PublicationError::invariant())?;
    if encoded.len() > MAX_BOOTSTRAP_JSON_BYTES {
        return Err(PublicationError::limit());
    }
    Ok(response)
}

fn project_replay_page(
    after: ReplayCursor,
    through: ReplayCursor,
    page: PublicJournalPage,
) -> Result<PublicReplayPage, PublicationError> {
    let scanned = ReplayCursor::from_journal_offset(page.scanned_through);
    if scanned < after || scanned > through {
        return Err(PublicationError::invariant());
    }
    let mut events = Vec::new();
    let mut prior = after;
    for candidate in page.candidates {
        let cursor = ReplayCursor::from_journal_offset(candidate.journal_offset);
        if cursor <= prior || cursor > scanned {
            return Err(PublicationError::invariant());
        }
        prior = cursor;
        if let Some(event) = map_public_event(candidate)? {
            enforce_event_size(&event)?;
            events.push(event);
        }
    }
    Ok(PublicReplayPage {
        events,
        scanned_through: scanned,
        has_more: page.has_more,
    })
}

fn map_public_event(event: JournalEvent) -> Result<Option<DurableEventEnvelope>, PublicationError> {
    if event.event_version != 1 {
        return Err(PublicationError::invariant());
    }
    let (event_type, payload) = match &event.payload {
        JournalEventPayload::MessageAccepted(message) => (
            "message.accepted",
            json!({
                "message_id": message.message_id,
                "role": message.role,
                "content": message.content.blocks().iter().map(PublicContentBlock::from_domain).collect::<Vec<_>>(),
                "client_message_id": message.client_message_id,
                "committed_at": message.committed_at,
            }),
        ),
        JournalEventPayload::WorkQueued(work) => (
            "work.queued",
            json!({
                "work_id": work.work_id,
                "conversation_work_ordinal": work.conversation_work_ordinal,
                "state": "queued",
                "queued_at": work.queued_at,
            }),
        ),
        JournalEventPayload::WorkStarted(transition) => (
            "work.started",
            json!({"state": transition.to_state, "transitioned_at": transition.transitioned_at}),
        ),
        JournalEventPayload::WorkResumed(transition) => (
            "work.started",
            json!({
                "state": transition.to_state,
                "transition_kind": "resumed",
                "transitioned_at": transition.transitioned_at,
            }),
        ),
        JournalEventPayload::WorkWaitingOnModel(transition) => (
            "work.waiting_on_model",
            json!({"state": transition.to_state, "transitioned_at": transition.transitioned_at}),
        ),
        JournalEventPayload::WorkWaitingOnTool(transition) => (
            "work.waiting_on_tool",
            json!({"state": transition.to_state, "transitioned_at": transition.transitioned_at}),
        ),
        JournalEventPayload::WorkCancelRequested(transition) => (
            "work.cancel_requested",
            json!({"state": transition.to_state, "transitioned_at": transition.transitioned_at}),
        ),
        JournalEventPayload::WorkCancelled(transition) => (
            "work.cancelled",
            json!({
                "state": transition.to_state,
                "terminal_reason": transition.terminal_reason,
                "transitioned_at": transition.transitioned_at,
            }),
        ),
        JournalEventPayload::WorkCompleted(transition) => (
            "work.completed",
            json!({
                "state": transition.to_state,
                "terminal_reason": transition.terminal_reason,
                "transitioned_at": transition.transitioned_at,
            }),
        ),
        JournalEventPayload::WorkFailed(transition) => (
            "work.failed",
            json!({
                "state": transition.to_state,
                "terminal_reason": transition.terminal_reason,
                "transitioned_at": transition.transitioned_at,
            }),
        ),
        JournalEventPayload::WorkInterrupted(transition) => (
            "work.interrupted",
            json!({
                "state": transition.to_state,
                "terminal_reason": transition.terminal_reason,
                "transitioned_at": transition.transitioned_at,
            }),
        ),
        JournalEventPayload::ToolExecutionDispatching(tool) => (
            "tool.execution_started",
            json!({
                "tool_execution_id": tool.tool_execution_id,
                "status": "dispatching",
                "observed_at": tool.observed_at,
            }),
        ),
        JournalEventPayload::ToolExecutionCompleted(tool) => (
            "tool.execution_finished",
            json!({
                "tool_execution_id": tool.tool_execution_id,
                "status": "completed",
                "result_class": tool.outcome_classification,
                "outcome_unknown": false,
                "observed_at": tool.observed_at,
            }),
        ),
        JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(tool) => (
            "tool.execution_interrupted_before_dispatch",
            json!({
                "tool_execution_id": tool.tool_execution_id,
                "status": "interrupted_before_dispatch",
                "outcome_unknown": false,
                "observed_at": tool.observed_at,
            }),
        ),
        JournalEventPayload::ToolExecutionOutcomeUnknown(tool) => (
            "tool.execution_finished",
            json!({
                "tool_execution_id": tool.tool_execution_id,
                "status": "outcome_unknown",
                "outcome_unknown": true,
                "observed_at": tool.observed_at,
            }),
        ),
        JournalEventPayload::AssistantMessageCommitted(message) => (
            "assistant.message_committed",
            json!({
                "message_id": message.message_id,
                "role": message.role,
                "content": message.content.blocks().iter().map(PublicContentBlock::from_domain).collect::<Vec<_>>(),
                "work_id": message.produced_by_work_id,
                "committed_at": message.committed_at,
            }),
        ),
        JournalEventPayload::RuntimeRecoveryPerformed(recovery) => {
            let relevant = recovery.interrupted_work
                + recovery.model_attempts_provider_outcome_unknown
                + recovery.tool_attempts_interrupted_before_dispatch
                + recovery.tool_attempts_outcome_unknown
                + recovery.cleanup_unconfirmed;
            if relevant == 0 {
                return Ok(None);
            }
            (
                "runtime.recovery_performed",
                json!({
                    "interrupted_work": recovery.interrupted_work,
                    "provider_outcomes_unknown": recovery.model_attempts_provider_outcome_unknown,
                    "tools_interrupted_before_dispatch": recovery.tool_attempts_interrupted_before_dispatch,
                    "tool_outcomes_unknown": recovery.tool_attempts_outcome_unknown,
                    "cleanup_unconfirmed": recovery.cleanup_unconfirmed,
                    "recovered_at": recovery.recovered_at,
                }),
            )
        }
        JournalEventPayload::CraxiiInitialized(_)
        | JournalEventPayload::ConversationCreated(_)
        | JournalEventPayload::ModelInvocationStarted(_)
        | JournalEventPayload::ModelInvocationStreaming(_)
        | JournalEventPayload::ModelInvocationCompleted(_)
        | JournalEventPayload::ModelInvocationFailed(_)
        | JournalEventPayload::ModelInvocationInterrupted(_)
        | JournalEventPayload::ToolExecutionRequested(_)
        | JournalEventPayload::ArtifactRecorded(_)
        | JournalEventPayload::RuntimeStarted(_)
        | JournalEventPayload::RuntimeStopping(_) => return Ok(None),
    };
    Ok(Some(DurableEventEnvelope {
        protocol_version: ProtocolVersion,
        delivery_kind: DeliveryKind::Durable,
        event_id: event.event_id,
        cursor: event.journal_offset,
        event_type,
        conversation_id: event.conversation_id,
        work_id: event.work_id,
        recorded_at: event.recorded_at,
        payload,
    }))
}

fn enforce_event_size(event: &DurableEventEnvelope) -> Result<(), PublicationError> {
    let payload = serde_json::to_vec(&event.payload).map_err(|_| PublicationError::invariant())?;
    let frame = serde_json::to_vec(event).map_err(|_| PublicationError::invariant())?;
    if payload.len() > MAX_DURABLE_PAYLOAD_BYTES || frame.len() > MAX_WEBSOCKET_FRAME_BYTES {
        return Err(PublicationError::invariant());
    }
    Ok(())
}

pub(crate) fn encode_public_event_frame(
    event: &DurableEventEnvelope,
) -> Result<String, PublicationError> {
    let payload = serde_json::to_vec(&event.payload).map_err(|_| PublicationError::invariant())?;
    if payload.len() > MAX_DURABLE_PAYLOAD_BYTES {
        return Err(PublicationError::invariant());
    }
    let frame = serde_json::to_string(event).map_err(|_| PublicationError::invariant())?;
    if frame.len() > MAX_WEBSOCKET_FRAME_BYTES {
        return Err(PublicationError::invariant());
    }
    Ok(frame)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationErrorKind {
    Storage,
    Invariant,
    BootstrapLimitExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationError {
    kind: PublicationErrorKind,
}

impl PublicationError {
    const fn from_store(error: StateStoreError) -> Self {
        let kind = match error.kind() {
            StateStoreErrorKind::Storage => PublicationErrorKind::Storage,
            StateStoreErrorKind::StateConflict
            | StateStoreErrorKind::InternalInvariant
            | StateStoreErrorKind::IdempotencyConflict
            | StateStoreErrorKind::TargetNotFound => PublicationErrorKind::Invariant,
        };
        Self { kind }
    }

    const fn invariant() -> Self {
        Self {
            kind: PublicationErrorKind::Invariant,
        }
    }

    const fn limit() -> Self {
        Self {
            kind: PublicationErrorKind::BootstrapLimitExceeded,
        }
    }

    #[must_use]
    pub const fn kind(self) -> PublicationErrorKind {
        self.kind
    }
}

impl fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            PublicationErrorKind::Storage => "public state storage failure",
            PublicationErrorKind::Invariant => "public state invariant failure",
            PublicationErrorKind::BootstrapLimitExceeded => "bootstrap limit exceeded",
        })
    }
}

impl std::error::Error for PublicationError {}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{PublicationErrorKind, encode_public_event_frame};
    use crate::domain::{JournalEventId, JournalOffset, UtcTimestamp};
    use crate::protocol::{
        DeliveryKind, DurableEventEnvelope, MAX_DURABLE_PAYLOAD_BYTES, MAX_WEBSOCKET_FRAME_BYTES,
        ProtocolVersion,
    };

    fn event(body: String) -> DurableEventEnvelope {
        DurableEventEnvelope {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Durable,
            event_id: JournalEventId::generate(),
            cursor: JournalOffset::try_new(1).unwrap(),
            event_type: "message.accepted",
            conversation_id: None,
            work_id: None,
            recorded_at: UtcTimestamp::parse_canonical("2026-08-28T00:00:00.000000Z").unwrap(),
            payload: json!({"body": body}),
        }
    }

    #[test]
    fn public_event_frame_size_boundary_encodes_without_truncation_and_rejects_oversize() {
        let empty_payload_bytes = serde_json::to_vec(&json!({"body": ""})).unwrap().len();
        let legal_body_bytes = MAX_DURABLE_PAYLOAD_BYTES - empty_payload_bytes;
        let legal = event("x".repeat(legal_body_bytes));
        let encoded = encode_public_event_frame(&legal).unwrap();
        assert_eq!(
            serde_json::to_vec(&legal.payload).unwrap().len(),
            MAX_DURABLE_PAYLOAD_BYTES
        );
        assert!(encoded.len() <= MAX_WEBSOCKET_FRAME_BYTES);
        let decoded: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded["payload"]["body"].as_str().unwrap().len(),
            legal_body_bytes
        );

        let oversized = event("x".repeat(legal_body_bytes + 1));
        let error = encode_public_event_frame(&oversized).unwrap_err();
        assert_eq!(error.kind(), PublicationErrorKind::Invariant);
    }
}
