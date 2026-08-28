use std::collections::HashMap;

use sqlx::Row;

use crate::bootstrap::compatibility::PROTOCOL_VERSION;
use crate::domain::{
    CancellationCheckpoint, CancellationCleanupDisposition, CancellationCommandReceipt,
    CancellationDecision, ClientCommandId, ClientMessageId, CommandHashEncodingVersion,
    CommandKind, CommandOutcome, CommandRequestHash, ConversationId, ConversationWorkOrdinal,
    CorrelationId, CraxiiId, CurrentWorkAttempt, DeviceId, DeviceTokenHash, IdempotencyKey,
    JournalActor, JournalCurrentAttempt, JournalEvent, JournalEventId, JournalEventPayload,
    JournalOffset, JournalStreamId, JournalWorkTerminalReason, Message, MessageCommandReceipt,
    MessageCommittedV1, MessageInput, MessageRole, ModelInvocationId, ProjectionVersion,
    RuntimeInstanceId, ToolExecutionId, UtcTimestamp, WorkCancellationReason, WorkCompletionReason,
    WorkFailureReason, WorkId, WorkInputActor, WorkInputFactV1, WorkInputOrdinal,
    WorkInputRelationship, WorkInterruptionReason, WorkItem, WorkItemInput, WorkItemInputData,
    WorkKind, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput, WorkQueuedV1, WorkState,
    WorkTerminalReason, WorkTransitionV1, WorkspaceId, decide_cancellation,
};
use crate::ports::device_credentials::{
    DeviceCredentialFuture, DeviceCredentialMatch, DeviceCredentialStore,
    DeviceCredentialStoreError, DeviceCredentialStoreErrorKind, DeviceSummary,
    ProvisionDeviceIntent, RevokeDeviceOutcome,
};
use crate::ports::state_store::{
    AcceptUserMessageRequest, CommandStateStore, RequestCancellationRequest, StateStoreFuture,
};

use super::codec::{
    decode_cancellation_reason, decode_message_row, decode_optional_id, decode_optional_timestamp,
    decode_timestamp, decode_work_state, encode_message_content,
};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::projection::{
    ProjectionMutationError, WorkProjectionTimes, advance_conversation_ordinal, guarded_work_update,
};
use super::stage9_codec::{
    decode_cancellation_receipt, decode_client_command_row, decode_message_receipt,
    encode_cancellation_receipt, encode_message_receipt,
};
use super::state_store::{SqliteStateStore, map_port_error};
use super::transaction::WriteTransaction;

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

fn invalid() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InternalInvariant)
}

fn idempotency_conflict() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::IdempotencyConflict)
}

fn target_not_found() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::TargetNotFound)
}

fn map_projection_error<C>(error: ProjectionMutationError<C>) -> SqliteAdapterError {
    match error {
        ProjectionMutationError::Conflict(_) => inconsistent(),
        ProjectionMutationError::Storage(error) => error,
        ProjectionMutationError::Invariant => invalid(),
    }
}

fn map_device_store_error(error: SqliteAdapterError) -> DeviceCredentialStoreError {
    let kind = match error.kind() {
        SqliteFailureKind::IdempotencyConflict | SqliteFailureKind::StateConflict => {
            DeviceCredentialStoreErrorKind::Conflict
        }
        SqliteFailureKind::InternalInvariant | SqliteFailureKind::InconsistentSchema => {
            DeviceCredentialStoreErrorKind::Inconsistent
        }
        _ => DeviceCredentialStoreErrorKind::Storage,
    };
    DeviceCredentialStoreError::new(kind)
}

