//! Journal-valid multi-conversation fixtures used only by CH-3 tests.

use sqlx::Row;

use crate::domain::{
    ClientMessageId, ContentBlock, ConversationCreatedV2, ConversationId, ConversationKind,
    ConversationLifecycle, ConversationWorkOrdinal, CorrelationId, CraxiiId, DeviceId,
    JournalActor, JournalEventId, JournalEventPayload, JournalStreamId, MessageContent, MessageId,
    ProjectionVersion, UserId, UtcTimestamp, WorkId, WorkspaceId,
};

use super::error::SqliteAdapterError;
use super::error::SqliteFailureKind;
use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::message_admission::{
    CanonicalMessageAdmission, CanonicalMessageCandidates, CanonicalMessageOrigin,
    CanonicalMessageTopology, admit_canonical_message,
};
use super::runtime::SqliteRuntime;
use super::transaction::WriteTransaction;

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

#[derive(Clone, Copy)]
pub(super) struct TestConversation {
    pub craxii_id: CraxiiId,
    pub owner_user_id: UserId,
    pub conversation_id: ConversationId,
    pub workspace_id: WorkspaceId,
    pub created_event_id: JournalEventId,
}

pub(super) async fn create_conversation(
    runtime: &SqliteRuntime,
    craxii_id: CraxiiId,
    owner_user_id: UserId,
    create_owner: bool,
    created_at: UtcTimestamp,
) -> Result<TestConversation, SqliteAdapterError> {
    let conversation_id = ConversationId::generate();
    let created_event_id = JournalEventId::generate();
    let mut transaction = WriteTransaction::begin(runtime, "create_ch3_test_conversation").await?;
    let root = sqlx::query(
        "SELECT je.event_id, je.correlation_id, p.default_workspace_id \
         FROM craxii_principals p \
         JOIN journal_events je ON je.craxii_id = p.craxii_id \
          AND je.event_type = 'craxii.initialized' \
         WHERE p.craxii_id = ?",
    )
    .bind(craxii_id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let initialized_event_id =
        JournalEventId::parse_canonical(&root.try_get::<String, _>("event_id")?)
            .map_err(|_| inconsistent())?;
    let correlation_id =
        CorrelationId::parse_canonical(&root.try_get::<String, _>("correlation_id")?)
            .map_err(|_| inconsistent())?;
    let workspace_id =
        WorkspaceId::parse_canonical(&root.try_get::<String, _>("default_workspace_id")?)
            .map_err(|_| inconsistent())?;
    if create_owner {
        sqlx::query(
            "INSERT INTO users (user_id, craxii_id, lifecycle_state, created_at) \
             VALUES (?, ?, 'active', ?)",
        )
        .bind(owner_user_id.to_string())
        .bind(craxii_id.to_string())
        .bind(created_at.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    }
    sqlx::query(
        "INSERT INTO conversations \
         (conversation_id, craxii_id, owner_user_id, kind, lifecycle_state, created_at, \
          next_work_ordinal, state_version) \
         VALUES (?, ?, ?, 'primary', 'active', ?, 1, 1)",
    )
    .bind(conversation_id.to_string())
    .bind(craxii_id.to_string())
    .bind(owner_user_id.to_string())
    .bind(created_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: created_event_id,
            craxii_id,
            stream_id: JournalStreamId::Conversation(conversation_id),
            conversation_id: Some(conversation_id),
            work_id: None,
            causation_event_id: Some(initialized_event_id),
            correlation_id,
            actor: JournalActor::Craxii(craxii_id),
            runtime_instance_id: None,
            payload: JournalEventPayload::ConversationCreatedV2(ConversationCreatedV2 {
                conversation_id,
                craxii_id,
                owner_user_id,
                kind: ConversationKind::Primary,
                lifecycle: ConversationLifecycle::Active,
                next_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                state_version: ProjectionVersion::try_new(1).unwrap(),
                created_at,
            }),
            recorded_at: created_at,
            occurred_at: None,
        })?,
    )
    .await?;
    transaction.commit().await?;
    Ok(TestConversation {
        craxii_id,
        owner_user_id,
        conversation_id,
        workspace_id,
        created_event_id,
    })
}

pub(super) async fn admit_message(
    runtime: &SqliteRuntime,
    conversation: TestConversation,
    device_id: DeviceId,
    text: &str,
    admitted_at: UtcTimestamp,
) -> Result<WorkId, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(runtime, "admit_ch3_test_message").await?;
    let row = sqlx::query(
        "SELECT next_work_ordinal, state_version FROM conversations WHERE conversation_id = ?",
    )
    .bind(conversation.conversation_id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let work_id = WorkId::generate();
    let client_message_id =
        ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string())
            .expect("UUIDv7 is a valid client message id");
    let receipt = admit_canonical_message(
        &mut transaction,
        CanonicalMessageAdmission {
            topology: CanonicalMessageTopology {
                craxii_id: conversation.craxii_id,
                user_id: conversation.owner_user_id,
                conversation_id: conversation.conversation_id,
                workspace_id: conversation.workspace_id,
                next_ordinal: ConversationWorkOrdinal::try_new(row.try_get("next_work_ordinal")?)
                    .map_err(|_| inconsistent())?,
                conversation_version: ProjectionVersion::try_new(row.try_get("state_version")?)
                    .map_err(|_| inconsistent())?,
                conversation_created_event_id: conversation.created_event_id,
            },
            origin: CanonicalMessageOrigin::Native {
                device_id,
                client_message_id,
            },
            content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
            reply_binding_id: None,
            admitted_at,
            candidates: CanonicalMessageCandidates {
                message_id: MessageId::generate(),
                work_id,
                acceptance_event_id: JournalEventId::generate(),
                queued_event_id: JournalEventId::generate(),
            },
        },
        |_| Ok(()),
    )
    .await?;
    debug_assert_eq!(receipt.work_id, work_id);
    transaction.commit().await?;
    Ok(work_id)
}
