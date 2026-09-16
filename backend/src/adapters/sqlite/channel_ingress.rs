//! Atomic SQLite implementation of generic inbound classification and admission.

use sqlx::Row;

use crate::application::channel_ingress::{DurableInboundOutcome, VerifiedInboundPayload};
use crate::domain::{
    ConversationBindingId, ConversationId, ConversationWorkOrdinal, CraxiiId, ExternalIdentityId,
    InboundDeliveryId, JournalActor, JournalEvent, JournalEventId, JournalEventPayload,
    JournalOffset, JournalStreamId, MessageAcceptedOriginV2, MessageId, MessageRole,
    ProjectionVersion, Sha256Digest, UserId, WorkCancellationV2, WorkId, WorkState, WorkspaceId,
};
use crate::ports::channel_ingress::{
    ChannelIngressFuture, ChannelIngressStore, ChannelIngressStoreError,
    ChannelIngressStoreErrorKind, ClassifiedInbound, ClassifyInboundRequest,
    InboundPostCommitEffect,
};

use super::cancellation::{
    CancellationJournalOrigin, apply_cancellation_decision, decode_loaded_cancellation_work,
};
use super::codec::{decode_message_row, decode_timestamp};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::message_admission::{
    CanonicalMessageAdmission, CanonicalMessageCandidates, CanonicalMessageOrigin,
    CanonicalMessageTopology, admit_canonical_message,
};
use super::state_store::SqliteStateStore;
use super::transaction::WriteTransaction;

fn store_error(kind: ChannelIngressStoreErrorKind) -> ChannelIngressStoreError {
    ChannelIngressStoreError::new(kind)
}

fn inconsistent() -> ChannelIngressStoreError {
    store_error(ChannelIngressStoreErrorKind::Inconsistent)
}

fn contradictory() -> ChannelIngressStoreError {
    store_error(ChannelIngressStoreErrorKind::ContradictoryReplay)
}

fn invalid() -> ChannelIngressStoreError {
    store_error(ChannelIngressStoreErrorKind::Invalid)
}

fn map_sqlite(error: SqliteAdapterError) -> ChannelIngressStoreError {
    match error.kind() {
        SqliteFailureKind::Storage | SqliteFailureKind::BusyOrLocked => {
            store_error(ChannelIngressStoreErrorKind::Storage)
        }
        _ => inconsistent(),
    }
}

fn map_sqlx(error: sqlx::Error) -> ChannelIngressStoreError {
    map_sqlite(SqliteAdapterError::from_sqlx(error))
}

fn consistency_error() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

struct ExistingInbound {
    inbound_delivery_id: InboundDeliveryId,
    material_digest: Sha256Digest,
    receipt_state: String,
    classification: Option<String>,
    message_id: Option<MessageId>,
    work_id: Option<WorkId>,
    control_target_work_id: Option<WorkId>,
}

fn optional_id<T>(value: Option<String>) -> Result<Option<T>, ChannelIngressStoreError>
where
    T: std::str::FromStr,
{
    value
        .map(|value| value.parse().map_err(|_| inconsistent()))
        .transpose()
}