fn decode_device_summary(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DeviceSummary, SqliteAdapterError> {
    let created_at = decode_timestamp(&row.try_get::<String, _>("created_at")?)?;
    let last_seen_at =
        decode_optional_timestamp(row.try_get::<Option<String>, _>("last_seen_at")?.as_deref())?;
    let revoked_at =
        decode_optional_timestamp(row.try_get::<Option<String>, _>("revoked_at")?.as_deref())?;
    if last_seen_at.is_some_and(|value| value < created_at)
        || revoked_at.is_some_and(|value| value < created_at)
    {
        return Err(inconsistent());
    }
    Ok(DeviceSummary {
        device_id: DeviceId::parse_canonical(&row.try_get::<String, _>("device_id")?)
            .map_err(|_| inconsistent())?,
        display_name: crate::domain::DeviceDisplayName::try_new(row.try_get("display_name")?)
            .map_err(|_| inconsistent())?,
        created_at,
        last_seen_at,
        revoked_at,
    })
}

impl SqliteStateStore {
    async fn provision_device_inner(
        &self,
        intent: ProvisionDeviceIntent,
    ) -> Result<DeviceSummary, SqliteAdapterError> {
        let mut transaction = WriteTransaction::begin(&self.runtime, "provision_device").await?;
        let result = sqlx::query(
            "INSERT INTO client_devices (device_id, display_name, token_hash, created_at, \
             last_seen_at, revoked_at) VALUES (?, ?, ?, ?, NULL, NULL)",
        )
        .bind(intent.device_id.to_string())
        .bind(intent.display_name.as_str())
        .bind(intent.token_hash.canonical_text())
        .bind(intent.created_at.to_string())
        .execute(transaction.connection())
        .await;
        if let Err(error) = result {
            let classified = SqliteAdapterError::from_sqlx(error);
            return Err(
                if classified
                    .sqlite_code()
                    .is_some_and(|code| code & 0xff == 19)
                {
                    SqliteAdapterError::new(SqliteFailureKind::StateConflict)
                } else {
                    classified
                },
            );
        }
        transaction.commit().await?;
        Ok(DeviceSummary {
            device_id: intent.device_id,
            display_name: intent.display_name,
            created_at: intent.created_at,
            last_seen_at: None,
            revoked_at: None,
        })
    }

    async fn lookup_device_inner(
        &self,
        token_hash: DeviceTokenHash,
    ) -> Result<Option<DeviceCredentialMatch>, SqliteAdapterError> {
        let mut connection = self.runtime.acquire().await?;
        let row = sqlx::query(
            "SELECT device_id, token_hash, revoked_at FROM client_devices WHERE token_hash = ?",
        )
        .bind(token_hash.canonical_text())
        .fetch_optional(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        row.map(|row| {
            Ok(DeviceCredentialMatch {
                device_id: DeviceId::parse_canonical(&row.try_get::<String, _>("device_id")?)
                    .map_err(|_| inconsistent())?,
                matched_hash: DeviceTokenHash::parse_canonical(
                    &row.try_get::<String, _>("token_hash")?,
                )
                .map_err(|_| inconsistent())?,
                revoked_at: decode_optional_timestamp(
                    row.try_get::<Option<String>, _>("revoked_at")?.as_deref(),
                )?,
            })
        })
        .transpose()
    }

    async fn list_devices_inner(&self) -> Result<Vec<DeviceSummary>, SqliteAdapterError> {
        let mut connection = self.runtime.acquire().await?;
        sqlx::query(
            "SELECT device_id, display_name, created_at, last_seen_at, revoked_at \
             FROM client_devices ORDER BY created_at ASC, device_id ASC",
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .iter()
        .map(decode_device_summary)
        .collect()
    }

    async fn revoke_device_inner(
        &self,
        device_id: DeviceId,
        revoked_at: UtcTimestamp,
    ) -> Result<RevokeDeviceOutcome, SqliteAdapterError> {
        let mut transaction = WriteTransaction::begin(&self.runtime, "revoke_device").await?;
        let row = sqlx::query(
            "SELECT device_id, display_name, created_at, last_seen_at, revoked_at \
             FROM client_devices WHERE device_id = ?",
        )
        .bind(device_id.to_string())
        .fetch_optional(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(RevokeDeviceOutcome::NotFound);
        };
        let mut summary = decode_device_summary(&row)?;
        if summary.revoked_at.is_some() {
            transaction.commit().await?;
            return Ok(RevokeDeviceOutcome::AlreadyRevoked(summary));
        }
        if revoked_at < summary.created_at {
            return Err(invalid());
        }
        let affected = sqlx::query(
            "UPDATE client_devices SET revoked_at = ? \
             WHERE device_id = ? AND revoked_at IS NULL",
        )
        .bind(revoked_at.to_string())
        .bind(device_id.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .rows_affected();
        if affected != 1 {
            return Err(inconsistent());
        }
        summary.revoked_at = Some(revoked_at);
        transaction.commit().await?;
        Ok(RevokeDeviceOutcome::Revoked(summary))
    }

    async fn touch_device_inner(
        &self,
        device_id: DeviceId,
        observed_at: UtcTimestamp,
    ) -> Result<(), SqliteAdapterError> {
        let mut transaction =
            WriteTransaction::begin(&self.runtime, "touch_device_last_seen").await?;
        sqlx::query(
            "UPDATE client_devices SET last_seen_at = ? \
             WHERE device_id = ? AND revoked_at IS NULL AND created_at <= ? \
               AND (last_seen_at IS NULL OR last_seen_at < ?)",
        )
        .bind(observed_at.to_string())
        .bind(device_id.to_string())
        .bind(observed_at.to_string())
        .bind(observed_at.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        transaction.commit().await
    }
}

impl DeviceCredentialStore for SqliteStateStore {
    fn provision_device(
        &self,
        intent: ProvisionDeviceIntent,
    ) -> DeviceCredentialFuture<'_, DeviceSummary> {
        Box::pin(async move {
            self.provision_device_inner(intent)
                .await
                .map_err(map_device_store_error)
        })
    }

    fn lookup_device_by_token_hash(
        &self,
        token_hash: DeviceTokenHash,
    ) -> DeviceCredentialFuture<'_, Option<DeviceCredentialMatch>> {
        Box::pin(async move {
            self.lookup_device_inner(token_hash)
                .await
                .map_err(map_device_store_error)
        })
    }

    fn list_devices(&self) -> DeviceCredentialFuture<'_, Vec<DeviceSummary>> {
        Box::pin(async move {
            self.list_devices_inner()
                .await
                .map_err(map_device_store_error)
        })
    }

    fn revoke_device(
        &self,
        device_id: DeviceId,
        revoked_at: UtcTimestamp,
    ) -> DeviceCredentialFuture<'_, RevokeDeviceOutcome> {
        Box::pin(async move {
            self.revoke_device_inner(device_id, revoked_at)
                .await
                .map_err(map_device_store_error)
        })
    }

    fn best_effort_touch_last_seen(
        &self,
        device_id: DeviceId,
        observed_at: UtcTimestamp,
    ) -> DeviceCredentialFuture<'_, ()> {
        Box::pin(async move {
            self.touch_device_inner(device_id, observed_at)
                .await
                .map_err(map_device_store_error)
        })
    }
}

struct MessageTopology {
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    workspace_id: WorkspaceId,
    next_ordinal: ConversationWorkOrdinal,
    conversation_version: ProjectionVersion,
    conversation_created_event_id: JournalEventId,
}

