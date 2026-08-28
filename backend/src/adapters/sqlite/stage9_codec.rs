use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};

use crate::domain::{
    CancellationCleanupDisposition, CancellationCommandReceipt, CommandKind, CommandRequestHash,
    ConversationId, ConversationWorkOrdinal, DeviceId, IdempotencyKey, JournalOffset,
    MessageCommandReceipt, MessageId, UtcTimestamp, WorkId, WorkState,
};

use super::error::{SqliteAdapterError, SqliteFailureKind};

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessageReceiptV1 {
    version: u8,
    conversation_id: ConversationId,
    message_id: MessageId,
    work_id: WorkId,
    work_ordinal: ConversationWorkOrdinal,
    work_state: WorkState,
    committed_cursor: JournalOffset,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCancellationReceiptV1 {
    version: u8,
    work_id: WorkId,
    resulting_work_state: WorkState,
    cleanup_pending: bool,
    committed_cursor: JournalOffset,
}

pub(super) fn encode_message_receipt(
    receipt: &MessageCommandReceipt,
) -> Result<String, SqliteAdapterError> {
    serde_json::to_string(&StoredMessageReceiptV1 {
        version: MessageCommandReceipt::VERSION,
        conversation_id: receipt.conversation_id,
        message_id: receipt.message_id,
        work_id: receipt.work_id,
        work_ordinal: receipt.work_ordinal,
        work_state: receipt.work_state(),
        committed_cursor: receipt.committed_cursor,
    })
    .map_err(|_| inconsistent())
}

pub(super) fn decode_message_receipt(
    json: &str,
    status: u16,
    row_cursor: JournalOffset,
) -> Result<MessageCommandReceipt, SqliteAdapterError> {
    let stored: StoredMessageReceiptV1 = serde_json::from_str(json).map_err(|_| inconsistent())?;
    if stored.version != MessageCommandReceipt::VERSION
        || status != MessageCommandReceipt::HTTP_STATUS
        || stored.work_state != WorkState::Queued
        || stored.committed_cursor != row_cursor
    {
        return Err(inconsistent());
    }
    Ok(MessageCommandReceipt {
        conversation_id: stored.conversation_id,
        message_id: stored.message_id,
        work_id: stored.work_id,
        work_ordinal: stored.work_ordinal,
        committed_cursor: stored.committed_cursor,
    })
}

pub(super) fn encode_cancellation_receipt(
    receipt: &CancellationCommandReceipt,
) -> Result<String, SqliteAdapterError> {
    serde_json::to_string(&StoredCancellationReceiptV1 {
        version: CancellationCommandReceipt::VERSION,
        work_id: receipt.work_id,
        resulting_work_state: receipt.resulting_work_state,
        cleanup_pending: receipt.cleanup.is_pending(),
        committed_cursor: receipt.committed_cursor,
    })
    .map_err(|_| inconsistent())
}

pub(super) fn decode_cancellation_receipt(
    json: &str,
    status: u16,
    row_cursor: JournalOffset,
) -> Result<CancellationCommandReceipt, SqliteAdapterError> {
    let stored: StoredCancellationReceiptV1 =
        serde_json::from_str(json).map_err(|_| inconsistent())?;
    if stored.version != CancellationCommandReceipt::VERSION
        || stored.committed_cursor != row_cursor
    {
        return Err(inconsistent());
    }
    let cleanup = if stored.cleanup_pending {
        CancellationCleanupDisposition::Pending
    } else {
        CancellationCleanupDisposition::NotPending
    };
    let receipt = CancellationCommandReceipt::try_new(
        stored.work_id,
        stored.resulting_work_state,
        cleanup,
        stored.committed_cursor,
    )
    .map_err(|_| inconsistent())?;
    if receipt.http_status() != status {
        return Err(inconsistent());
    }
    Ok(receipt)
}

pub(super) struct DecodedClientCommandRow {
    pub device_id: DeviceId,
    pub idempotency_key: IdempotencyKey,
    pub command_kind: CommandKind,
    pub request_hash: CommandRequestHash,
    pub response_http_status: u16,
    pub response_json: String,
    pub committed_cursor: JournalOffset,
    pub created_at: UtcTimestamp,
}

pub(super) fn decode_client_command_row(
    row: &SqliteRow,
) -> Result<DecodedClientCommandRow, SqliteAdapterError> {
    let status = u16::try_from(row.try_get::<i64, _>("response_http_status")?)
        .map_err(|_| inconsistent())?;
    Ok(DecodedClientCommandRow {
        device_id: DeviceId::parse_canonical(&row.try_get::<String, _>("device_id")?)
            .map_err(|_| inconsistent())?,
        idempotency_key: IdempotencyKey::parse_canonical(
            &row.try_get::<String, _>("idempotency_key")?,
        )
        .map_err(|_| inconsistent())?,
        command_kind: CommandKind::parse(&row.try_get::<String, _>("command_type")?)
            .map_err(|_| inconsistent())?,
        request_hash: CommandRequestHash::parse_canonical(
            &row.try_get::<String, _>("request_hash")?,
        )
        .map_err(|_| inconsistent())?,
        response_http_status: status,
        response_json: row.try_get("response_json")?,
        committed_cursor: JournalOffset::try_new(row.try_get("committed_cursor")?)
            .map_err(|_| inconsistent())?,
        created_at: UtcTimestamp::parse_canonical(&row.try_get::<String, _>("created_at")?)
            .map_err(|_| inconsistent())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    #[test]
    fn message_codec_is_exact_versioned_and_rejects_extra_or_contradictory_data() {
        let receipt = MessageCommandReceipt {
            conversation_id: V7.parse().unwrap(),
            message_id: V7.parse().unwrap(),
            work_id: V7.parse().unwrap(),
            work_ordinal: ConversationWorkOrdinal::try_new(7).unwrap(),
            committed_cursor: JournalOffset::try_new(9).unwrap(),
        };
        let json = encode_message_receipt(&receipt).unwrap();
        assert_eq!(
            decode_message_receipt(&json, 202, receipt.committed_cursor).unwrap(),
            receipt
        );
        assert!(decode_message_receipt(&json, 200, receipt.committed_cursor).is_err());
        let extra = json.replacen('{', "{\"extra\":true,", 1);
        assert!(decode_message_receipt(&extra, 202, receipt.committed_cursor).is_err());
        assert!(
            decode_message_receipt(
                &json.replace("\"version\":1", "\"version\":2"),
                202,
                receipt.committed_cursor
            )
            .is_err()
        );
    }

    #[test]
    fn cancellation_codec_rejects_every_status_state_cleanup_contradiction() {
        let cursor = JournalOffset::try_new(9).unwrap();
        let receipt = CancellationCommandReceipt::try_new(
            V7.parse().unwrap(),
            WorkState::CancelRequested,
            CancellationCleanupDisposition::Pending,
            cursor,
        )
        .unwrap();
        let json = encode_cancellation_receipt(&receipt).unwrap();
        assert_eq!(
            decode_cancellation_receipt(&json, 202, cursor).unwrap(),
            receipt
        );
        assert!(decode_cancellation_receipt(&json, 200, cursor).is_err());
        assert!(decode_cancellation_receipt(&json.replace("true", "false"), 202, cursor).is_err());
        assert!(
            decode_cancellation_receipt(&json.replace("cancel_requested", "queued"), 202, cursor)
                .is_err()
        );
        let extra = json.replacen('{', "{\"extra\":true,", 1);
        assert!(decode_cancellation_receipt(&extra, 202, cursor).is_err());
    }
}
