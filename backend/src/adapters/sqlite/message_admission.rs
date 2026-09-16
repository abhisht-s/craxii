//! SQLite-private transport-neutral canonical message/work admission kernel.

use crate::domain::{
    ClientMessageId, ConversationBindingId, ConversationId, ConversationWorkOrdinal, CorrelationId,
    CraxiiId, DeviceId, InboundDeliveryId, JournalActor, JournalEventId, JournalEventPayload,
    JournalOffset, JournalStreamId, Message, MessageAcceptedOriginV2, MessageCommittedV2,
    MessageContent, MessageId, MessageInput, MessageRole, ProjectionVersion, UserId, UtcTimestamp,
    WorkId, WorkInputActor, WorkInputFactV1, WorkInputOrdinal, WorkInputRelationship, WorkItem,
    WorkItemInput, WorkItemInputData, WorkKind, WorkQueuedV2, WorkspaceId,
};

use super::codec::encode_message_content;
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::projection::{ProjectionMutationError, advance_conversation_ordinal};
use super::transaction::WriteTransaction;

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

fn invalid() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InternalInvariant)
}

fn map_projection_error<C>(error: ProjectionMutationError<C>) -> SqliteAdapterError {
    match error {
        ProjectionMutationError::Conflict(_) => inconsistent(),
        ProjectionMutationError::Storage(error) => error,
        ProjectionMutationError::Invariant => invalid(),
    }
}

pub(super) struct CanonicalMessageTopology {
    pub craxii_id: CraxiiId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub workspace_id: WorkspaceId,
    pub next_ordinal: ConversationWorkOrdinal,
    pub conversation_version: ProjectionVersion,
    pub conversation_created_event_id: JournalEventId,
}

#[derive(Clone, Copy)]
pub(super) enum CanonicalMessageOrigin {
    Native {
        device_id: DeviceId,
        client_message_id: ClientMessageId,
    },
    Channel {
        inbound_delivery_id: InboundDeliveryId,
    },
}

#[derive(Clone, Copy)]
pub(super) struct CanonicalMessageCandidates {
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub acceptance_event_id: JournalEventId,
    pub queued_event_id: JournalEventId,
}