async fn load_message_topology(
    transaction: &mut WriteTransaction,
    requested_conversation_id: ConversationId,
) -> Result<MessageTopology, SqliteAdapterError> {
    let rows = sqlx::query(
        "SELECT p.craxii_id, p.primary_conversation_id, p.default_workspace_id, \
                c.craxii_id AS conversation_owner, c.kind, c.lifecycle_state, \
                c.next_work_ordinal, c.state_version, w.craxii_id AS workspace_owner \
         FROM craxii_principals p \
         JOIN conversations c ON c.conversation_id = p.primary_conversation_id \
         JOIN workspaces w ON w.workspace_id = p.default_workspace_id",
    )
    .fetch_all(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let [row] = rows.as_slice() else {
        return Err(inconsistent());
    };
    let craxii_id = CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
        .map_err(|_| inconsistent())?;
    let conversation_id =
        ConversationId::parse_canonical(&row.try_get::<String, _>("primary_conversation_id")?)
            .map_err(|_| inconsistent())?;
    let workspace_id =
        WorkspaceId::parse_canonical(&row.try_get::<String, _>("default_workspace_id")?)
            .map_err(|_| inconsistent())?;
    if conversation_id != requested_conversation_id
        || CraxiiId::parse_canonical(&row.try_get::<String, _>("conversation_owner")?)
            .map_err(|_| inconsistent())?
            != craxii_id
        || CraxiiId::parse_canonical(&row.try_get::<String, _>("workspace_owner")?)
            .map_err(|_| inconsistent())?
            != craxii_id
        || row.try_get::<String, _>("kind")? != "primary"
        || row.try_get::<String, _>("lifecycle_state")? != "active"
    {
        return Err(inconsistent());
    }
    let stream_id = JournalStreamId::Conversation(conversation_id).to_string();
    let created_rows = sqlx::query(
        "SELECT event_id FROM journal_events \
         WHERE stream_id = ? AND event_type = 'conversation.created'",
    )
    .bind(stream_id)
    .fetch_all(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let [created] = created_rows.as_slice() else {
        return Err(inconsistent());
    };
    Ok(MessageTopology {
        craxii_id,
        conversation_id,
        workspace_id,
        next_ordinal: ConversationWorkOrdinal::try_new(row.try_get("next_work_ordinal")?)
            .map_err(|_| inconsistent())?,
        conversation_version: ProjectionVersion::try_new(row.try_get("state_version")?)
            .map_err(|_| inconsistent())?,
        conversation_created_event_id: JournalEventId::parse_canonical(
            &created.try_get::<String, _>("event_id")?,
        )
        .map_err(|_| inconsistent())?,
    })
}

async fn load_command_row(
    transaction: &mut WriteTransaction,
    device_id: DeviceId,
    idempotency_key: &IdempotencyKey,
) -> Result<Option<super::stage9_codec::DecodedClientCommandRow>, SqliteAdapterError> {
    sqlx::query(
        "SELECT device_id, idempotency_key, command_type, request_hash, response_http_status, \
                response_json, committed_cursor, created_at \
         FROM client_commands WHERE device_id = ? AND idempotency_key = ?",
    )
    .bind(device_id.to_string())
    .bind(idempotency_key.as_str())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .as_ref()
    .map(decode_client_command_row)
    .transpose()
}

fn validate_command_match(
    row: &super::stage9_codec::DecodedClientCommandRow,
    device_id: DeviceId,
    idempotency_key: &IdempotencyKey,
    kind: CommandKind,
    request_hash: CommandRequestHash,
) -> Result<(), SqliteAdapterError> {
    if row.device_id != device_id || row.idempotency_key != *idempotency_key {
        return Err(inconsistent());
    }
    if row.command_kind != kind || row.request_hash != request_hash {
        return Err(idempotency_conflict());
    }
    Ok(())
}

struct NewClientCommandRow<'a> {
    device_id: DeviceId,
    idempotency_key: &'a IdempotencyKey,
    kind: CommandKind,
    request_hash: CommandRequestHash,
    status: u16,
    response_json: &'a str,
    cursor: JournalOffset,
    created_at: UtcTimestamp,
}

async fn insert_command_row(
    transaction: &mut WriteTransaction,
    row: NewClientCommandRow<'_>,
) -> Result<(), SqliteAdapterError> {
    let result = sqlx::query(
        "INSERT INTO client_commands (device_id, idempotency_key, command_type, request_hash, \
                response_http_status, response_json, committed_cursor, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(row.device_id.to_string())
    .bind(row.idempotency_key.as_str())
    .bind(row.kind.as_str())
    .bind(row.request_hash.canonical_text())
    .bind(i64::from(row.status))
    .bind(row.response_json)
    .bind(row.cursor.get())
    .bind(row.created_at.to_string())
    .execute(transaction.connection())
    .await;
    match result {
        Ok(result) if result.rows_affected() == 1 => Ok(()),
        Ok(_) => Err(inconsistent()),
        Err(error) => {
            let classified = SqliteAdapterError::from_sqlx(error);
            if classified
                .sqlite_code()
                .is_some_and(|code| code & 0xff == 19)
            {
                Err(idempotency_conflict())
            } else {
                Err(classified)
            }
        }
    }
}

async fn accept_message_inner(
    store: &SqliteStateStore,
    request: AcceptUserMessageRequest,
) -> Result<CommandOutcome<MessageCommandReceipt>, SqliteAdapterError> {
    request
        .idempotency_key
        .require_message_id(request.client_message_id)
        .map_err(|_| invalid())?;
    if request.hash_version != CommandHashEncodingVersion::V1
        || request.request_hash
            != CommandRequestHash::for_message(
                PROTOCOL_VERSION,
                request.conversation_id,
                request.client_message_id,
                &request.content,
            )
    {
        return Err(invalid());
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "accept_user_message").await?;
    if let Some(row) = load_command_row(
        &mut transaction,
        request.device_id,
        &request.idempotency_key,
    )
    .await?
    {
        validate_command_match(
            &row,
            request.device_id,
            &request.idempotency_key,
            CommandKind::Message,
            request.request_hash,
        )?;
        let receipt = decode_message_receipt(
            &row.response_json,
            row.response_http_status,
            row.committed_cursor,
        )?;
        transaction.commit().await?;
        return Ok(CommandOutcome::Replayed(receipt));
    }

    let topology = load_message_topology(&mut transaction, request.conversation_id).await?;
    let correlation_id = CorrelationId::for_work(request.candidates.work_id);
    let message = Message::try_new(MessageInput {
        message_id: request.candidates.message_id,
        craxii_id: topology.craxii_id,
        conversation_id: topology.conversation_id,
        role: MessageRole::User,
        content: request.content,
        produced_by_work_id: None,
        device_id: Some(request.device_id),
        client_message_id: Some(request.client_message_id),
        committed_at: request.accepted_at,
    })
    .map_err(|_| invalid())?;
    let work = WorkItem::new(WorkItemInputData {
        work_id: request.candidates.work_id,
        craxii_id: topology.craxii_id,
        conversation_id: topology.conversation_id,
        conversation_work_ordinal: topology.next_ordinal,
        workspace_id: topology.workspace_id,
        correlation_id,
        created_at: request.accepted_at,
        queued_at: request.accepted_at,
    });
    let input = WorkItemInput::new(
        work.work_id(),
        request.candidates.acceptance_event_id,
        WorkInputRelationship::Trigger,
        WorkInputOrdinal::try_new(1).map_err(|_| invalid())?,
        request.accepted_at,
        WorkInputActor::User,
    );

    let (content_json, content_sha256) = encode_message_content(message.content())?;
    sqlx::query(
        "INSERT INTO messages (message_id, craxii_id, conversation_id, role, content_json, \
                content_sha256, produced_by_work_id, client_device_id, client_message_id, \
                committed_at) VALUES (?, ?, ?, 'user', ?, ?, NULL, ?, ?, ?)",
    )
    .bind(message.message_id().to_string())
    .bind(message.craxii_id().to_string())
    .bind(message.conversation_id().to_string())
    .bind(content_json)
    .bind(content_sha256.to_string())
    .bind(request.device_id.to_string())
    .bind(request.client_message_id.to_string())
    .bind(request.accepted_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterMessageInsert)?;

    append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.candidates.acceptance_event_id,
            craxii_id: topology.craxii_id,
            stream_id: JournalStreamId::Conversation(topology.conversation_id),
            conversation_id: Some(topology.conversation_id),
            work_id: None,
            causation_event_id: Some(topology.conversation_created_event_id),
            correlation_id,
            actor: JournalActor::User(Some(request.device_id)),
            runtime_instance_id: None,
            payload: JournalEventPayload::MessageAccepted(MessageCommittedV1 {
                message_id: message.message_id(),
                craxii_id: message.craxii_id(),
                conversation_id: message.conversation_id(),
                role: message.role(),
                content: message.content().clone(),
                content_sha256: message.content_sha256(),
                produced_by_work_id: None,
                device_id: Some(request.device_id),
                client_message_id: Some(request.client_message_id),
                committed_at: request.accepted_at,
            }),
            recorded_at: request.accepted_at,
            occurred_at: None,
        })?,
    )
    .await?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterMessageAccepted)?;

    sqlx::query(
        "INSERT INTO work_items (work_id, craxii_id, conversation_id, \
                conversation_work_ordinal, kind, state, state_version, priority, workspace_id, \
                runtime_instance_id, current_model_invocation_id, current_tool_execution_id, \
                correlation_id, created_at, queued_at, started_at, cancel_requested_at, \
                cancellation_reason_code, terminal_at, terminal_reason_code, terminal_detail_json) \
         VALUES (?, ?, ?, ?, 'conversational', 'queued', 1, 0, ?, NULL, NULL, NULL, ?, ?, ?, \
                 NULL, NULL, NULL, NULL, NULL, NULL)",
    )
    .bind(work.work_id().to_string())
    .bind(work.craxii_id().to_string())
    .bind(work.conversation_id().to_string())
    .bind(work.conversation_work_ordinal().get())
    .bind(work.workspace_id().to_string())
    .bind(correlation_id.to_string())
    .bind(request.accepted_at.to_string())
    .bind(request.accepted_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterWorkInsert)?;

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
    store.fire_stage9_test_hook(Stage9TestHook::AfterWorkInput)?;

    let queued_position = append_event(
        &mut transaction,
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
            payload: JournalEventPayload::WorkQueued(WorkQueuedV1 {
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
            }),
            recorded_at: request.accepted_at,
            occurred_at: None,
        })?,
    )
    .await?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterWorkQueued)?;

    advance_conversation_ordinal(
        &mut transaction,
        topology.conversation_id,
        topology.conversation_version,
        topology.next_ordinal,
    )
    .await
    .map_err(map_projection_error)?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterConversationAdvance)?;

    let receipt = MessageCommandReceipt {
        conversation_id: topology.conversation_id,
        message_id: message.message_id(),
        work_id: work.work_id(),
        work_ordinal: work.conversation_work_ordinal(),
        committed_cursor: queued_position.offset,
    };
    let response_json = encode_message_receipt(&receipt)?;
    store.fire_stage9_test_hook(Stage9TestHook::BeforeClientCommandInsert)?;
    insert_command_row(
        &mut transaction,
        NewClientCommandRow {
            device_id: request.device_id,
            idempotency_key: &request.idempotency_key,
            kind: CommandKind::Message,
            request_hash: request.request_hash,
            status: MessageCommandReceipt::HTTP_STATUS,
            response_json: &response_json,
            cursor: receipt.committed_cursor,
            created_at: request.accepted_at,
        },
    )
    .await?;
    store.fire_stage9_test_hook(Stage9TestHook::AfterClientCommandInsert)?;
    transaction.commit().await?;
    Ok(CommandOutcome::Committed(receipt))
}