fn decode_existing(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ExistingInbound, ChannelIngressStoreError> {
    Ok(ExistingInbound {
        inbound_delivery_id: row
            .try_get::<String, _>("inbound_delivery_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        material_digest: Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("material_sha256")?,
        )
        .map_err(|_| inconsistent())?,
        receipt_state: row.try_get("receipt_state")?,
        classification: row.try_get("classification")?,
        message_id: optional_id(row.try_get("message_id")?)?,
        work_id: optional_id(row.try_get("work_id")?)?,
        control_target_work_id: optional_id(row.try_get("control_target_work_id")?)?,
    })
}

impl From<sqlx::Error> for ChannelIngressStoreError {
    fn from(error: sqlx::Error) -> Self {
        map_sqlx(error)
    }
}

async fn load_existing_by_event(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
) -> Result<Option<ExistingInbound>, ChannelIngressStoreError> {
    sqlx::query(
        "SELECT inbound_delivery_id, material_sha256, receipt_state, classification, \
                message_id, work_id, control_target_work_id \
         FROM inbound_deliveries WHERE channel_account_id = ? AND external_event_id = ?",
    )
    .bind(request.channel_account_id.to_string())
    .bind(request.external_event_id.as_str())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .as_ref()
    .map(decode_existing)
    .transpose()
}

async fn load_existing_by_message(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
) -> Result<Option<ExistingInbound>, ChannelIngressStoreError> {
    let Some(external_message_id) = &request.external_message_id else {
        return Ok(None);
    };
    sqlx::query(
        "SELECT inbound_delivery_id, material_sha256, receipt_state, classification, \
                message_id, work_id, control_target_work_id \
         FROM inbound_deliveries \
         WHERE channel_account_id = ? AND external_conversation_id = ? \
           AND COALESCE(external_thread_id, '') = COALESCE(?, '') AND external_message_id = ?",
    )
    .bind(request.channel_account_id.to_string())
    .bind(request.external_conversation_id.as_str())
    .bind(request.external_thread_id.as_ref().map(|id| id.as_str()))
    .bind(external_message_id.as_str())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .as_ref()
    .map(decode_existing)
    .transpose()
}

async fn message_cursor(
    transaction: &mut WriteTransaction,
    work_id: WorkId,
) -> Result<JournalOffset, ChannelIngressStoreError> {
    let rows = sqlx::query_scalar::<_, i64>(
        "SELECT journal_offset FROM journal_events \
         WHERE stream_id = ? AND event_type = 'work.queued' ORDER BY stream_seq ASC",
    )
    .bind(JournalStreamId::Work(work_id).to_string())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let [offset] = rows.as_slice() else {
        return Err(inconsistent());
    };
    JournalOffset::try_new(*offset).map_err(|_| inconsistent())
}

async fn duplicate_outcome(
    transaction: &mut WriteTransaction,
    existing: ExistingInbound,
    material_digest: Sha256Digest,
) -> Result<DurableInboundOutcome, ChannelIngressStoreError> {
    if existing.material_digest != material_digest {
        return Err(contradictory());
    }
    if existing.receipt_state != "classified" {
        return Err(inconsistent());
    }
    match existing.classification.as_deref() {
        Some("message") => {
            let (Some(message_id), Some(work_id), None) = (
                existing.message_id,
                existing.work_id,
                existing.control_target_work_id,
            ) else {
                return Err(inconsistent());
            };
            let row =
                sqlx::query("SELECT conversation_work_ordinal FROM work_items WHERE work_id = ?")
                    .bind(work_id.to_string())
                    .fetch_optional(transaction.connection())
                    .await
                    .map_err(map_sqlx)?
                    .ok_or_else(inconsistent)?;
            let work_ordinal =
                ConversationWorkOrdinal::try_new(row.try_get("conversation_work_ordinal")?)
                    .map_err(|_| inconsistent())?;
            Ok(DurableInboundOutcome::MessageAccepted {
                inbound_delivery_id: existing.inbound_delivery_id,
                message_id,
                work_id,
                work_ordinal,
                committed_cursor: message_cursor(transaction, work_id).await?,
            })
        }
        Some("control") => {
            if existing.message_id.is_some() || existing.work_id.is_some() {
                return Err(inconsistent());
            }
            Ok(match existing.control_target_work_id {
                Some(target_work_id) => DurableInboundOutcome::ControlApplied {
                    inbound_delivery_id: existing.inbound_delivery_id,
                    target_work_id,
                },
                None => DurableInboundOutcome::ControlNoOp {
                    inbound_delivery_id: existing.inbound_delivery_id,
                },
            })
        }
        Some("unsupported")
            if existing.message_id.is_none()
                && existing.work_id.is_none()
                && existing.control_target_work_id.is_none() =>
        {
            Ok(DurableInboundOutcome::Unsupported {
                inbound_delivery_id: existing.inbound_delivery_id,
            })
        }
        Some("rejected")
            if existing.message_id.is_none()
                && existing.work_id.is_none()
                && existing.control_target_work_id.is_none() =>
        {
            Ok(DurableInboundOutcome::Rejected {
                inbound_delivery_id: existing.inbound_delivery_id,
            })
        }
        _ => Err(inconsistent()),
    }
}

struct AccountTopology {
    craxii_id: CraxiiId,
    primary_conversation_id: ConversationId,
    workspace_id: WorkspaceId,
    active: bool,
}

async fn load_account(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
) -> Result<AccountTopology, ChannelIngressStoreError> {
    let row = sqlx::query(
        "SELECT a.craxii_id, a.lifecycle_state, p.craxii_id AS principal_craxii_id, \
                p.primary_conversation_id, p.default_workspace_id \
         FROM channel_accounts a \
         LEFT JOIN craxii_principals p ON p.craxii_id = a.craxii_id \
         WHERE a.channel_account_id = ?",
    )
    .bind(request.channel_account_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .ok_or_else(inconsistent)?;
    let craxii_id: CraxiiId = row
        .try_get::<String, _>("craxii_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let principal: Option<String> = row.try_get("principal_craxii_id")?;
    if principal
        .as_deref()
        .and_then(|value| value.parse::<CraxiiId>().ok())
        != Some(craxii_id)
    {
        return Err(inconsistent());
    }
    let active = match row.try_get::<String, _>("lifecycle_state")?.as_str() {
        "active" => true,
        "disabled" => false,
        _ => return Err(inconsistent()),
    };
    Ok(AccountTopology {
        craxii_id,
        primary_conversation_id: row
            .try_get::<String, _>("primary_conversation_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        workspace_id: row
            .try_get::<String, _>("default_workspace_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        active,
    })
}

async fn insert_received(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
    craxii_id: CraxiiId,
) -> Result<(), ChannelIngressStoreError> {
    if matches!(request.payload, VerifiedInboundPayload::Text(_))
        && request.external_message_id.is_none()
    {
        return Err(invalid());
    }
    sqlx::query(
        "INSERT INTO inbound_deliveries \
         (inbound_delivery_id, channel_account_id, craxii_id, external_event_id, \
          external_message_id, external_subject_id, external_conversation_id, external_thread_id, \
          material_sha256, provider_occurred_at, received_at, receipt_state, classification, \
          external_identity_id, conversation_binding_id, user_id, conversation_id, message_id, \
          work_id, control_target_work_id, classified_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'received', NULL, NULL, NULL, NULL, NULL, NULL, \
                 NULL, NULL, NULL)",
    )
    .bind(request.candidates.inbound_delivery_id.to_string())
    .bind(request.channel_account_id.to_string())
    .bind(craxii_id.to_string())
    .bind(request.external_event_id.as_str())
    .bind(request.external_message_id.as_ref().map(|id| id.as_str()))
    .bind(request.sender_subject_id.as_str())
    .bind(request.external_conversation_id.as_str())
    .bind(request.external_thread_id.as_ref().map(|id| id.as_str()))
    .bind(request.material_digest.to_string())
    .bind(request.provider_occurred_at.map(|at| at.to_string()))
    .bind(request.observed_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    Ok(())
}

async fn classify_without_resolution(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
    classification: &'static str,
) -> Result<(), ChannelIngressStoreError> {
    let affected = sqlx::query(
        "UPDATE inbound_deliveries SET receipt_state = 'classified', classification = ?, \
                classified_at = ? WHERE inbound_delivery_id = ? AND receipt_state = 'received'",
    )
    .bind(classification)
    .bind(request.observed_at.to_string())
    .bind(request.candidates.inbound_delivery_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if affected == 1 {
        Ok(())
    } else {
        Err(inconsistent())
    }
}

struct AuthorizedTopology {
    external_identity_id: ExternalIdentityId,
    binding_id: ConversationBindingId,
    user_id: UserId,
    conversation_id: ConversationId,
    next_ordinal: ConversationWorkOrdinal,
    conversation_version: ProjectionVersion,
    conversation_created_event_id: JournalEventId,
}

async fn resolve_authorized_topology(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
    account: &AccountTopology,
) -> Result<Option<AuthorizedTopology>, ChannelIngressStoreError> {
    let identities = sqlx::query(
        "SELECT i.external_identity_id, i.craxii_id, i.user_id, i.lifecycle_state, \
                u.craxii_id AS user_craxii_id, u.lifecycle_state AS user_lifecycle \
         FROM external_identities i LEFT JOIN users u ON u.user_id = i.user_id \
         WHERE i.channel_account_id = ? AND i.external_subject_id = ? \
         ORDER BY i.external_identity_id ASC",
    )
    .bind(request.channel_account_id.to_string())
    .bind(request.sender_subject_id.as_str())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let active_identities = identities
        .iter()
        .filter(|row| row.try_get::<String, _>("lifecycle_state").ok().as_deref() == Some("active"))
        .collect::<Vec<_>>();
    let [identity] = active_identities.as_slice() else {
        if active_identities.is_empty() {
            return Ok(None);
        }
        return Err(inconsistent());
    };
    let external_identity_id: ExternalIdentityId = identity
        .try_get::<String, _>("external_identity_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let user_id: UserId = identity
        .try_get::<String, _>("user_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let identity_craxii: CraxiiId = identity
        .try_get::<String, _>("craxii_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let user_craxii: Option<String> = identity.try_get("user_craxii_id")?;
    if identity_craxii != account.craxii_id
        || user_craxii
            .as_deref()
            .and_then(|value| value.parse::<CraxiiId>().ok())
            != Some(account.craxii_id)
        || identity
            .try_get::<Option<String>, _>("user_lifecycle")?
            .as_deref()
            != Some("active")
    {
        return Err(inconsistent());
    }

    let bindings = sqlx::query(
        "SELECT * FROM conversation_bindings \
         WHERE channel_account_id = ? AND external_conversation_id = ? \
           AND COALESCE(external_thread_id, '') = COALESCE(?, '') \
           AND lifecycle_state = 'active' ORDER BY conversation_binding_id ASC",
    )
    .bind(request.channel_account_id.to_string())
    .bind(request.external_conversation_id.as_str())
    .bind(request.external_thread_id.as_ref().map(|id| id.as_str()))
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let [binding] = bindings.as_slice() else {
        if bindings.is_empty() {
            return Ok(None);
        }
        return Err(inconsistent());
    };
    let binding_identity: ExternalIdentityId = binding
        .try_get::<String, _>("external_identity_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let binding_user: UserId = binding
        .try_get::<String, _>("user_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let binding_craxii: CraxiiId = binding
        .try_get::<String, _>("craxii_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    if binding_craxii != account.craxii_id {
        return Err(inconsistent());
    }
    if binding_identity != external_identity_id || binding_user != user_id {
        return Ok(None);
    }
    let conversation_id: ConversationId = binding
        .try_get::<String, _>("conversation_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let binding_id: ConversationBindingId = binding
        .try_get::<String, _>("conversation_binding_id")?
        .parse()
        .map_err(|_| inconsistent())?;
    let conversation = sqlx::query(
        "SELECT c.craxii_id, c.owner_user_id, c.kind, c.lifecycle_state, \
                c.next_work_ordinal, c.state_version, w.craxii_id AS workspace_craxii_id \
         FROM conversations c LEFT JOIN workspaces w ON w.workspace_id = ? \
         WHERE c.conversation_id = ?",
    )
    .bind(account.workspace_id.to_string())
    .bind(conversation_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .ok_or_else(inconsistent)?;
    if conversation
        .try_get::<String, _>("craxii_id")?
        .parse::<CraxiiId>()
        .map_err(|_| inconsistent())?
        != account.craxii_id
        || conversation
            .try_get::<String, _>("owner_user_id")?
            .parse::<UserId>()
            .map_err(|_| inconsistent())?
            != user_id
        || conversation.try_get::<String, _>("kind")? != "primary"
        || conversation.try_get::<String, _>("lifecycle_state")? != "active"
        || conversation
            .try_get::<Option<String>, _>("workspace_craxii_id")?
            .as_deref()
            .and_then(|value| value.parse::<CraxiiId>().ok())
            != Some(account.craxii_id)
    {
        return Err(inconsistent());
    }
    if conversation_id != account.primary_conversation_id {
        return Ok(None);
    }
    let created = sqlx::query_scalar::<_, String>(
        "SELECT event_id FROM journal_events \
         WHERE stream_id = ? AND event_type = 'conversation.created'",
    )
    .bind(JournalStreamId::Conversation(conversation_id).to_string())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let [created_event_id] = created.as_slice() else {
        return Err(inconsistent());
    };
    Ok(Some(AuthorizedTopology {
        external_identity_id,
        binding_id,
        user_id,
        conversation_id,
        next_ordinal: ConversationWorkOrdinal::try_new(conversation.try_get("next_work_ordinal")?)
            .map_err(|_| inconsistent())?,
        conversation_version: ProjectionVersion::try_new(conversation.try_get("state_version")?)
            .map_err(|_| inconsistent())?,
        conversation_created_event_id: created_event_id.parse().map_err(|_| inconsistent())?,
    }))
}

async fn classify_message(
    transaction: &mut WriteTransaction,
    request: ClassifyInboundRequest,
    account: AccountTopology,
    authorized: AuthorizedTopology,
    content: crate::domain::MessageContent,
) -> Result<ClassifiedInbound, ChannelIngressStoreError> {
    let affected = sqlx::query(
        "UPDATE inbound_deliveries SET receipt_state = 'classified', classification = 'message', \
                external_identity_id = ?, conversation_binding_id = ?, user_id = ?, \
                conversation_id = ?, message_id = ?, work_id = ?, classified_at = ? \
         WHERE inbound_delivery_id = ? AND receipt_state = 'received'",
    )
    .bind(authorized.external_identity_id.to_string())
    .bind(authorized.binding_id.to_string())
    .bind(authorized.user_id.to_string())
    .bind(authorized.conversation_id.to_string())
    .bind(request.candidates.message_id.to_string())
    .bind(request.candidates.work_id.to_string())
    .bind(request.observed_at.to_string())
    .bind(request.candidates.inbound_delivery_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if affected != 1 {
        return Err(inconsistent());
    }
    let admission = admit_canonical_message(
        transaction,
        CanonicalMessageAdmission {
            topology: CanonicalMessageTopology {
                craxii_id: account.craxii_id,
                user_id: authorized.user_id,
                conversation_id: authorized.conversation_id,
                workspace_id: account.workspace_id,
                next_ordinal: authorized.next_ordinal,
                conversation_version: authorized.conversation_version,
                conversation_created_event_id: authorized.conversation_created_event_id,
            },
            origin: CanonicalMessageOrigin::Channel {
                inbound_delivery_id: request.candidates.inbound_delivery_id,
            },
            content,
            reply_binding_id: Some(authorized.binding_id),
            admitted_at: request.observed_at,
            candidates: CanonicalMessageCandidates {
                message_id: request.candidates.message_id,
                work_id: request.candidates.work_id,
                acceptance_event_id: request.candidates.acceptance_event_id,
                queued_event_id: request.candidates.queued_event_id,
            },
        },
        |_| Ok(()),
    )
    .await
    .map_err(map_sqlite)?;
    Ok(ClassifiedInbound::newly(
        DurableInboundOutcome::MessageAccepted {
            inbound_delivery_id: request.candidates.inbound_delivery_id,
            message_id: admission.message_id,
            work_id: admission.work_id,
            work_ordinal: admission.work_ordinal,
            committed_cursor: admission.committed_cursor,
        },
        InboundPostCommitEffect::MessageCommitted {
            work_id: admission.work_id,
            cursor: admission.committed_cursor,
        },
    ))
}

async fn select_control_target(
    transaction: &mut WriteTransaction,
    conversation_id: ConversationId,
) -> Result<Option<sqlx::sqlite::SqliteRow>, ChannelIngressStoreError> {
    let active = sqlx::query(
        "SELECT * FROM work_items WHERE conversation_id = ? \
         AND state IN ('running', 'waiting_on_model', 'waiting_on_tool', 'cancel_requested') \
         ORDER BY conversation_work_ordinal ASC, work_id ASC",
    )
    .bind(conversation_id.to_string())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    match active.len() {
        0 => {}
        1 => return Ok(active.into_iter().next()),
        _ => return Err(inconsistent()),
    }
    sqlx::query(
        "SELECT * FROM work_items WHERE conversation_id = ? AND state = 'queued' \
         ORDER BY conversation_work_ordinal ASC, work_id ASC LIMIT 1",
    )
    .bind(conversation_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)
}

async fn classify_control(
    transaction: &mut WriteTransaction,
    request: &ClassifyInboundRequest,
    authorized: &AuthorizedTopology,
) -> Result<ClassifiedInbound, ChannelIngressStoreError> {
    let target = select_control_target(transaction, authorized.conversation_id).await?;
    let target_work_id = target
        .as_ref()
        .map(|row| {
            row.try_get::<String, _>("work_id")?
                .parse::<WorkId>()
                .map_err(|_| inconsistent())
        })
        .transpose()?;
    let affected = sqlx::query(
        "UPDATE inbound_deliveries SET receipt_state = 'classified', classification = 'control', \
                external_identity_id = ?, conversation_binding_id = ?, user_id = ?, \
                conversation_id = ?, control_target_work_id = ?, classified_at = ? \
         WHERE inbound_delivery_id = ? AND receipt_state = 'received'",
    )
    .bind(authorized.external_identity_id.to_string())
    .bind(authorized.binding_id.to_string())
    .bind(authorized.user_id.to_string())
    .bind(authorized.conversation_id.to_string())
    .bind(target_work_id.map(|id| id.to_string()))
    .bind(request.observed_at.to_string())
    .bind(request.candidates.inbound_delivery_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if affected != 1 {
        return Err(inconsistent());
    }
    let Some(row) = target else {
        return Ok(ClassifiedInbound::newly(
            DurableInboundOutcome::ControlNoOp {
                inbound_delivery_id: request.candidates.inbound_delivery_id,
            },
            InboundPostCommitEffect::None,
        ));
    };
    let work_id = target_work_id.ok_or_else(inconsistent)?;
    let work = decode_loaded_cancellation_work(&row, work_id).map_err(map_sqlite)?;
    if work.conversation_id != authorized.conversation_id {
        return Err(inconsistent());
    }
    let mutation = apply_cancellation_decision(
        transaction,
        &work,
        request.observed_at,
        request.candidates.cancellation_event_id,
        CancellationJournalOrigin::Channel {
            user_id: authorized.user_id,
            inbound_delivery_id: request.candidates.inbound_delivery_id,
        },
    )
    .await
    .map_err(map_sqlite)?;
    let effect = match (mutation.resulting_state, mutation.committed_cursor) {
        (WorkState::CancelRequested, Some(cursor)) => {
            InboundPostCommitEffect::ActiveCancellationCommitted { work_id, cursor }
        }
        (WorkState::Cancelled, Some(cursor)) => {
            InboundPostCommitEffect::DirectCancellationCommitted { work_id, cursor }
        }
        (WorkState::CancelRequested, None) => InboundPostCommitEffect::None,
        _ => return Err(inconsistent()),
    };
    Ok(ClassifiedInbound::newly(
        DurableInboundOutcome::ControlApplied {
            inbound_delivery_id: request.candidates.inbound_delivery_id,
            target_work_id: work_id,
        },
        effect,
    ))
}

async fn classify_inner(
    store: &SqliteStateStore,
    request: ClassifyInboundRequest,
) -> Result<ClassifiedInbound, ChannelIngressStoreError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "classify_channel_inbound")
        .await
        .map_err(map_sqlite)?;
    let primary = load_existing_by_event(&mut transaction, &request).await?;
    let secondary = load_existing_by_message(&mut transaction, &request).await?;
    let existing = match (primary, secondary) {
        (Some(primary), Some(secondary))
            if primary.inbound_delivery_id != secondary.inbound_delivery_id =>
        {
            return Err(inconsistent());
        }
        (Some(primary), _) => Some(primary),
        (_, Some(secondary)) => Some(secondary),
        (None, None) => None,
    };
    if let Some(existing) = existing {
        let outcome =
            duplicate_outcome(&mut transaction, existing, request.material_digest).await?;
        transaction.commit().await.map_err(map_sqlite)?;
        return Ok(ClassifiedInbound::duplicate(outcome));
    }

    let account = load_account(&mut transaction, &request).await?;
    insert_received(&mut transaction, &request, account.craxii_id).await?;
    if !account.active {
        classify_without_resolution(&mut transaction, &request, "rejected").await?;
        transaction.commit().await.map_err(map_sqlite)?;
        return Ok(ClassifiedInbound::newly(
            DurableInboundOutcome::Rejected {
                inbound_delivery_id: request.candidates.inbound_delivery_id,
            },
            InboundPostCommitEffect::None,
        ));
    }
    let Some(authorized) =
        resolve_authorized_topology(&mut transaction, &request, &account).await?
    else {
        classify_without_resolution(&mut transaction, &request, "rejected").await?;
        transaction.commit().await.map_err(map_sqlite)?;
        return Ok(ClassifiedInbound::newly(
            DurableInboundOutcome::Rejected {
                inbound_delivery_id: request.candidates.inbound_delivery_id,
            },
            InboundPostCommitEffect::None,
        ));
    };

    let classified = match request.payload.clone() {
        VerifiedInboundPayload::Unsupported(_) => {
            classify_without_resolution(&mut transaction, &request, "unsupported").await?;
            ClassifiedInbound::newly(
                DurableInboundOutcome::Unsupported {
                    inbound_delivery_id: request.candidates.inbound_delivery_id,
                },
                InboundPostCommitEffect::None,
            )
        }
        VerifiedInboundPayload::Text(_) if request.is_control => {
            classify_control(&mut transaction, &request, &authorized).await?
        }
        VerifiedInboundPayload::Text(content) => {
            classify_message(&mut transaction, request, account, authorized, content).await?
        }
    };
    transaction.commit().await.map_err(map_sqlite)?;
    Ok(classified)
}

impl ChannelIngressStore for SqliteStateStore {
    fn classify_inbound(&self, request: ClassifyInboundRequest) -> ChannelIngressFuture<'_> {
        Box::pin(async move { classify_inner(self, request).await })
    }
}

fn channel_cancellation(event: &JournalEvent) -> Option<&WorkCancellationV2> {
    match &event.payload {
        JournalEventPayload::WorkCancelRequestedV2(value)
        | JournalEventPayload::WorkCancelledV2(value) => Some(value),
        _ => None,
    }
}

fn any_cancel_requested(event: &JournalEvent, work_id: WorkId) -> bool {
    match &event.payload {
        JournalEventPayload::WorkCancelRequested(value) => value.work_id == work_id,
        JournalEventPayload::WorkCancelRequestedV2(value) => value.transition.work_id == work_id,
        _ => false,
    }
}

async fn verify_message_classification(
    connection: &mut sqlx::SqliteConnection,
    row: &sqlx::sqlite::SqliteRow,
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    let inbound_delivery_id =
        InboundDeliveryId::parse_canonical(&row.try_get::<String, _>("inbound_delivery_id")?)
            .map_err(|_| consistency_error())?;
    let user_id = UserId::parse_canonical(&row.try_get::<String, _>("user_id")?)
        .map_err(|_| consistency_error())?;
    let conversation_id =
        ConversationId::parse_canonical(&row.try_get::<String, _>("conversation_id")?)
            .map_err(|_| consistency_error())?;
    let binding_id = ConversationBindingId::parse_canonical(
        &row.try_get::<String, _>("conversation_binding_id")?,
    )
    .map_err(|_| consistency_error())?;
    let message_id = MessageId::parse_canonical(&row.try_get::<String, _>("message_id")?)
        .map_err(|_| consistency_error())?;
    let work_id = WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
        .map_err(|_| consistency_error())?;

    let message_row = sqlx::query("SELECT * FROM messages WHERE message_id = ?")
        .bind(message_id.to_string())
        .fetch_optional(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .ok_or_else(consistency_error)?;
    let message = decode_message_row(&message_row)?;
    if message.role() != MessageRole::User
        || message.conversation_id() != conversation_id
        || message.author_user_id() != Some(user_id)
        || message.inbound_delivery_id() != Some(inbound_delivery_id)
        || message.device_id().is_some()
        || message.client_message_id().is_some()
    {
        return Err(consistency_error());
    }

    let work_row = sqlx::query(
        "SELECT conversation_id, correlation_id, reply_binding_id \
         FROM work_items WHERE work_id = ?",
    )
    .bind(work_id.to_string())
    .fetch_optional(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(consistency_error)?;
    let correlation_id = crate::domain::CorrelationId::parse_canonical(
        &work_row.try_get::<String, _>("correlation_id")?,
    )
    .map_err(|_| consistency_error())?;
    if work_row.try_get::<String, _>("conversation_id")? != conversation_id.to_string()
        || work_row
            .try_get::<Option<String>, _>("reply_binding_id")?
            .as_deref()
            != Some(binding_id.to_string().as_str())
        || correlation_id != crate::domain::CorrelationId::for_work(work_id)
    {
        return Err(consistency_error());
    }

    let input_rows = sqlx::query(
        "SELECT input_event_id, relationship, ordinal_within_work, attached_by_actor \
         FROM work_item_inputs WHERE work_id = ?",
    )
    .bind(work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let [input] = input_rows.as_slice() else {
        return Err(consistency_error());
    };
    let accepted_event_id =
        JournalEventId::parse_canonical(&input.try_get::<String, _>("input_event_id")?)
            .map_err(|_| consistency_error())?;
    let accepted = events
        .iter()
        .find(|event| event.event_id == accepted_event_id)
        .ok_or_else(consistency_error)?;
    let JournalEventPayload::MessageAcceptedV2(payload) = &accepted.payload else {
        return Err(consistency_error());
    };
    let accepted_cause = accepted
        .causation_event_id
        .and_then(|id| events.iter().find(|event| event.event_id == id))
        .ok_or_else(consistency_error)?;
    if payload.message_id != message_id
        || payload.craxii_id != message.craxii_id()
        || payload.conversation_id != conversation_id
        || payload.author_user_id != user_id
        || payload.content != *message.content()
        || payload.origin
            != (MessageAcceptedOriginV2::InboundDelivery {
                inbound_delivery_id,
            })
        || accepted.actor != JournalActor::UserV2(user_id)
        || accepted.stream_id != JournalStreamId::Conversation(conversation_id)
        || accepted.conversation_id != Some(conversation_id)
        || accepted.work_id.is_some()
        || accepted.correlation_id != correlation_id
        || accepted_cause.stream_id != JournalStreamId::Conversation(conversation_id)
        || !matches!(
            accepted_cause.payload,
            JournalEventPayload::ConversationCreated(_)
                | JournalEventPayload::ConversationCreatedV2(_)
        )
        || input.try_get::<String, _>("relationship")? != "trigger"
        || input.try_get::<i64, _>("ordinal_within_work")? != 1
        || input.try_get::<String, _>("attached_by_actor")? != "user"
    {
        return Err(consistency_error());
    }

    let queued = events
        .iter()
        .filter(|event| event.stream_id == JournalStreamId::Work(work_id))
        .find(|event| matches!(event.payload, JournalEventPayload::WorkQueuedV2(_)))
        .ok_or_else(consistency_error)?;
    let JournalEventPayload::WorkQueuedV2(payload) = &queued.payload else {
        return Err(consistency_error());
    };
    if payload.work_id != work_id
        || payload.craxii_id != message.craxii_id()
        || payload.conversation_id != conversation_id
        || payload.correlation_id != correlation_id
        || payload.reply_binding_id != Some(binding_id)
        || payload.trigger.input_event_id != accepted_event_id
        || queued.actor != JournalActor::Craxii(payload.craxii_id)
        || queued.conversation_id != Some(conversation_id)
        || queued.work_id != Some(work_id)
        || queued.causation_event_id != Some(accepted_event_id)
        || queued.correlation_id != correlation_id
    {
        return Err(consistency_error());
    }
    Ok(())
}

fn verify_channel_cancellation_event(
    event: &JournalEvent,
    cancellation: &WorkCancellationV2,
    inbound_delivery_id: InboundDeliveryId,
    user_id: UserId,
    conversation_id: ConversationId,
    target_work_id: WorkId,
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    let transition = &cancellation.transition;
    let previous = events
        .iter()
        .find(|candidate| {
            candidate.stream_id == event.stream_id
                && candidate
                    .stream_seq
                    .get()
                    .checked_add(1)
                    .is_some_and(|next| next == event.stream_seq.get())
        })
        .ok_or_else(consistency_error)?;
    if cancellation.inbound_delivery_id != inbound_delivery_id
        || transition.work_id != target_work_id
        || event.actor != JournalActor::UserV2(user_id)
        || event.stream_id != JournalStreamId::Work(target_work_id)
        || event.work_id != Some(target_work_id)
        || event.conversation_id != Some(conversation_id)
        || event.correlation_id != crate::domain::CorrelationId::for_work(target_work_id)
        || event.causation_event_id != Some(previous.event_id)
        || event.recorded_at != transition.transitioned_at
        || event.occurred_at.is_some()
        || !super::stage9::exact_stage9_cancellation_shape(transition)
    {
        return Err(consistency_error());
    }
    Ok(())
}

async fn verify_control_classification(
    connection: &mut sqlx::SqliteConnection,
    row: &sqlx::sqlite::SqliteRow,
    events: &[JournalEvent],
) -> Result<Option<InboundDeliveryId>, SqliteAdapterError> {
    let inbound_delivery_id =
        InboundDeliveryId::parse_canonical(&row.try_get::<String, _>("inbound_delivery_id")?)
            .map_err(|_| consistency_error())?;
    let user_id = UserId::parse_canonical(&row.try_get::<String, _>("user_id")?)
        .map_err(|_| consistency_error())?;
    let conversation_id =
        ConversationId::parse_canonical(&row.try_get::<String, _>("conversation_id")?)
            .map_err(|_| consistency_error())?;
    let target_work_id = row
        .try_get::<Option<String>, _>("control_target_work_id")?
        .map(|value| WorkId::parse_canonical(&value).map_err(|_| consistency_error()))
        .transpose()?;
    let matching = events
        .iter()
        .filter_map(|event| channel_cancellation(event).map(|value| (event, value)))
        .filter(|(_, value)| value.inbound_delivery_id == inbound_delivery_id)
        .collect::<Vec<_>>();
    let Some(target_work_id) = target_work_id else {
        return if matching.is_empty() {
            Ok(None)
        } else {
            Err(consistency_error())
        };
    };
    let work_conversation: String =
        sqlx::query_scalar("SELECT conversation_id FROM work_items WHERE work_id = ?")
            .bind(target_work_id.to_string())
            .fetch_optional(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?
            .ok_or_else(consistency_error)?;
    if work_conversation != conversation_id.to_string() {
        return Err(consistency_error());
    }
    match matching.as_slice() {
        [(event, cancellation)] => {
            verify_channel_cancellation_event(
                event,
                cancellation,
                inbound_delivery_id,
                user_id,
                conversation_id,
                target_work_id,
                events,
            )?;
            Ok(Some(inbound_delivery_id))
        }
        [] => {
            let classified_at = decode_timestamp(&row.try_get::<String, _>("classified_at")?)?;
            if events.iter().any(|event| {
                event.recorded_at <= classified_at
                    && event.work_id == Some(target_work_id)
                    && any_cancel_requested(event, target_work_id)
            }) {
                Ok(None)
            } else {
                Err(consistency_error())
            }
        }
        _ => Err(consistency_error()),
    }
}

pub(super) async fn verify_channel_ingress_consistency(
    connection: &mut sqlx::SqliteConnection,
    events: &[JournalEvent],
) -> Result<u64, SqliteAdapterError> {
    let rows = sqlx::query(
        "SELECT inbound_delivery_id, classification, user_id, conversation_binding_id, \
                conversation_id, message_id, work_id, control_target_work_id, classified_at \
         FROM inbound_deliveries WHERE receipt_state = 'classified'",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let mut linked_cancellations = std::collections::HashSet::new();
    for row in &rows {
        match row.try_get::<String, _>("classification")?.as_str() {
            "message" => verify_message_classification(connection, row, events).await?,
            "control" => {
                if let Some(id) = verify_control_classification(connection, row, events).await? {
                    linked_cancellations.insert(id);
                }
            }
            "unsupported" | "rejected" => {}
            _ => return Err(consistency_error()),
        }
    }
    for cancellation in events.iter().filter_map(channel_cancellation) {
        if !linked_cancellations.contains(&cancellation.inbound_delivery_id) {
            return Err(consistency_error());
        }
    }
    Ok(8)
}