pub(super) struct CanonicalMessageAdmission {
    pub topology: CanonicalMessageTopology,
    pub origin: CanonicalMessageOrigin,
    pub content: MessageContent,
    pub reply_binding_id: Option<ConversationBindingId>,
    pub admitted_at: UtcTimestamp,
    pub candidates: CanonicalMessageCandidates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CanonicalMessageAdmissionStep {
    MessageInserted,
    MessageAccepted,
    WorkInserted,
    WorkInputInserted,
    WorkQueued,
    ConversationAdvanced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanonicalMessageAdmissionReceipt {
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub committed_cursor: JournalOffset,
}

pub(super) async fn admit_canonical_message<F>(
    transaction: &mut WriteTransaction,
    request: CanonicalMessageAdmission,
    mut after_step: F,
) -> Result<CanonicalMessageAdmissionReceipt, SqliteAdapterError>
where
    F: FnMut(CanonicalMessageAdmissionStep) -> Result<(), SqliteAdapterError>,
{
    let topology = request.topology;
    let correlation_id = CorrelationId::for_work(request.candidates.work_id);
    let (device_id, client_message_id, inbound_delivery_id, journal_origin) = match request.origin {
        CanonicalMessageOrigin::Native {
            device_id,
            client_message_id,
        } => (
            Some(device_id),
            Some(client_message_id),
            None,
            MessageAcceptedOriginV2::Native {
                device_id,
                client_message_id,
            },
        ),
        CanonicalMessageOrigin::Channel {
            inbound_delivery_id,
        } => (
            None,
            None,
            Some(inbound_delivery_id),
            MessageAcceptedOriginV2::InboundDelivery {
                inbound_delivery_id,
            },
        ),
    };
    let message = Message::try_new(MessageInput {
        message_id: request.candidates.message_id,
        craxii_id: topology.craxii_id,
        conversation_id: topology.conversation_id,
        role: MessageRole::User,
        content: request.content,
        author_user_id: Some(topology.user_id),
        produced_by_work_id: None,
        device_id,
        client_message_id,
        inbound_delivery_id,
        committed_at: request.admitted_at,
    })
    .map_err(|_| invalid())?;
    let work = WorkItem::new(WorkItemInputData {
        work_id: request.candidates.work_id,
        craxii_id: topology.craxii_id,
        conversation_id: topology.conversation_id,
        conversation_work_ordinal: topology.next_ordinal,
        workspace_id: topology.workspace_id,
        reply_binding_id: request.reply_binding_id,
        correlation_id,
        created_at: request.admitted_at,
        queued_at: request.admitted_at,
    });
    let input = WorkItemInput::new(
        work.work_id(),
        request.candidates.acceptance_event_id,
        WorkInputRelationship::Trigger,
        WorkInputOrdinal::try_new(1).map_err(|_| invalid())?,
        request.admitted_at,
        WorkInputActor::User,
    );

    let (content_json, content_sha256) = encode_message_content(message.content())?;
    sqlx::query(
        "INSERT INTO messages (message_id, craxii_id, conversation_id, role, content_json, \
                content_sha256, author_user_id, produced_by_work_id, client_device_id, client_message_id, \
                inbound_delivery_id, committed_at) VALUES (?, ?, ?, 'user', ?, ?, ?, NULL, ?, ?, ?, ?)",
    )
    .bind(message.message_id().to_string())
    .bind(message.craxii_id().to_string())
    .bind(message.conversation_id().to_string())
    .bind(content_json)
    .bind(content_sha256.to_string())
    .bind(topology.user_id.to_string())
    .bind(device_id.map(|id| id.to_string()))
    .bind(client_message_id.map(|id| id.to_string()))
    .bind(inbound_delivery_id.map(|id| id.to_string()))
    .bind(request.admitted_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    after_step(CanonicalMessageAdmissionStep::MessageInserted)?;

    append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.candidates.acceptance_event_id,
            craxii_id: topology.craxii_id,
            stream_id: JournalStreamId::Conversation(topology.conversation_id),
            conversation_id: Some(topology.conversation_id),
            work_id: None,
            causation_event_id: Some(topology.conversation_created_event_id),
            correlation_id,
            actor: JournalActor::UserV2(topology.user_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::MessageAcceptedV2(MessageCommittedV2 {
                message_id: message.message_id(),
                craxii_id: message.craxii_id(),
                conversation_id: message.conversation_id(),
                role: message.role(),
                content: message.content().clone(),
                content_sha256: message.content_sha256(),
                author_user_id: topology.user_id,
                origin: journal_origin,
                committed_at: request.admitted_at,
            }),
            recorded_at: request.admitted_at,
            occurred_at: None,
        })?,
    )
    .await?;
    after_step(CanonicalMessageAdmissionStep::MessageAccepted)?;

    sqlx::query(
        "INSERT INTO work_items (work_id, craxii_id, conversation_id, \
                conversation_work_ordinal, kind, state, state_version, priority, workspace_id, \
                runtime_instance_id, current_model_invocation_id, current_tool_execution_id, \
                correlation_id, created_at, queued_at, started_at, cancel_requested_at, \
                cancellation_reason_code, terminal_at, terminal_reason_code, terminal_detail_json, \
                reply_binding_id) \
         VALUES (?, ?, ?, ?, 'conversational', 'queued', 1, 0, ?, NULL, NULL, NULL, ?, ?, ?, \
                 NULL, NULL, NULL, NULL, NULL, NULL, ?)",
    )
    .bind(work.work_id().to_string())
    .bind(work.craxii_id().to_string())
    .bind(work.conversation_id().to_string())
    .bind(work.conversation_work_ordinal().get())
    .bind(work.workspace_id().to_string())
    .bind(correlation_id.to_string())
    .bind(request.admitted_at.to_string())
    .bind(request.admitted_at.to_string())
    .bind(request.reply_binding_id.map(|id| id.to_string()))
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    after_step(CanonicalMessageAdmissionStep::WorkInserted)?;

    sqlx::query(
        "INSERT INTO work_item_inputs (work_id, input_event_id, relationship, \
                ordinal_within_work, attached_at, attached_by_actor) \
         VALUES (?, ?, 'trigger', 1, ?, 'user')",
    )
    .bind(input.work_id().to_string())
    .bind(input.input_event_id().to_string())
    .bind(input.attached_at().to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    after_step(CanonicalMessageAdmissionStep::WorkInputInserted)?;

    let queued_position = append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.candidates.queued_event_id,
            craxii_id: topology.craxii_id,
            stream_id: JournalStreamId::Work(work.work_id()),
            conversation_id: Some(topology.conversation_id),
            work_id: Some(work.work_id()),
            causation_event_id: Some(request.candidates.acceptance_event_id),
            correlation_id,
            actor: JournalActor::Craxii(topology.craxii_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::WorkQueuedV2(WorkQueuedV2 {
                work_id: work.work_id(),
                craxii_id: work.craxii_id(),
                conversation_id: work.conversation_id(),
                conversation_work_ordinal: work.conversation_work_ordinal(),
                kind: WorkKind::Conversational,
                priority: 0,
                workspace_id: work.workspace_id(),
                correlation_id,
                state_version: ProjectionVersion::try_new(1).map_err(|_| invalid())?,
                created_at: work.created_at(),
                queued_at: work.queued_at(),
                trigger: WorkInputFactV1 {
                    input_event_id: input.input_event_id(),
                    relationship: input.relationship(),
                    ordinal_within_work: input.ordinal_within_work(),
                    attached_at: input.attached_at(),
                    actor: input.actor(),
                },
                reply_binding_id: request.reply_binding_id,
            }),
            recorded_at: request.admitted_at,
            occurred_at: None,
        })?,
    )
    .await?;
    after_step(CanonicalMessageAdmissionStep::WorkQueued)?;

    advance_conversation_ordinal(
        transaction,
        topology.conversation_id,
        topology.conversation_version,
        topology.next_ordinal,
    )
    .await
    .map_err(map_projection_error)?;
    after_step(CanonicalMessageAdmissionStep::ConversationAdvanced)?;

    Ok(CanonicalMessageAdmissionReceipt {
        message_id: message.message_id(),
        work_id: work.work_id(),
        work_ordinal: work.conversation_work_ordinal(),
        committed_cursor: queued_position.offset,
    })
}