struct CancellationWork {
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    correlation_id: CorrelationId,
    snapshot: WorkLifecycleSnapshot,
    started_at: Option<UtcTimestamp>,
}

async fn load_cancellation_work(
    transaction: &mut WriteTransaction,
    work_id: WorkId,
) -> Result<CancellationWork, SqliteAdapterError> {
    let row = sqlx::query(
        "SELECT w.*, p.primary_conversation_id, p.default_workspace_id \
         FROM work_items w \
         JOIN craxii_principals p ON p.craxii_id = w.craxii_id \
         WHERE w.work_id = ?",
    )
    .bind(work_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(target_not_found)?;
    let conversation_id =
        ConversationId::parse_canonical(&row.try_get::<String, _>("conversation_id")?)
            .map_err(|_| inconsistent())?;
    let workspace_id = WorkspaceId::parse_canonical(&row.try_get::<String, _>("workspace_id")?)
        .map_err(|_| inconsistent())?;
    if ConversationId::parse_canonical(&row.try_get::<String, _>("primary_conversation_id")?)
        .map_err(|_| inconsistent())?
        != conversation_id
        || WorkspaceId::parse_canonical(&row.try_get::<String, _>("default_workspace_id")?)
            .map_err(|_| inconsistent())?
            != workspace_id
    {
        return Err(target_not_found());
    }
    let state = decode_work_state(&row.try_get::<String, _>("state")?)?;
    let owner: Option<RuntimeInstanceId> = decode_optional_id(
        row.try_get::<Option<String>, _>("runtime_instance_id")?
            .as_deref(),
    )?;
    let model: Option<ModelInvocationId> = decode_optional_id(
        row.try_get::<Option<String>, _>("current_model_invocation_id")?
            .as_deref(),
    )?;
    let tool: Option<ToolExecutionId> = decode_optional_id(
        row.try_get::<Option<String>, _>("current_tool_execution_id")?
            .as_deref(),
    )?;
    let current_attempt = match (model, tool) {
        (None, None) => CurrentWorkAttempt::None,
        (Some(value), None) => CurrentWorkAttempt::Model(value),
        (None, Some(value)) => CurrentWorkAttempt::Tool(value),
        (Some(_), Some(_)) => return Err(inconsistent()),
    };
    let cancellation_reason = row
        .try_get::<Option<String>, _>("cancellation_reason_code")?
        .map(|value| decode_cancellation_reason(&value))
        .transpose()?;
    let terminal_reason = representative_terminal_reason(
        state,
        row.try_get::<Option<String>, _>("terminal_reason_code")?
            .as_deref(),
    )?;
    let snapshot = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id,
        state,
        projection_version: ProjectionVersion::try_new(row.try_get("state_version")?)
            .map_err(|_| inconsistent())?,
        runtime_owner: owner,
        current_attempt,
        cancellation_reason,
        terminal_reason,
    })
    .map_err(|_| inconsistent())?;
    Ok(CancellationWork {
        craxii_id: CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        conversation_id,
        correlation_id: CorrelationId::parse_canonical(
            &row.try_get::<String, _>("correlation_id")?,
        )
        .map_err(|_| inconsistent())?,
        snapshot,
        started_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("started_at")?.as_deref(),
        )?,
    })
}

