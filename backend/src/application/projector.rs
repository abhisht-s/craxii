//! Pure deterministic reconstruction from trusted typed journal events.

use std::collections::{HashMap, HashSet};

use crate::domain::{
    ArtifactId, ConversationCreatedV1, ConversationId, CorrelationId, CraxiiInitializedV1,
    JournalActor, JournalContractError, JournalCurrentAttempt, JournalEvent, JournalEventId,
    JournalEventKind, JournalEventPayload, JournalRuntimeState, JournalStreamId,
    MessageCommittedV1, ModelInvocationEventV1, ModelInvocationId, ProjectionVersion,
    RuntimeEventV1, RuntimeInstanceId, StreamSeq, ToolExecutionEventV1, ToolExecutionId,
    WorkCancellationReason, WorkId, WorkInputActor, WorkInputRelationship, WorkQueuedV1, WorkState,
    WorkTransitionV1, is_legal_model_pair, is_legal_tool_pair, is_legal_work_pair,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedMessage {
    pub stream_seq: StreamSeq,
    pub event_id: JournalEventId,
    pub message: MessageCommittedV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedWork {
    pub created: WorkQueuedV1,
    pub state: WorkState,
    pub state_version: ProjectionVersion,
    pub runtime_owner: Option<RuntimeInstanceId>,
    pub current_attempt: JournalCurrentAttempt,
    pub cancellation_reason: Option<WorkCancellationReason>,
    pub terminal_reason: Option<crate::domain::JournalWorkTerminalReason>,
    pub started_at: Option<crate::domain::UtcTimestamp>,
    pub cancel_requested_at: Option<crate::domain::UtcTimestamp>,
    pub terminal_at: Option<crate::domain::UtcTimestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedModelReference {
    pub kind: JournalEventKind,
    pub fact: ModelInvocationEventV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedToolReference {
    pub kind: JournalEventKind,
    pub fact: ToolExecutionEventV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedRuntimeReference {
    pub kind: JournalEventKind,
    pub fact: RuntimeEventV1,
}

/// Stage 7's journal-derived state. No backing Stage 8 row is fabricated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectedState {
    pub root: Option<CraxiiInitializedV1>,
    pub primary_conversation: Option<ConversationCreatedV1>,
    pub messages: HashMap<ConversationId, Vec<ProjectedMessage>>,
    pub works: HashMap<WorkId, ProjectedWork>,
    pub models: HashMap<ModelInvocationId, ProjectedModelReference>,
    pub tools: HashMap<ToolExecutionId, ProjectedToolReference>,
    pub artifacts: HashSet<ArtifactId>,
    pub runtimes: HashMap<RuntimeInstanceId, ProjectedRuntimeReference>,
    pub evidence_warnings: Vec<JournalEventId>,
}

struct ObservedEvent {
    correlation_id: CorrelationId,
    kind: JournalEventKind,
    conversation_id: Option<ConversationId>,
    work_id: Option<WorkId>,
}

/// Replays globally ordered typed events without I/O, clocks, randomness, or mutable globals.
pub fn project(events: &[JournalEvent]) -> Result<ProjectedState, JournalContractError> {
    let mut state = ProjectedState::default();
    let mut prior_offset = None;
    let mut stream_heads = HashMap::<JournalStreamId, StreamSeq>::new();
    let mut observed = HashMap::<JournalEventId, ObservedEvent>::new();

    for event in events {
        if event.event_version != 1 {
            return Err(JournalContractError::UnsupportedEventVersion);
        }
        if prior_offset.is_some_and(|prior| event.journal_offset <= prior) {
            return Err(JournalContractError::InvalidOrder);
        }
        prior_offset = Some(event.journal_offset);

        if observed.contains_key(&event.event_id) {
            return Err(JournalContractError::InvalidOrder);
        }
        let expected_seq = match stream_heads.get(&event.stream_id) {
            None => 1,
            Some(previous) => previous
                .checked_increment()
                .map_err(|_| JournalContractError::InvalidOrder)?
                .get(),
        };
        if event.stream_seq.get() != expected_seq {
            return Err(JournalContractError::InvalidOrder);
        }
        stream_heads.insert(event.stream_id, event.stream_seq);

        let cause = event
            .causation_event_id
            .map(|id| {
                if id == event.event_id {
                    return Err(JournalContractError::InvalidCausation);
                }
                observed
                    .get(&id)
                    .ok_or(JournalContractError::InvalidCausation)
            })
            .transpose()?;
        validate_stream_and_links(event)?;
        apply_event(&mut state, event, cause)?;
        observed.insert(
            event.event_id,
            ObservedEvent {
                correlation_id: event.correlation_id,
                kind: event.kind(),
                conversation_id: event.conversation_id,
                work_id: event.work_id,
            },
        );
    }
    Ok(state)
}

fn validate_stream_and_links(event: &JournalEvent) -> Result<(), JournalContractError> {
    if event.kind().primary_stream() != event.stream_id.family() {
        return Err(JournalContractError::InvalidEnvelope);
    }
    let valid = match (&event.payload, event.stream_id) {
        (JournalEventPayload::CraxiiInitialized(payload), JournalStreamId::Craxii(id)) => {
            id == payload.craxii_id
                && event.craxii_id == payload.craxii_id
                && event.conversation_id == Some(payload.primary_conversation_id)
                && event.work_id.is_none()
        }
        (JournalEventPayload::ConversationCreated(payload), JournalStreamId::Conversation(id)) => {
            id == payload.conversation_id
                && event.conversation_id == Some(payload.conversation_id)
                && event.craxii_id == payload.craxii_id
                && event.work_id.is_none()
        }
        (JournalEventPayload::MessageAccepted(payload), JournalStreamId::Conversation(id))
        | (
            JournalEventPayload::AssistantMessageCommitted(payload),
            JournalStreamId::Conversation(id),
        ) => {
            id == payload.conversation_id
                && event.conversation_id == Some(payload.conversation_id)
                && event.craxii_id == payload.craxii_id
                && event.work_id == payload.produced_by_work_id
        }
        (JournalEventPayload::WorkQueued(payload), JournalStreamId::Work(id)) => {
            id == payload.work_id
                && event.work_id == Some(payload.work_id)
                && event.conversation_id == Some(payload.conversation_id)
                && event.craxii_id == payload.craxii_id
                && event.correlation_id == payload.correlation_id
        }
        (
            JournalEventPayload::WorkStarted(payload)
            | JournalEventPayload::WorkWaitingOnModel(payload)
            | JournalEventPayload::WorkWaitingOnTool(payload)
            | JournalEventPayload::WorkResumed(payload)
            | JournalEventPayload::WorkCancelRequested(payload)
            | JournalEventPayload::WorkCancelled(payload)
            | JournalEventPayload::WorkCompleted(payload)
            | JournalEventPayload::WorkFailed(payload)
            | JournalEventPayload::WorkInterrupted(payload),
            JournalStreamId::Work(id),
        ) => id == payload.work_id && event.work_id == Some(payload.work_id),
        (
            JournalEventPayload::ModelInvocationStarted(payload)
            | JournalEventPayload::ModelInvocationCompleted(payload)
            | JournalEventPayload::ModelInvocationFailed(payload)
            | JournalEventPayload::ModelInvocationInterrupted(payload),
            JournalStreamId::Work(id),
        ) => id == payload.work_id && event.work_id == Some(payload.work_id),
        (
            JournalEventPayload::ToolExecutionRequested(payload)
            | JournalEventPayload::ToolExecutionDispatching(payload)
            | JournalEventPayload::ToolExecutionCompleted(payload)
            | JournalEventPayload::ToolExecutionOutcomeUnknown(payload),
            JournalStreamId::Work(id),
        ) => id == payload.work_id && event.work_id == Some(payload.work_id),
        (JournalEventPayload::ArtifactRecorded(payload), JournalStreamId::Work(id)) => {
            id == payload.work_id && event.work_id == Some(payload.work_id)
        }
        (
            JournalEventPayload::RuntimeStarted(payload)
            | JournalEventPayload::RuntimeRecoveryPerformed(payload)
            | JournalEventPayload::RuntimeStopping(payload),
            JournalStreamId::Runtime(id),
        ) => {
            id == payload.runtime_instance_id
                && event.runtime_instance_id == Some(payload.runtime_instance_id)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(JournalContractError::InvalidEnvelope)
    }
}

fn apply_event(
    state: &mut ProjectedState,
    event: &JournalEvent,
    cause: Option<&ObservedEvent>,
) -> Result<(), JournalContractError> {
    match &event.payload {
        JournalEventPayload::CraxiiInitialized(payload) => {
            if state.root.is_some()
                || event.causation_event_id.is_some()
                || event.actor != crate::domain::JournalActor::Craxii(payload.craxii_id)
            {
                return Err(JournalContractError::InconsistentProjection);
            }
            state.root = Some(payload.clone());
        }
        JournalEventPayload::ConversationCreated(payload) => {
            let Some(root) = state.root.as_ref() else {
                return Err(JournalContractError::InvalidCausation);
            };
            let Some(cause) = cause else {
                return Err(JournalContractError::InvalidCausation);
            };
            if cause.kind != JournalEventKind::CraxiiInitialized
                || cause.correlation_id != event.correlation_id
                || payload.conversation_id != root.primary_conversation_id
                || payload.craxii_id != root.craxii_id
                || event.actor != JournalActor::Craxii(root.craxii_id)
                || state.primary_conversation.is_some()
            {
                return Err(JournalContractError::InconsistentProjection);
            }
            state.primary_conversation = Some(payload.clone());
        }
        JournalEventPayload::MessageAccepted(payload) => {
            if payload.role != crate::domain::MessageRole::User
                || payload.validate_contract().is_err()
                || event.actor != JournalActor::User(payload.device_id)
                || state
                    .primary_conversation
                    .as_ref()
                    .is_none_or(|conversation| {
                        conversation.conversation_id != payload.conversation_id
                            || conversation.craxii_id != payload.craxii_id
                    })
                || state
                    .messages
                    .values()
                    .flatten()
                    .any(|message| message.message.message_id == payload.message_id)
            {
                return Err(JournalContractError::InvalidEnvelope);
            }
            state
                .messages
                .entry(payload.conversation_id)
                .or_default()
                .push(ProjectedMessage {
                    stream_seq: event.stream_seq,
                    event_id: event.event_id,
                    message: payload.clone(),
                });
        }
        JournalEventPayload::AssistantMessageCommitted(payload) => {
            if payload.role != crate::domain::MessageRole::Assistant
                || payload.validate_contract().is_err()
                || event.actor != JournalActor::Craxii(payload.craxii_id)
                || state
                    .primary_conversation
                    .as_ref()
                    .is_none_or(|conversation| {
                        conversation.conversation_id != payload.conversation_id
                            || conversation.craxii_id != payload.craxii_id
                    })
                || payload.produced_by_work_id.is_none_or(|work_id| {
                    state
                        .works
                        .get(&work_id)
                        .is_none_or(|work| work.created.correlation_id != event.correlation_id)
                })
                || state
                    .messages
                    .values()
                    .flatten()
                    .any(|message| message.message.message_id == payload.message_id)
            {
                return Err(JournalContractError::InvalidEnvelope);
            }
            state
                .messages
                .entry(payload.conversation_id)
                .or_default()
                .push(ProjectedMessage {
                    stream_seq: event.stream_seq,
                    event_id: event.event_id,
                    message: payload.clone(),
                });
        }
        JournalEventPayload::WorkQueued(payload) => {
            let Some(cause) = cause else {
                return Err(JournalContractError::InvalidCausation);
            };
            let root_matches = state.root.as_ref().is_some_and(|root| {
                root.craxii_id == payload.craxii_id && root.workspace_id == payload.workspace_id
            });
            let conversation = state
                .primary_conversation
                .as_mut()
                .ok_or(JournalContractError::InconsistentProjection)?;
            if cause.kind != JournalEventKind::MessageAccepted
                || cause.correlation_id != event.correlation_id
                || cause.conversation_id != Some(payload.conversation_id)
                || cause.work_id.is_some()
                || !root_matches
                || conversation.conversation_id != payload.conversation_id
                || conversation.craxii_id != payload.craxii_id
                || conversation.next_work_ordinal != payload.conversation_work_ordinal
                || payload.kind != crate::domain::WorkKind::Conversational
                || payload.priority != 0
                || payload.trigger.input_event_id != event.causation_event_id.unwrap()
                || payload.trigger.relationship != WorkInputRelationship::Trigger
                || payload.trigger.ordinal_within_work.get() != 1
                || payload.trigger.actor != WorkInputActor::User
                || event.actor != JournalActor::Craxii(payload.craxii_id)
                || payload.state_version.get() != 1
                || state.works.contains_key(&payload.work_id)
            {
                return Err(JournalContractError::InconsistentProjection);
            }
            conversation.next_work_ordinal = conversation
                .next_work_ordinal
                .checked_increment()
                .map_err(|_| JournalContractError::InconsistentProjection)?;
            conversation.state_version = conversation
                .state_version
                .checked_increment()
                .map_err(|_| JournalContractError::InconsistentProjection)?;
            state.works.insert(
                payload.work_id,
                ProjectedWork {
                    created: payload.clone(),
                    state: WorkState::Queued,
                    state_version: payload.state_version,
                    runtime_owner: None,
                    current_attempt: JournalCurrentAttempt::None,
                    cancellation_reason: None,
                    terminal_reason: None,
                    started_at: None,
                    cancel_requested_at: None,
                    terminal_at: None,
                },
            );
        }
        JournalEventPayload::WorkStarted(payload)
        | JournalEventPayload::WorkWaitingOnModel(payload)
        | JournalEventPayload::WorkWaitingOnTool(payload)
        | JournalEventPayload::WorkResumed(payload)
        | JournalEventPayload::WorkCancelRequested(payload)
        | JournalEventPayload::WorkCancelled(payload)
        | JournalEventPayload::WorkCompleted(payload)
        | JournalEventPayload::WorkFailed(payload)
        | JournalEventPayload::WorkInterrupted(payload) => {
            require_work_context(state, event, payload.work_id)?;
            if event.runtime_instance_id != payload.runtime_owner {
                return Err(JournalContractError::InvalidEnvelope);
            }
            apply_work_transition(state, event.correlation_id, payload)?;
        }
        JournalEventPayload::ModelInvocationStarted(payload)
        | JournalEventPayload::ModelInvocationCompleted(payload)
        | JournalEventPayload::ModelInvocationFailed(payload)
        | JournalEventPayload::ModelInvocationInterrupted(payload) => {
            require_work_context(state, event, payload.work_id)?;
            match event.kind() {
                JournalEventKind::ModelInvocationStarted => {
                    if payload.state != crate::domain::ModelInvocationState::Requesting
                        || state.models.contains_key(&payload.model_invocation_id)
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
                _ => {
                    let previous = state
                        .models
                        .get(&payload.model_invocation_id)
                        .ok_or(JournalContractError::InconsistentProjection)?;
                    if previous.fact.work_id != payload.work_id
                        || previous.fact.logical_invocation_id != payload.logical_invocation_id
                        || !is_legal_model_pair(previous.fact.state, payload.state)
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
            }
            state.models.insert(
                payload.model_invocation_id,
                ProjectedModelReference {
                    kind: event.kind(),
                    fact: payload.clone(),
                },
            );
        }
        JournalEventPayload::ToolExecutionRequested(payload)
        | JournalEventPayload::ToolExecutionDispatching(payload)
        | JournalEventPayload::ToolExecutionCompleted(payload)
        | JournalEventPayload::ToolExecutionOutcomeUnknown(payload) => {
            require_work_context(state, event, payload.work_id)?;
            match event.kind() {
                JournalEventKind::ToolExecutionRequested => {
                    if payload.state != crate::domain::ToolExecutionState::Requested
                        || state.tools.contains_key(&payload.tool_execution_id)
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
                _ => {
                    let previous = state
                        .tools
                        .get(&payload.tool_execution_id)
                        .ok_or(JournalContractError::InconsistentProjection)?;
                    if previous.fact.work_id != payload.work_id
                        || !is_legal_tool_pair(previous.fact.state, payload.state)
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
            }
            state.tools.insert(
                payload.tool_execution_id,
                ProjectedToolReference {
                    kind: event.kind(),
                    fact: payload.clone(),
                },
            );
        }
        JournalEventPayload::ArtifactRecorded(payload) => {
            require_work_context(state, event, payload.work_id)?;
            if !state.artifacts.insert(payload.artifact_id) {
                return Err(JournalContractError::InconsistentProjection);
            }
        }
        JournalEventPayload::RuntimeStarted(payload)
        | JournalEventPayload::RuntimeRecoveryPerformed(payload)
        | JournalEventPayload::RuntimeStopping(payload) => {
            if event.actor != JournalActor::Runtime(payload.runtime_instance_id)
                || state.root.as_ref().is_none_or(|root| {
                    root.craxii_id != event.craxii_id
                        || root.workstation_id != payload.workstation_id
                })
            {
                return Err(JournalContractError::InvalidEnvelope);
            }
            match event.kind() {
                JournalEventKind::RuntimeStarted => {
                    if payload.state != JournalRuntimeState::Running
                        || state.runtimes.contains_key(&payload.runtime_instance_id)
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
                JournalEventKind::RuntimeRecoveryPerformed | JournalEventKind::RuntimeStopping => {
                    let previous = state
                        .runtimes
                        .get(&payload.runtime_instance_id)
                        .ok_or(JournalContractError::InconsistentProjection)?;
                    if previous.fact.workstation_id != payload.workstation_id
                        || previous.fact.workstation_generation != payload.workstation_generation
                    {
                        return Err(JournalContractError::InconsistentProjection);
                    }
                }
                _ => unreachable!(),
            }
            state.runtimes.insert(
                payload.runtime_instance_id,
                ProjectedRuntimeReference {
                    kind: event.kind(),
                    fact: payload.clone(),
                },
            );
        }
    }
    if !event.kind().state_bearing() {
        state.evidence_warnings.push(event.event_id);
    }
    Ok(())
}

fn apply_work_transition(
    state: &mut ProjectedState,
    correlation_id: CorrelationId,
    payload: &WorkTransitionV1,
) -> Result<(), JournalContractError> {
    let work = state
        .works
        .get_mut(&payload.work_id)
        .ok_or(JournalContractError::InconsistentProjection)?;
    if work.created.correlation_id != correlation_id
        || work.state != payload.from_state
        || work.state_version != payload.expected_state_version
        || work.runtime_owner != payload.expected_runtime_owner
        || work.current_attempt != payload.expected_current_attempt
        || work.cancellation_reason != payload.expected_cancellation_reason
        || payload.state_version
            != payload
                .expected_state_version
                .checked_increment()
                .map_err(|_| JournalContractError::InconsistentProjection)?
        || !is_legal_work_pair(payload.from_state, payload.to_state)
        || work.state.is_terminal()
    {
        return Err(JournalContractError::InconsistentProjection);
    }
    validate_projected_work_shape(payload)?;
    if payload.from_state == WorkState::Queued && payload.to_state == WorkState::Running {
        work.started_at = Some(payload.transitioned_at);
    }
    if payload.to_state == WorkState::CancelRequested {
        work.cancel_requested_at = Some(payload.transitioned_at);
    }
    if payload.to_state.is_terminal() {
        work.terminal_at = Some(payload.transitioned_at);
    }
    work.state = payload.to_state;
    work.state_version = payload.state_version;
    work.runtime_owner = payload.runtime_owner;
    work.current_attempt = payload.current_attempt;
    work.cancellation_reason = payload.cancellation_reason;
    work.terminal_reason = payload.terminal_reason;
    Ok(())
}

fn validate_projected_work_shape(payload: &WorkTransitionV1) -> Result<(), JournalContractError> {
    let valid = match payload.to_state {
        WorkState::Queued => false,
        WorkState::Running => {
            payload.runtime_owner.is_some()
                && payload.current_attempt == JournalCurrentAttempt::None
                && payload.cancellation_reason.is_none()
                && payload.terminal_reason.is_none()
        }
        WorkState::WaitingOnModel => {
            payload.runtime_owner.is_some()
                && matches!(payload.current_attempt, JournalCurrentAttempt::Model(_))
                && payload.cancellation_reason.is_none()
                && payload.terminal_reason.is_none()
        }
        WorkState::WaitingOnTool => {
            payload.runtime_owner.is_some()
                && matches!(payload.current_attempt, JournalCurrentAttempt::Tool(_))
                && payload.cancellation_reason.is_none()
                && payload.terminal_reason.is_none()
        }
        WorkState::CancelRequested => {
            payload.runtime_owner.is_some()
                && payload.cancellation_reason.is_some()
                && payload.terminal_reason.is_none()
        }
        WorkState::Completed => {
            payload.runtime_owner.is_none()
                && payload.current_attempt == JournalCurrentAttempt::None
                && matches!(
                    payload.terminal_reason,
                    Some(
                        crate::domain::JournalWorkTerminalReason::Answered
                            | crate::domain::JournalWorkTerminalReason::Refused
                    )
                )
        }
        WorkState::Failed => {
            payload.runtime_owner.is_none()
                && payload.current_attempt == JournalCurrentAttempt::None
                && matches!(
                    payload.terminal_reason,
                    Some(
                        crate::domain::JournalWorkTerminalReason::DefiniteNormalizedError
                            | crate::domain::JournalWorkTerminalReason::ProviderExhausted
                            | crate::domain::JournalWorkTerminalReason::InvalidModelOutput
                            | crate::domain::JournalWorkTerminalReason::LifecycleLimit
                    )
                )
        }
        WorkState::Cancelled => {
            payload.runtime_owner.is_none()
                && payload.current_attempt == JournalCurrentAttempt::None
                && matches!(
                    payload.terminal_reason,
                    Some(
                        crate::domain::JournalWorkTerminalReason::UserRequest
                            | crate::domain::JournalWorkTerminalReason::GracefulShutdown
                    )
                )
        }
        WorkState::Interrupted => {
            payload.runtime_owner.is_none()
                && payload.current_attempt == JournalCurrentAttempt::None
                && matches!(
                payload.terminal_reason,
                Some(
                    crate::domain::JournalWorkTerminalReason::RuntimeOwnershipLost
                        | crate::domain::JournalWorkTerminalReason::ProviderOutcomeUnknown
                        | crate::domain::JournalWorkTerminalReason::ToolInterruptedBeforeDispatch
                        | crate::domain::JournalWorkTerminalReason::ToolOutcomeUnknown
                        | crate::domain::JournalWorkTerminalReason::CleanupUnconfirmed
                )
            )
        }
    };
    if valid {
        Ok(())
    } else {
        Err(JournalContractError::InconsistentProjection)
    }
}

fn require_work_context(
    state: &ProjectedState,
    event: &JournalEvent,
    work_id: WorkId,
) -> Result<(), JournalContractError> {
    if state.works.get(&work_id).is_some_and(|work| {
        work.created.correlation_id == event.correlation_id
            && work.created.craxii_id == event.craxii_id
            && event.conversation_id == Some(work.created.conversation_id)
            && event.work_id == Some(work_id)
    }) {
        Ok(())
    } else {
        Err(JournalContractError::InconsistentProjection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ClientMessageId, ContentBlock, ConversationLifecycle, ConversationWorkOrdinal, DeviceId,
        JournalActor, JournalEventPayload, JournalOffset, MessageContent, MessageId, MessageRole,
        Sha256Digest, UtcTimestamp, WorkInputActor, WorkInputFactV1, WorkInputOrdinal, WorkKind,
    };

    fn at() -> UtcTimestamp {
        "2026-08-28T00:00:00.000001Z".parse().unwrap()
    }

    fn bootstrap_events() -> Vec<JournalEvent> {
        let craxii_id = crate::domain::CraxiiId::generate();
        let conversation_id = ConversationId::generate();
        let root_event_id = JournalEventId::generate();
        let correlation_id = CorrelationId::generate();
        let root_payload = CraxiiInitializedV1 {
            craxii_id,
            display_name: "Craxii".into(),
            owner_label: "local-owner".into(),
            architecture_revision: "V0.0.01".into(),
            schema_revision: crate::domain::SchemaVersion::try_new(2).unwrap(),
            workstation_id: crate::domain::WorkstationId::generate(),
            workstation_generation: crate::domain::WorkstationGeneration::try_new(1).unwrap(),
            workstation_architecture: "aarch64".into(),
            workstation_os_release: "test".into(),
            capabilities_sha256: Sha256Digest::hash_bytes(b"capabilities"),
            workspace_id: crate::domain::WorkspaceId::generate(),
            workspace_logical_name: "primary".into(),
            workspace_logical_root: "/workspace".into(),
            primary_conversation_id: conversation_id,
            created_at: at(),
        };
        let conversation_payload = ConversationCreatedV1 {
            conversation_id,
            craxii_id,
            kind: crate::domain::ConversationKind::Primary,
            lifecycle: ConversationLifecycle::Active,
            next_work_ordinal: crate::domain::ConversationWorkOrdinal::try_new(1).unwrap(),
            state_version: ProjectionVersion::try_new(1).unwrap(),
            created_at: at(),
        };
        vec![
            JournalEvent {
                journal_offset: JournalOffset::try_new(1).unwrap(),
                event_id: root_event_id,
                craxii_id,
                stream_id: JournalStreamId::Craxii(craxii_id),
                stream_seq: StreamSeq::try_new(1).unwrap(),
                event_version: 1,
                conversation_id: Some(conversation_id),
                work_id: None,
                causation_event_id: None,
                correlation_id,
                actor: JournalActor::Craxii(craxii_id),
                runtime_instance_id: None,
                payload: JournalEventPayload::CraxiiInitialized(root_payload),
                payload_sha256: Sha256Digest::hash_bytes(b"root"),
                recorded_at: at(),
                occurred_at: None,
            },
            JournalEvent {
                journal_offset: JournalOffset::try_new(4).unwrap(),
                event_id: JournalEventId::generate(),
                craxii_id,
                stream_id: JournalStreamId::Conversation(conversation_id),
                stream_seq: StreamSeq::try_new(1).unwrap(),
                event_version: 1,
                conversation_id: Some(conversation_id),
                work_id: None,
                causation_event_id: Some(root_event_id),
                correlation_id,
                actor: JournalActor::Craxii(craxii_id),
                runtime_instance_id: None,
                payload: JournalEventPayload::ConversationCreated(conversation_payload),
                payload_sha256: Sha256Digest::hash_bytes(b"conversation"),
                recorded_at: at(),
                occurred_at: None,
            },
        ]
    }

    fn message_payload(
        craxii_id: crate::domain::CraxiiId,
        conversation_id: ConversationId,
    ) -> MessageCommittedV1 {
        let content = MessageContent::try_new(vec![ContentBlock::text("input").unwrap()]).unwrap();
        MessageCommittedV1 {
            message_id: MessageId::generate(),
            craxii_id,
            conversation_id,
            role: MessageRole::User,
            content_sha256: content.content_sha256(),
            content,
            produced_by_work_id: None,
            device_id: Some(DeviceId::generate()),
            client_message_id: Some(
                "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d"
                    .parse::<ClientMessageId>()
                    .unwrap(),
            ),
            committed_at: at(),
        }
    }

    fn append_message_and_work(
        events: &mut Vec<JournalEvent>,
        offset: i64,
        conversation_seq: i64,
    ) -> (JournalEventId, WorkId) {
        let root = events[0].craxii_id;
        let conversation = events[1].conversation_id.unwrap();
        let workspace_id = match &events[0].payload {
            JournalEventPayload::CraxiiInitialized(payload) => payload.workspace_id,
            _ => unreachable!(),
        };
        let message_event_id = JournalEventId::generate();
        let work_id = WorkId::generate();
        let correlation_id = CorrelationId::generate();
        let message = message_payload(root, conversation);
        events.push(JournalEvent {
            journal_offset: JournalOffset::try_new(offset).unwrap(),
            event_id: message_event_id,
            craxii_id: root,
            stream_id: JournalStreamId::Conversation(conversation),
            stream_seq: StreamSeq::try_new(conversation_seq).unwrap(),
            event_version: 1,
            conversation_id: Some(conversation),
            work_id: None,
            causation_event_id: Some(events[1].event_id),
            correlation_id,
            actor: JournalActor::User(message.device_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::MessageAccepted(message),
            payload_sha256: Sha256Digest::hash_bytes(b"message"),
            recorded_at: at(),
            occurred_at: None,
        });
        events.push(JournalEvent {
            journal_offset: JournalOffset::try_new(offset + 1).unwrap(),
            event_id: JournalEventId::generate(),
            craxii_id: root,
            stream_id: JournalStreamId::Work(work_id),
            stream_seq: StreamSeq::try_new(1).unwrap(),
            event_version: 1,
            conversation_id: Some(conversation),
            work_id: Some(work_id),
            causation_event_id: Some(message_event_id),
            correlation_id,
            actor: JournalActor::Craxii(root),
            runtime_instance_id: None,
            payload: JournalEventPayload::WorkQueued(WorkQueuedV1 {
                work_id,
                craxii_id: root,
                conversation_id: conversation,
                conversation_work_ordinal: ConversationWorkOrdinal::try_new(
                    (conversation_seq + 1) / 2,
                )
                .unwrap(),
                kind: WorkKind::Conversational,
                priority: 0,
                workspace_id,
                correlation_id,
                state_version: ProjectionVersion::try_new(1).unwrap(),
                created_at: at(),
                queued_at: at(),
                trigger: WorkInputFactV1 {
                    input_event_id: message_event_id,
                    relationship: WorkInputRelationship::Trigger,
                    ordinal_within_work: WorkInputOrdinal::try_new(1).unwrap(),
                    attached_at: at(),
                    actor: WorkInputActor::User,
                },
            }),
            payload_sha256: Sha256Digest::hash_bytes(b"work"),
            recorded_at: at(),
            occurred_at: None,
        });
        (message_event_id, work_id)
    }

    #[test]
    fn replay_is_deterministic_and_global_offset_gaps_are_allowed() {
        let events = bootstrap_events();
        assert_eq!(project(&events).unwrap(), project(&events.clone()).unwrap());
    }

    #[test]
    fn duplicate_ids_nonincreasing_offsets_stream_gaps_and_forward_causes_fail() {
        let events = bootstrap_events();
        let mut duplicate = events.clone();
        duplicate[1].event_id = duplicate[0].event_id;
        assert!(project(&duplicate).is_err());
        let mut offset = events.clone();
        offset[1].journal_offset = offset[0].journal_offset;
        assert!(project(&offset).is_err());
        let mut stream = events.clone();
        stream[1].stream_seq = StreamSeq::try_new(2).unwrap();
        assert!(project(&stream).is_err());
        let mut cause = events;
        cause[0].causation_event_id = Some(cause[1].event_id);
        assert!(project(&cause).is_err());
    }

    #[test]
    fn causal_inputs_are_explicit_and_later_messages_do_not_leak_into_prior_work() {
        let mut events = bootstrap_events();
        let (message_a, work_a) = append_message_and_work(&mut events, 5, 2);
        let (message_b, work_b) = append_message_and_work(&mut events, 7, 3);
        let projected = project(&events).unwrap();
        assert_eq!(
            projected.works[&work_a].created.trigger.input_event_id,
            message_a
        );
        assert_eq!(
            projected.works[&work_b].created.trigger.input_event_id,
            message_b
        );
        assert_ne!(
            projected.works[&work_a].created.trigger.input_event_id,
            message_b
        );
    }

    #[test]
    fn canonical_message_order_uses_conversation_stream_sequence_only() {
        let mut events = bootstrap_events();
        let root = events[0].craxii_id;
        let conversation = events[1].conversation_id.unwrap();
        let first = message_payload(root, conversation);
        let first_id = first.message_id;
        let second = message_payload(root, conversation);
        let second_id = second.message_id;
        let cause = events[1].event_id;
        for (offset, seq, payload) in [(20, 2, first), (41, 3, second)] {
            let actor = JournalActor::User(payload.device_id);
            events.push(JournalEvent {
                journal_offset: JournalOffset::try_new(offset).unwrap(),
                event_id: JournalEventId::generate(),
                craxii_id: root,
                stream_id: JournalStreamId::Conversation(conversation),
                stream_seq: StreamSeq::try_new(seq).unwrap(),
                event_version: 1,
                conversation_id: Some(conversation),
                work_id: None,
                causation_event_id: Some(cause),
                correlation_id: CorrelationId::generate(),
                actor,
                runtime_instance_id: None,
                payload: JournalEventPayload::MessageAccepted(payload),
                payload_sha256: Sha256Digest::hash_bytes(b"message"),
                recorded_at: at(),
                occurred_at: None,
            });
        }
        let messages = &project(&events).unwrap().messages[&conversation];
        assert_eq!(
            messages
                .iter()
                .map(|message| message.message.message_id)
                .collect::<Vec<_>>(),
            vec![first_id, second_id]
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.stream_seq.get())
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn projector_accepts_exactly_all_seventeen_stage_four_work_pairs() {
        let pairs = [
            (WorkState::Queued, WorkState::Running),
            (WorkState::Queued, WorkState::Cancelled),
            (WorkState::Running, WorkState::WaitingOnModel),
            (WorkState::WaitingOnModel, WorkState::Running),
            (WorkState::WaitingOnModel, WorkState::Failed),
            (WorkState::Running, WorkState::WaitingOnTool),
            (WorkState::WaitingOnTool, WorkState::Running),
            (WorkState::Running, WorkState::CancelRequested),
            (WorkState::WaitingOnModel, WorkState::CancelRequested),
            (WorkState::WaitingOnTool, WorkState::CancelRequested),
            (WorkState::Running, WorkState::Completed),
            (WorkState::Running, WorkState::Failed),
            (WorkState::Running, WorkState::Interrupted),
            (WorkState::WaitingOnModel, WorkState::Interrupted),
            (WorkState::WaitingOnTool, WorkState::Interrupted),
            (WorkState::CancelRequested, WorkState::Cancelled),
            (WorkState::CancelRequested, WorkState::Interrupted),
        ];
        assert_eq!(pairs.len(), 17);
        for (from, to) in pairs {
            assert!(is_legal_work_pair(from, to));
            let mut events = bootstrap_events();
            let (_, work_id) = append_message_and_work(&mut events, 5, 2);
            let queued = match &events[3].payload {
                JournalEventPayload::WorkQueued(payload) => payload.clone(),
                _ => unreachable!(),
            };
            let expected_current_attempt = match from {
                WorkState::WaitingOnModel => {
                    JournalCurrentAttempt::Model(ModelInvocationId::generate())
                }
                WorkState::WaitingOnTool => {
                    JournalCurrentAttempt::Tool(ToolExecutionId::generate())
                }
                _ => JournalCurrentAttempt::None,
            };
            let expected_runtime_owner =
                (!matches!(from, WorkState::Queued)).then(RuntimeInstanceId::generate);
            let expected_cancellation_reason =
                (from == WorkState::CancelRequested).then_some(WorkCancellationReason::UserRequest);
            let current_attempt = match to {
                WorkState::WaitingOnModel => {
                    JournalCurrentAttempt::Model(ModelInvocationId::generate())
                }
                WorkState::WaitingOnTool => {
                    JournalCurrentAttempt::Tool(ToolExecutionId::generate())
                }
                WorkState::CancelRequested => expected_current_attempt,
                _ => JournalCurrentAttempt::None,
            };
            let terminal_reason = match to {
                WorkState::Completed => Some(crate::domain::JournalWorkTerminalReason::Answered),
                WorkState::Failed => {
                    Some(crate::domain::JournalWorkTerminalReason::ProviderExhausted)
                }
                WorkState::Cancelled => Some(crate::domain::JournalWorkTerminalReason::UserRequest),
                WorkState::Interrupted => {
                    Some(crate::domain::JournalWorkTerminalReason::RuntimeOwnershipLost)
                }
                _ => None,
            };
            let transition = WorkTransitionV1 {
                work_id,
                from_state: from,
                to_state: to,
                expected_state_version: ProjectionVersion::try_new(1).unwrap(),
                expected_runtime_owner,
                expected_current_attempt,
                expected_cancellation_reason,
                state_version: ProjectionVersion::try_new(2).unwrap(),
                runtime_owner: if to.is_terminal() {
                    None
                } else if from == WorkState::Queued {
                    Some(RuntimeInstanceId::generate())
                } else {
                    expected_runtime_owner
                },
                current_attempt,
                cancellation_reason: (to == WorkState::CancelRequested)
                    .then_some(WorkCancellationReason::UserRequest),
                terminal_reason,
                transitioned_at: at(),
            };
            let craxii_id = queued.craxii_id;
            let conversation_id = queued.conversation_id;
            let runtime_instance_id = transition.runtime_owner;
            let mut state = ProjectedState::default();
            state.works.insert(
                work_id,
                ProjectedWork {
                    created: queued,
                    state: from,
                    state_version: ProjectionVersion::try_new(1).unwrap(),
                    runtime_owner: expected_runtime_owner,
                    current_attempt: expected_current_attempt,
                    cancellation_reason: expected_cancellation_reason,
                    terminal_reason: None,
                    started_at: (!matches!(from, WorkState::Queued)).then(at),
                    cancel_requested_at: (from == WorkState::CancelRequested).then(at),
                    terminal_at: None,
                },
            );
            let correlation = state.works[&work_id].created.correlation_id;
            let payload = match to {
                WorkState::Running if from == WorkState::Queued => {
                    JournalEventPayload::WorkStarted(transition)
                }
                WorkState::Running => JournalEventPayload::WorkResumed(transition),
                WorkState::WaitingOnModel => JournalEventPayload::WorkWaitingOnModel(transition),
                WorkState::WaitingOnTool => JournalEventPayload::WorkWaitingOnTool(transition),
                WorkState::CancelRequested => JournalEventPayload::WorkCancelRequested(transition),
                WorkState::Cancelled => JournalEventPayload::WorkCancelled(transition),
                WorkState::Completed => JournalEventPayload::WorkCompleted(transition),
                WorkState::Failed => JournalEventPayload::WorkFailed(transition),
                WorkState::Interrupted => JournalEventPayload::WorkInterrupted(transition),
                WorkState::Queued => unreachable!("no legal transition targets queued"),
            };
            let event = JournalEvent {
                journal_offset: JournalOffset::try_new(6).unwrap(),
                event_id: JournalEventId::generate(),
                craxii_id,
                stream_id: JournalStreamId::Work(work_id),
                stream_seq: StreamSeq::try_new(2).unwrap(),
                event_version: 1,
                conversation_id: Some(conversation_id),
                work_id: Some(work_id),
                causation_event_id: None,
                correlation_id: correlation,
                actor: JournalActor::Craxii(craxii_id),
                runtime_instance_id,
                payload,
                payload_sha256: Sha256Digest::hash_bytes(b"typed-test-event"),
                recorded_at: at(),
                occurred_at: None,
            };
            validate_stream_and_links(&event).unwrap();
            apply_event(&mut state, &event, None).unwrap();
            assert_eq!(state.works[&work_id].state, to);
            assert_eq!(state.works[&work_id].state_version.get(), 2);
        }
    }

    #[test]
    fn projector_rejects_illegal_stale_and_terminal_work_transitions() {
        let mut events = bootstrap_events();
        let (_, work_id) = append_message_and_work(&mut events, 5, 2);
        let created = match &events[3].payload {
            JournalEventPayload::WorkQueued(payload) => payload.clone(),
            _ => unreachable!(),
        };
        let correlation = created.correlation_id;
        let seed = |state, version| {
            let mut projected = ProjectedState::default();
            projected.works.insert(
                work_id,
                ProjectedWork {
                    created: created.clone(),
                    state,
                    state_version: ProjectionVersion::try_new(version).unwrap(),
                    runtime_owner: None,
                    current_attempt: JournalCurrentAttempt::None,
                    cancellation_reason: None,
                    terminal_reason: state
                        .is_terminal()
                        .then_some(crate::domain::JournalWorkTerminalReason::Answered),
                    started_at: (!matches!(state, WorkState::Queued)).then(at),
                    cancel_requested_at: None,
                    terminal_at: state.is_terminal().then(at),
                },
            );
            projected
        };
        let transition = |from_state, to_state, expected, next| WorkTransitionV1 {
            work_id,
            from_state,
            to_state,
            expected_state_version: ProjectionVersion::try_new(expected).unwrap(),
            expected_runtime_owner: None,
            expected_current_attempt: JournalCurrentAttempt::None,
            expected_cancellation_reason: None,
            state_version: ProjectionVersion::try_new(next).unwrap(),
            runtime_owner: (to_state == WorkState::Running).then(RuntimeInstanceId::generate),
            current_attempt: JournalCurrentAttempt::None,
            cancellation_reason: None,
            terminal_reason: (to_state == WorkState::Completed)
                .then_some(crate::domain::JournalWorkTerminalReason::Answered),
            transitioned_at: at(),
        };

        let mut illegal = seed(WorkState::Queued, 1);
        assert!(
            apply_work_transition(
                &mut illegal,
                correlation,
                &transition(WorkState::Queued, WorkState::Completed, 1, 2)
            )
            .is_err()
        );

        let mut stale = seed(WorkState::Queued, 1);
        assert!(
            apply_work_transition(
                &mut stale,
                correlation,
                &transition(WorkState::Queued, WorkState::Running, 2, 3)
            )
            .is_err()
        );

        let mut terminal = seed(WorkState::Completed, 2);
        assert!(
            apply_work_transition(
                &mut terminal,
                correlation,
                &transition(WorkState::Completed, WorkState::Running, 2, 3)
            )
            .is_err()
        );
    }
}