fn representative_terminal_reason(
    state: WorkState,
    code: Option<&str>,
) -> Result<Option<WorkTerminalReason>, SqliteAdapterError> {
    match state {
        WorkState::Completed => match code {
            Some("answered") => Ok(Some(WorkTerminalReason::Completion(
                WorkCompletionReason::Answered,
            ))),
            Some("refused") => Ok(Some(WorkTerminalReason::Completion(
                WorkCompletionReason::Refused,
            ))),
            _ => Err(inconsistent()),
        },
        WorkState::Failed => Ok(Some(WorkTerminalReason::Failure(
            WorkFailureReason::ProviderExhausted,
        ))),
        WorkState::Cancelled => match code {
            Some("user_request") => Ok(Some(WorkTerminalReason::Cancellation(
                WorkCancellationReason::UserRequest,
            ))),
            Some("graceful_shutdown") => Ok(Some(WorkTerminalReason::Cancellation(
                WorkCancellationReason::GracefulShutdown,
            ))),
            _ => Err(inconsistent()),
        },
        WorkState::Interrupted => Ok(Some(WorkTerminalReason::Interruption(
            WorkInterruptionReason::RuntimeOwnershipLost,
        ))),
        _ if code.is_none() => Ok(None),
        _ => Err(inconsistent()),
    }
}

async fn latest_work_event(
    transaction: &mut WriteTransaction,
    work_id: WorkId,
) -> Result<JournalEventId, SqliteAdapterError> {
    let row = sqlx::query(
        "SELECT event_id FROM journal_events WHERE stream_id = ? \
         ORDER BY stream_seq DESC LIMIT 1",
    )
    .bind(JournalStreamId::Work(work_id).to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(inconsistent)?;
    JournalEventId::parse_canonical(&row.try_get::<String, _>("event_id")?)
        .map_err(|_| inconsistent())
}

async fn journal_head_in_write(
    transaction: &mut WriteTransaction,
) -> Result<JournalOffset, SqliteAdapterError> {
    let value =
        sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(journal_offset) FROM journal_events")
            .fetch_one(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?
            .ok_or_else(inconsistent)?;
    JournalOffset::try_new(value).map_err(|_| inconsistent())
}

fn journal_attempt(value: CurrentWorkAttempt) -> JournalCurrentAttempt {
    match value {
        CurrentWorkAttempt::None => JournalCurrentAttempt::None,
        CurrentWorkAttempt::Model(id) => JournalCurrentAttempt::Model(id),
        CurrentWorkAttempt::Tool(id) => JournalCurrentAttempt::Tool(id),
    }
}

fn cancellation_payload(
    current: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    requested_at: UtcTimestamp,
) -> Result<JournalEventPayload, SqliteAdapterError> {
    let terminal_reason = match next.terminal_reason() {
        Some(WorkTerminalReason::Cancellation(WorkCancellationReason::UserRequest)) => {
            Some(JournalWorkTerminalReason::UserRequest)
        }
        Some(WorkTerminalReason::Cancellation(WorkCancellationReason::GracefulShutdown)) => {
            Some(JournalWorkTerminalReason::GracefulShutdown)
        }
        None => None,
        _ => return Err(invalid()),
    };
    let transition = WorkTransitionV1 {
        work_id: current.work_id(),
        from_state: current.state(),
        to_state: next.state(),
        expected_state_version: current.projection_version(),
        expected_runtime_owner: current.runtime_owner(),
        expected_current_attempt: journal_attempt(current.current_attempt()),
        expected_cancellation_reason: current.cancellation_reason(),
        state_version: next.projection_version(),
        runtime_owner: next.runtime_owner(),
        current_attempt: journal_attempt(next.current_attempt()),
        cancellation_reason: next.cancellation_reason(),
        terminal_reason,
        transitioned_at: requested_at,
    };
    match next.state() {
        WorkState::CancelRequested => Ok(JournalEventPayload::WorkCancelRequested(transition)),
        WorkState::Cancelled => Ok(JournalEventPayload::WorkCancelled(transition)),
        _ => Err(invalid()),
    }
}

async fn cancellation_inner(
    store: &SqliteStateStore,
    request: RequestCancellationRequest,
) -> Result<CommandOutcome<CancellationCommandReceipt>, SqliteAdapterError> {
    request
        .idempotency_key
        .require_command_id(request.client_command_id)
        .map_err(|_| invalid())?;
    if request.hash_version != CommandHashEncodingVersion::V1
        || request.request_hash
            != CommandRequestHash::for_cancellation(
                PROTOCOL_VERSION,
                request.client_command_id,
                request.work_id,
            )
    {
        return Err(invalid());
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "request_cancellation").await?;
    if let Some(row) = load_command_row(
        &mut transaction,
        request.device_id,
        &request.idempotency_key,
    )
    .await?
    {
        validate_command_match(
            &row,
            request.device_id,
            &request.idempotency_key,
            CommandKind::Cancel,
            request.request_hash,
        )?;
        let receipt = decode_cancellation_receipt(
            &row.response_json,
            row.response_http_status,
            row.committed_cursor,
        )?;
        transaction.commit().await?;
        return Ok(CommandOutcome::Replayed(receipt));
    }

    let work = load_cancellation_work(&mut transaction, request.work_id).await?;
    let decision = decide_cancellation(
        &work.snapshot,
        CancellationCheckpoint::BeforeNextIteration,
        WorkCancellationReason::UserRequest,
    )
    .map_err(|_| inconsistent())?;
    let (resulting_state, cleanup, cursor) = match &decision {
        CancellationDecision::DirectCancelled { transition, .. }
        | CancellationDecision::CancellationRequested { transition, .. } => {
            let next = transition.next();
            guarded_work_update(
                &mut transaction,
                &work.snapshot,
                next,
                WorkProjectionTimes {
                    started_at: work.started_at,
                    cancel_requested_at: if next.state() == WorkState::CancelRequested {
                        Some(request.requested_at)
                    } else {
                        None
                    },
                    terminal_at: if next.state() == WorkState::Cancelled {
                        Some(request.requested_at)
                    } else {
                        None
                    },
                },
            )
            .await
            .map_err(map_projection_error)?;
            let causation = latest_work_event(&mut transaction, request.work_id).await?;
            let position = append_event(
                &mut transaction,
                prepare_event(JournalAppendIntent {
                    event_id: request.event_id,
                    craxii_id: work.craxii_id,
                    stream_id: JournalStreamId::Work(request.work_id),
                    conversation_id: Some(work.conversation_id),
                    work_id: Some(request.work_id),
                    causation_event_id: Some(causation),
                    correlation_id: work.correlation_id,
                    actor: JournalActor::User(Some(request.device_id)),
                    runtime_instance_id: next.runtime_owner(),
                    payload: cancellation_payload(&work.snapshot, next, request.requested_at)?,
                    recorded_at: request.requested_at,
                    occurred_at: None,
                })?,
            )
            .await?;
            (
                next.state(),
                if next.state() == WorkState::CancelRequested {
                    CancellationCleanupDisposition::Pending
                } else {
                    CancellationCleanupDisposition::NotPending
                },
                position.offset,
            )
        }
        CancellationDecision::AlreadyRequestedNoOp { .. } => (
            WorkState::CancelRequested,
            CancellationCleanupDisposition::Pending,
            journal_head_in_write(&mut transaction).await?,
        ),
        CancellationDecision::AlreadyTerminalNoOp { state, .. } => (
            *state,
            CancellationCleanupDisposition::NotPending,
            journal_head_in_write(&mut transaction).await?,
        ),
    };
    let receipt =
        CancellationCommandReceipt::try_new(request.work_id, resulting_state, cleanup, cursor)
            .map_err(|_| invalid())?;
    let response_json = encode_cancellation_receipt(&receipt)?;
    insert_command_row(
        &mut transaction,
        NewClientCommandRow {
            device_id: request.device_id,
            idempotency_key: &request.idempotency_key,
            kind: CommandKind::Cancel,
            request_hash: request.request_hash,
            status: receipt.http_status(),
            response_json: &response_json,
            cursor,
            created_at: request.requested_at,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(CommandOutcome::Committed(receipt))
}

impl CommandStateStore for SqliteStateStore {
    fn accept_user_message_and_create_work(
        &self,
        request: AcceptUserMessageRequest,
    ) -> StateStoreFuture<'_, CommandOutcome<MessageCommandReceipt>> {
        Box::pin(async move {
            accept_message_inner(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn request_cancellation(
        &self,
        request: RequestCancellationRequest,
    ) -> StateStoreFuture<'_, CommandOutcome<CancellationCommandReceipt>> {
        Box::pin(async move {
            cancellation_inner(self, request)
                .await
                .map_err(map_port_error)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage9TestHook {
    AfterMessageInsert,
    AfterMessageAccepted,
    AfterWorkInsert,
    AfterWorkInput,
    AfterWorkQueued,
    AfterConversationAdvance,
    BeforeClientCommandInsert,
    AfterClientCommandInsert,
}

impl SqliteStateStore {
    #[cfg(test)]
    pub(super) fn set_stage9_test_hook(&self, hook: Option<Stage9TestHook>) {
        *self.stage9_hook.lock().unwrap() = hook;
    }

    #[cfg(test)]
    fn fire_stage9_test_hook(&self, hook: Stage9TestHook) -> Result<(), SqliteAdapterError> {
        if self.stage9_hook.lock().unwrap().as_ref() == Some(&hook) {
            Err(invalid())
        } else {
            Ok(())
        }
    }

    #[cfg(not(test))]
    fn fire_stage9_test_hook(&self, _hook: Stage9TestHook) -> Result<(), SqliteAdapterError> {
        Ok(())
    }
}

pub(super) async fn verify_stage9_consistency(
    connection: &mut sqlx::SqliteConnection,
    events: &[JournalEvent],
) -> Result<u64, SqliteAdapterError> {
    verify_devices(connection).await?;
    verify_client_commands(connection, events).await?;
    Ok(12)
}

async fn verify_devices(connection: &mut sqlx::SqliteConnection) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query("SELECT * FROM client_devices")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    for row in &rows {
        let summary = decode_device_summary(row)?;
        DeviceTokenHash::parse_canonical(&row.try_get::<String, _>("token_hash")?)
            .map_err(|_| inconsistent())?;
        if summary
            .last_seen_at
            .is_some_and(|seen| seen < summary.created_at)
            || summary
                .revoked_at
                .is_some_and(|revoked| revoked < summary.created_at)
        {
            return Err(inconsistent());
        }
    }
    Ok(())
}

async fn verify_client_commands(
    connection: &mut sqlx::SqliteConnection,
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query(
        "SELECT device_id, idempotency_key, command_type, request_hash, response_http_status, \
                response_json, committed_cursor, created_at FROM client_commands",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let events_by_offset = events
        .iter()
        .map(|event| (event.journal_offset, event))
        .collect::<HashMap<_, _>>();
    let events_by_id = events
        .iter()
        .map(|event| (event.event_id, event))
        .collect::<HashMap<_, _>>();
    let commands = rows
        .iter()
        .map(decode_client_command_row)
        .collect::<Result<Vec<_>, _>>()?;
    for command in &commands {
        let device_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM client_devices WHERE device_id = ?")
                .bind(command.device_id.to_string())
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
        if device_exists != 1 || !events_by_offset.contains_key(&command.committed_cursor) {
            return Err(inconsistent());
        }
        match command.command_kind {
            CommandKind::Message => {
                verify_message_command(connection, command, &events_by_id).await?
            }
            CommandKind::Cancel => {
                verify_cancellation_command(connection, command, &events_by_offset, events).await?
            }
        }
    }
    verify_stage9_cancellation_events(connection, &commands, events).await?;
    Ok(())
}

async fn verify_message_command(
    connection: &mut sqlx::SqliteConnection,
    command: &super::stage9_codec::DecodedClientCommandRow,
    events_by_id: &HashMap<JournalEventId, &JournalEvent>,
) -> Result<(), SqliteAdapterError> {
    let client_message_id = ClientMessageId::parse_canonical(command.idempotency_key.as_str())
        .map_err(|_| inconsistent())?;
    let message_rows =
        sqlx::query("SELECT * FROM messages WHERE client_device_id = ? AND client_message_id = ?")
            .bind(command.device_id.to_string())
            .bind(client_message_id.to_string())
            .fetch_all(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    let [message_row] = message_rows.as_slice() else {
        return Err(inconsistent());
    };
    let message = decode_message_row(message_row)?;
    let receipt = decode_message_receipt(
        &command.response_json,
        command.response_http_status,
        command.committed_cursor,
    )?;
    if message.role() != MessageRole::User
        || message.device_id() != Some(command.device_id)
        || message.client_message_id() != Some(client_message_id)
        || message.message_id() != receipt.message_id
        || message.conversation_id() != receipt.conversation_id
        || message.committed_at() != command.created_at
        || command.request_hash
            != CommandRequestHash::for_message(
                PROTOCOL_VERSION,
                receipt.conversation_id,
                client_message_id,
                message.content(),
            )
    {
        return Err(inconsistent());
    }

    let work_row = sqlx::query(
        "SELECT craxii_id, conversation_id, conversation_work_ordinal, correlation_id \
         FROM work_items WHERE work_id = ?",
    )
    .bind(receipt.work_id.to_string())
    .fetch_optional(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(inconsistent)?;
    let work_craxii = CraxiiId::parse_canonical(&work_row.try_get::<String, _>("craxii_id")?)
        .map_err(|_| inconsistent())?;
    let work_conversation =
        ConversationId::parse_canonical(&work_row.try_get::<String, _>("conversation_id")?)
            .map_err(|_| inconsistent())?;
    let work_ordinal =
        ConversationWorkOrdinal::try_new(work_row.try_get("conversation_work_ordinal")?)
            .map_err(|_| inconsistent())?;
    let work_correlation =
        CorrelationId::parse_canonical(&work_row.try_get::<String, _>("correlation_id")?)
            .map_err(|_| inconsistent())?;
    if work_conversation != receipt.conversation_id
        || work_ordinal != receipt.work_ordinal
        || work_correlation != CorrelationId::for_work(receipt.work_id)
    {
        return Err(inconsistent());
    }

    let input_rows = sqlx::query(
        "SELECT input_event_id, relationship, ordinal_within_work, attached_at, \
                attached_by_actor FROM work_item_inputs WHERE work_id = ?",
    )
    .bind(receipt.work_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let [input] = input_rows.as_slice() else {
        return Err(inconsistent());
    };
    let accepted_event_id =
        JournalEventId::parse_canonical(&input.try_get::<String, _>("input_event_id")?)
            .map_err(|_| inconsistent())?;
    let accepted_event = events_by_id
        .get(&accepted_event_id)
        .copied()
        .ok_or_else(inconsistent)?;
    let JournalEventPayload::MessageAccepted(accepted) = &accepted_event.payload else {
        return Err(inconsistent());
    };
    let caused_by = accepted_event
        .causation_event_id
        .and_then(|id| events_by_id.get(&id).copied())
        .ok_or_else(inconsistent)?;
    if input.try_get::<String, _>("relationship")? != "trigger"
        || input.try_get::<i64, _>("ordinal_within_work")? != 1
        || input.try_get::<String, _>("attached_by_actor")? != "user"
        || decode_timestamp(&input.try_get::<String, _>("attached_at")?)? != message.committed_at()
        || accepted.message_id != message.message_id()
        || accepted.content != *message.content()
        || accepted.device_id != Some(command.device_id)
        || accepted.client_message_id != Some(client_message_id)
        || accepted_event.stream_id != JournalStreamId::Conversation(receipt.conversation_id)
        || accepted_event.conversation_id != Some(receipt.conversation_id)
        || accepted_event.work_id.is_some()
        || accepted_event.correlation_id != work_correlation
        || accepted_event.actor != JournalActor::User(Some(command.device_id))
        || !matches!(
            caused_by.payload,
            JournalEventPayload::ConversationCreated(_)
        )
        || caused_by.stream_id != JournalStreamId::Conversation(receipt.conversation_id)
    {
        return Err(inconsistent());
    }

    let queued_event = events_by_id
        .values()
        .copied()
        .find(|event| {
            event.journal_offset == receipt.committed_cursor
                && event.work_id == Some(receipt.work_id)
        })
        .ok_or_else(inconsistent)?;
    let JournalEventPayload::WorkQueued(queued) = &queued_event.payload else {
        return Err(inconsistent());
    };
    if queued.work_id != receipt.work_id
        || queued.craxii_id != work_craxii
        || queued.conversation_id != receipt.conversation_id
        || queued.conversation_work_ordinal != receipt.work_ordinal
        || queued.correlation_id != work_correlation
        || queued.trigger.input_event_id != accepted_event_id
        || queued_event.stream_id != JournalStreamId::Work(receipt.work_id)
        || queued_event.causation_event_id != Some(accepted_event_id)
        || queued_event.correlation_id != work_correlation
        || queued_event.actor != JournalActor::Craxii(work_craxii)
    {
        return Err(inconsistent());
    }
    Ok(())
}

async fn verify_cancellation_command(
    connection: &mut sqlx::SqliteConnection,
    command: &super::stage9_codec::DecodedClientCommandRow,
    events_by_offset: &HashMap<JournalOffset, &JournalEvent>,
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    let client_command_id = ClientCommandId::parse_canonical(command.idempotency_key.as_str())
        .map_err(|_| inconsistent())?;
    let receipt = decode_cancellation_receipt(
        &command.response_json,
        command.response_http_status,
        command.committed_cursor,
    )?;
    if command.request_hash
        != CommandRequestHash::for_cancellation(
            PROTOCOL_VERSION,
            client_command_id,
            receipt.work_id,
        )
    {
        return Err(inconsistent());
    }
    let work_row = sqlx::query("SELECT state, correlation_id FROM work_items WHERE work_id = ?")
        .bind(receipt.work_id.to_string())
        .fetch_optional(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .ok_or_else(inconsistent)?;
    let current_state = decode_work_state(&work_row.try_get::<String, _>("state")?)?;
    let correlation_id =
        CorrelationId::parse_canonical(&work_row.try_get::<String, _>("correlation_id")?)
            .map_err(|_| inconsistent())?;
    let cursor_event = events_by_offset
        .get(&receipt.committed_cursor)
        .copied()
        .ok_or_else(inconsistent)?;
    if cancellation_event_matches_command(cursor_event, command, &receipt, correlation_id) {
        return Ok(());
    }

    if events.iter().any(|event| {
        cancellation_event_matches_command_material(event, command, &receipt, correlation_id)
    }) {
        return Err(inconsistent());
    }

    let compatible_no_op = match receipt.resulting_work_state {
        WorkState::CancelRequested => {
            (current_state == WorkState::CancelRequested || current_state.is_terminal())
                && events.iter().any(|event| {
                    event.journal_offset <= receipt.committed_cursor
                        && event.work_id == Some(receipt.work_id)
                        && matches!(
                            &event.payload,
                            JournalEventPayload::WorkCancelRequested(transition)
                                if transition.work_id == receipt.work_id
                                    && transition.to_state == WorkState::CancelRequested
                        )
                })
        }
        state if state.is_terminal() => {
            current_state == state
                && events.iter().any(|event| {
                    if event.journal_offset > receipt.committed_cursor
                        || event.work_id != Some(receipt.work_id)
                    {
                        return false;
                    }
                    match &event.payload {
                        JournalEventPayload::WorkCompleted(value)
                        | JournalEventPayload::WorkFailed(value)
                        | JournalEventPayload::WorkCancelled(value)
                        | JournalEventPayload::WorkInterrupted(value) => value.to_state == state,
                        _ => false,
                    }
                })
        }
        _ => false,
    };
    if compatible_no_op {
        Ok(())
    } else {
        Err(inconsistent())
    }
}

fn cancellation_transition(event: &JournalEvent) -> Option<&WorkTransitionV1> {
    match &event.payload {
        JournalEventPayload::WorkCancelRequested(transition)
            if matches!(
                transition.from_state,
                WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool
            ) && transition.to_state == WorkState::CancelRequested
                && transition.cancellation_reason == Some(WorkCancellationReason::UserRequest) =>
        {
            Some(transition)
        }
        JournalEventPayload::WorkCancelled(transition)
            if transition.from_state == WorkState::Queued
                && transition.to_state == WorkState::Cancelled =>
        {
            Some(transition)
        }
        _ => None,
    }
}

fn cancellation_event_matches_command(
    event: &JournalEvent,
    command: &super::stage9_codec::DecodedClientCommandRow,
    receipt: &CancellationCommandReceipt,
    correlation_id: CorrelationId,
) -> bool {
    event.journal_offset == receipt.committed_cursor
        && cancellation_event_matches_command_material(event, command, receipt, correlation_id)
}

fn cancellation_event_matches_command_material(
    event: &JournalEvent,
    command: &super::stage9_codec::DecodedClientCommandRow,
    receipt: &CancellationCommandReceipt,
    correlation_id: CorrelationId,
) -> bool {
    cancellation_transition(event).is_some_and(|transition| {
        transition.work_id == receipt.work_id
            && transition.to_state == receipt.resulting_work_state
            && transition.transitioned_at == command.created_at
            && event.stream_id == JournalStreamId::Work(receipt.work_id)
            && event.work_id == Some(receipt.work_id)
            && event.correlation_id == correlation_id
            && event.actor == JournalActor::User(Some(command.device_id))
    })
}

async fn verify_stage9_cancellation_events(
    connection: &mut sqlx::SqliteConnection,
    commands: &[super::stage9_codec::DecodedClientCommandRow],
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    for event in events {
        let Some(transition) = cancellation_transition(event) else {
            continue;
        };
        let JournalActor::User(Some(actor_id)) = event.actor else {
            return Err(inconsistent());
        };
        let actor_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM client_devices WHERE device_id = ?")
                .bind(actor_id.to_string())
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
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
            .ok_or_else(inconsistent)?;
        if actor_exists != 1
            || event.stream_id != JournalStreamId::Work(transition.work_id)
            || event.work_id != Some(transition.work_id)
            || event.correlation_id != CorrelationId::for_work(transition.work_id)
            || event.causation_event_id != Some(previous.event_id)
            || event.recorded_at != transition.transitioned_at
            || event.occurred_at.is_some()
            || !exact_stage9_cancellation_shape(transition)
        {
            return Err(inconsistent());
        }

        let mut matching_commands = 0_u8;
        for command in commands
            .iter()
            .filter(|command| command.command_kind == CommandKind::Cancel)
        {
            let receipt = decode_cancellation_receipt(
                &command.response_json,
                command.response_http_status,
                command.committed_cursor,
            )?;
            if cancellation_event_matches_command(event, command, &receipt, event.correlation_id) {
                matching_commands = matching_commands.saturating_add(1);
            }
        }
        if matching_commands == 0
            || events.iter().any(|candidate| {
                candidate.journal_offset > event.journal_offset
                    && candidate.work_id == event.work_id
                    && candidate.actor == event.actor
                    && cancellation_transition(candidate)
                        .is_some_and(|later| later.transitioned_at == transition.transitioned_at)
            })
        {
            return Err(inconsistent());
        }
    }
    Ok(())
}

fn exact_stage9_cancellation_shape(transition: &WorkTransitionV1) -> bool {
    let common = transition.expected_cancellation_reason.is_none()
        && transition.runtime_owner == transition.expected_runtime_owner
        && transition.current_attempt == transition.expected_current_attempt;
    if !common {
        return false;
    }
    match (transition.from_state, transition.to_state) {
        (WorkState::Queued, WorkState::Cancelled) => {
            transition.expected_runtime_owner.is_none()
                && transition.expected_current_attempt == JournalCurrentAttempt::None
                && transition.cancellation_reason.is_none()
                && transition.terminal_reason == Some(JournalWorkTerminalReason::UserRequest)
        }
        (
            WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool,
            WorkState::CancelRequested,
        ) => {
            transition.expected_runtime_owner.is_some()
                && transition.cancellation_reason == Some(WorkCancellationReason::UserRequest)
                && transition.terminal_reason.is_none()
        }
        _ => false,
    }
}
