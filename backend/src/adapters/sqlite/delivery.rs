//! Durable provider-neutral outbound-delivery persistence and state transitions.

use std::time::Duration;

use sqlx::Row;

use crate::application::delivery_planner::{
    DELIVERY_DEADLINE_SECONDS, render_assistant_v1, render_control_acknowledgement,
};
use crate::domain::{
    ChannelDispatchResult, ChannelProviderId, ControlAcknowledgementOutcome, DeliveryFailure,
    DeliveryFailureClass, DeliverySource, ExternalConversationId, ExternalMessageId,
    ExternalThreadId, OutboundDelivery, OutboundDeliveryId, OutboundDeliveryState,
    PreparedChannelDispatch, RuntimeInstanceId, Sha256Digest, UtcTimestamp,
};
use crate::ports::delivery_store::{
    ClaimDeliveryRequest, DeliveryClaim, DeliveryRecoveryReceipt, DeliveryRoute, DeliveryStore,
    DeliveryStoreError, DeliveryStoreErrorKind, DeliveryStoreFuture, DeliverySummary,
    ListDeliverySummariesRequest, LoadDeliveryRouteRequest, PersistDispatchResultDisposition,
    PersistDispatchResultRequest, RecoverDeliveriesRequest, ShutdownDeliveryRequest,
};

use super::codec::{decode_message_row, decode_timestamp};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::state_store::SqliteStateStore;
use super::transaction::WriteTransaction;

const DISPATCH_MATERIAL_MAGIC: &[u8] = b"craxii.dispatch-material";
const DISPATCH_MATERIAL_VERSION: u8 = 1;
const MAX_ATTEMPTS: u16 = 8;

fn expected_deadline(created_at: UtcTimestamp) -> Option<UtcTimestamp> {
    UtcTimestamp::from_offset_datetime(
        created_at
            .to_offset_datetime()
            .checked_add(time::Duration::seconds(DELIVERY_DEADLINE_SECONDS))?,
    )
    .ok()
}

fn error(kind: DeliveryStoreErrorKind) -> DeliveryStoreError {
    DeliveryStoreError::new(kind)
}

fn inconsistent() -> DeliveryStoreError {
    error(DeliveryStoreErrorKind::Inconsistent)
}

fn invalid() -> DeliveryStoreError {
    error(DeliveryStoreErrorKind::Invalid)
}

fn conflict() -> DeliveryStoreError {
    error(DeliveryStoreErrorKind::StateConflict)
}

fn map_sqlite(error: SqliteAdapterError) -> DeliveryStoreError {
    match error.kind() {
        SqliteFailureKind::Storage | SqliteFailureKind::BusyOrLocked => {
            DeliveryStoreError::new(DeliveryStoreErrorKind::Storage)
        }
        SqliteFailureKind::StateConflict => conflict(),
        _ => inconsistent(),
    }
}

fn map_sqlx(error: sqlx::Error) -> DeliveryStoreError {
    map_sqlite(SqliteAdapterError::from_sqlx(error))
}

impl From<sqlx::Error> for DeliveryStoreError {
    fn from(error: sqlx::Error) -> Self {
        map_sqlx(error)
    }
}

/// V1 length-framed digest over only immutable dispatch routing and payload topology.
#[must_use]
pub fn dispatch_material_sha256_v1(
    channel_account_id: crate::domain::ChannelAccountId,
    provider_id: &ChannelProviderId,
    external_conversation_id: &ExternalConversationId,
    external_thread_id: Option<&ExternalThreadId>,
    payload_sha256: Sha256Digest,
    part_ordinal: u16,
    part_count: u16,
) -> Sha256Digest {
    fn framed(bytes: &[u8], output: &mut Vec<u8>) {
        output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        output.extend_from_slice(bytes);
    }
    let mut material = Vec::with_capacity(256);
    material.extend_from_slice(DISPATCH_MATERIAL_MAGIC);
    material.push(DISPATCH_MATERIAL_VERSION);
    framed(channel_account_id.to_string().as_bytes(), &mut material);
    framed(provider_id.as_str().as_bytes(), &mut material);
    framed(external_conversation_id.as_str().as_bytes(), &mut material);
    match external_thread_id {
        Some(thread) => {
            material.push(1);
            framed(thread.as_str().as_bytes(), &mut material);
        }
        None => material.push(0),
    }
    framed(payload_sha256.as_bytes(), &mut material);
    framed(&part_ordinal.to_be_bytes(), &mut material);
    framed(&part_count.to_be_bytes(), &mut material);
    Sha256Digest::hash_bytes(&material)
}

pub(super) async fn insert_delivery(
    transaction: &mut WriteTransaction,
    delivery: &OutboundDelivery,
) -> Result<(), SqliteAdapterError> {
    if delivery.payload_text.is_empty()
        || Sha256Digest::hash_bytes(delivery.payload_text.as_bytes()) != delivery.payload_sha256
        || delivery.part_ordinal == 0
        || delivery.part_ordinal > delivery.part_count
        || delivery.part_count > 64
        || delivery.attempt_count != 0
        || delivery.updated_at != delivery.created_at
        || Some(delivery.delivery_deadline_at) != expected_deadline(delivery.created_at)
        || !matches!(
            (delivery.state, &delivery.failure),
            (OutboundDeliveryState::Queued, None)
                | (OutboundDeliveryState::PermanentFailure, Some(_))
        )
    {
        return Err(SqliteAdapterError::new(
            SqliteFailureKind::InternalInvariant,
        ));
    }
    let (source_kind, message_id, work_id, inbound_id, control_outcome) = match delivery.source {
        DeliverySource::AssistantMessage {
            message_id,
            work_id,
        } => (
            "assistant",
            Some(message_id.to_string()),
            Some(work_id.to_string()),
            None,
            None,
        ),
        DeliverySource::ControlAcknowledgement {
            inbound_delivery_id,
            outcome,
        } => (
            "control",
            None,
            None,
            Some(inbound_delivery_id.to_string()),
            Some(outcome.as_str()),
        ),
    };
    let failure_class = delivery
        .failure
        .as_ref()
        .map(|value| value.class().as_str());
    let failure_code = delivery
        .failure
        .as_ref()
        .and_then(DeliveryFailure::code)
        .map(|value| value.as_str());
    let next_attempt_at =
        (delivery.state == OutboundDeliveryState::Queued).then(|| delivery.created_at.to_string());
    let terminal_at = delivery
        .state
        .is_terminal()
        .then(|| delivery.created_at.to_string());
    sqlx::query(
        "INSERT INTO outbound_deliveries (outbound_delivery_id, craxii_id, \
         conversation_binding_id, channel_account_id, provider_key, external_conversation_id, \
         external_thread_id, source_kind, source_message_id, source_work_id, \
         source_inbound_delivery_id, control_outcome, payload_version, payload_text, \
         payload_sha256, part_ordinal, part_count, state, attempt_count, \
         dispatch_runtime_instance_id, next_attempt_at, delivery_deadline_at, \
         accepted_external_message_id, failure_class, failure_code, created_at, updated_at, \
         accepted_at, terminal_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, 0, NULL, ?, ?, NULL, \
                 ?, ?, ?, ?, NULL, ?)",
    )
    .bind(delivery.outbound_delivery_id.to_string())
    .bind(delivery.craxii_id.to_string())
    .bind(delivery.conversation_binding_id.to_string())
    .bind(delivery.channel_account_id.to_string())
    .bind(delivery.provider_id.as_str())
    .bind(delivery.external_conversation_id.as_str())
    .bind(
        delivery
            .external_thread_id
            .as_ref()
            .map(ExternalThreadId::as_str),
    )
    .bind(source_kind)
    .bind(message_id)
    .bind(work_id)
    .bind(inbound_id)
    .bind(control_outcome)
    .bind(&delivery.payload_text)
    .bind(delivery.payload_sha256.to_string())
    .bind(i64::from(delivery.part_ordinal))
    .bind(i64::from(delivery.part_count))
    .bind(delivery.state.as_str())
    .bind(next_attempt_at)
    .bind(delivery.delivery_deadline_at.to_string())
    .bind(failure_class)
    .bind(failure_code)
    .bind(delivery.created_at.to_string())
    .bind(delivery.updated_at.to_string())
    .bind(terminal_at)
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    Ok(())
}

async fn load_route(
    store: &SqliteStateStore,
    request: LoadDeliveryRouteRequest,
) -> Result<DeliveryRoute, DeliveryStoreError> {
    let mut connection = store.runtime.acquire().await.map_err(map_sqlite)?;
    let row = sqlx::query(
        "SELECT w.craxii_id, w.reply_binding_id, b.conversation_binding_id, \
                b.channel_account_id, b.external_conversation_id, b.external_thread_id, \
                b.lifecycle_state AS binding_state, a.provider_key, \
                a.lifecycle_state AS account_state \
         FROM work_items w \
         JOIN conversation_bindings b ON b.conversation_binding_id = w.reply_binding_id \
         JOIN channel_accounts a ON a.channel_account_id = b.channel_account_id \
         WHERE w.work_id = ? AND w.reply_binding_id = ? \
           AND w.craxii_id = b.craxii_id AND b.craxii_id = a.craxii_id \
           AND w.conversation_id = b.conversation_id",
    )
    .bind(request.work_id.to_string())
    .bind(request.conversation_binding_id.to_string())
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlx)?
    .ok_or_else(inconsistent)?;
    Ok(DeliveryRoute {
        craxii_id: row
            .try_get::<String, _>("craxii_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        conversation_binding_id: row
            .try_get::<String, _>("conversation_binding_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        channel_account_id: row
            .try_get::<String, _>("channel_account_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        provider_id: ChannelProviderId::try_new(row.try_get::<String, _>("provider_key")?)
            .map_err(|_| inconsistent())?,
        external_conversation_id: ExternalConversationId::try_new(
            row.try_get::<String, _>("external_conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        external_thread_id: row
            .try_get::<Option<String>, _>("external_thread_id")?
            .map(ExternalThreadId::try_new)
            .transpose()
            .map_err(|_| inconsistent())?,
        binding_active: row.try_get::<String, _>("binding_state")? == "active",
        account_active: row.try_get::<String, _>("account_state")? == "active",
    })
}

struct DispatchRow {
    id: OutboundDeliveryId,
    craxii_id: String,
    binding_id: String,
    account_id: crate::domain::ChannelAccountId,
    provider: ChannelProviderId,
    destination: ExternalConversationId,
    thread: Option<ExternalThreadId>,
    payload: String,
    payload_sha256: Sha256Digest,
    ordinal: u16,
    count: u16,
    state: OutboundDeliveryState,
    attempts: u16,
    deadline: UtcTimestamp,
    source_message_id: Option<String>,
    source_inbound_delivery_id: Option<String>,
}

fn parse_state(value: &str) -> Result<OutboundDeliveryState, DeliveryStoreError> {
    match value {
        "queued" => Ok(OutboundDeliveryState::Queued),
        "dispatching" => Ok(OutboundDeliveryState::Dispatching),
        "retry_wait" => Ok(OutboundDeliveryState::RetryWait),
        "accepted" => Ok(OutboundDeliveryState::Accepted),
        "permanent_failure" => Ok(OutboundDeliveryState::PermanentFailure),
        "outcome_unknown" => Ok(OutboundDeliveryState::OutcomeUnknown),
        _ => Err(inconsistent()),
    }
}

fn parse_failure(value: &str) -> Result<DeliveryFailureClass, DeliveryStoreError> {
    match value {
        "adapter_unavailable" => Ok(DeliveryFailureClass::AdapterUnavailable),
        "binding_revoked" => Ok(DeliveryFailureClass::BindingRevoked),
        "channel_account_disabled" => Ok(DeliveryFailureClass::ChannelAccountDisabled),
        "unsupported_payload" => Ok(DeliveryFailureClass::UnsupportedPayload),
        "profile_unavailable" => Ok(DeliveryFailureClass::ProfileUnavailable),
        "payload_too_large" => Ok(DeliveryFailureClass::PayloadTooLarge),
        "prior_part_permanent_failure" => Ok(DeliveryFailureClass::PriorPartPermanentFailure),
        "prior_part_outcome_unknown" => Ok(DeliveryFailureClass::PriorPartOutcomeUnknown),
        "retry_exhausted" => Ok(DeliveryFailureClass::RetryExhausted),
        "delivery_deadline_exceeded" => Ok(DeliveryFailureClass::DeliveryDeadlineExceeded),
        "provider_retryable" => Ok(DeliveryFailureClass::ProviderRetryable),
        "provider_permanent" => Ok(DeliveryFailureClass::ProviderPermanent),
        "provider_outcome_unknown" => Ok(DeliveryFailureClass::ProviderOutcomeUnknown),
        "stale_dispatch" => Ok(DeliveryFailureClass::StaleDispatch),
        "shutdown_interrupted" => Ok(DeliveryFailureClass::ShutdownInterrupted),
        "storage_inconsistent" => Ok(DeliveryFailureClass::StorageInconsistent),
        _ => Err(inconsistent()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryAttemptResultKind {
    Accepted,
    RetryableFailure,
    PermanentFailure,
    OutcomeUnknown,
}

fn parse_attempt_result(value: &str) -> Result<DeliveryAttemptResultKind, DeliveryStoreError> {
    match value {
        "accepted" => Ok(DeliveryAttemptResultKind::Accepted),
        "retryable_failure" => Ok(DeliveryAttemptResultKind::RetryableFailure),
        "permanent_failure" => Ok(DeliveryAttemptResultKind::PermanentFailure),
        "outcome_unknown" => Ok(DeliveryAttemptResultKind::OutcomeUnknown),
        _ => Err(inconsistent()),
    }
}

#[derive(Debug)]
struct DeliveryAttemptEvidence {
    runtime_instance_id: RuntimeInstanceId,
    attempt_number: u16,
    prior_state: OutboundDeliveryState,
    dispatch_material_version: i64,
    dispatch_material_sha256: Sha256Digest,
    started_at: UtcTimestamp,
    completed_at: Option<UtcTimestamp>,
    result_kind: Option<DeliveryAttemptResultKind>,
    failure_class: Option<DeliveryFailureClass>,
    failure_code: Option<String>,
    provider_retry_after_ms: Option<i64>,
    selected_retry_delay_ms: Option<i64>,
    scheduled_next_attempt_at: Option<UtcTimestamp>,
    accepted_external_message_id: Option<ExternalMessageId>,
}

#[derive(Debug)]
struct DeliveryParentEvidence {
    state: OutboundDeliveryState,
    attempt_count: u16,
    dispatch_runtime_instance_id: Option<RuntimeInstanceId>,
    next_attempt_at: Option<UtcTimestamp>,
    accepted_external_message_id: Option<ExternalMessageId>,
    failure_class: Option<DeliveryFailureClass>,
    failure_code: Option<String>,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
    accepted_at: Option<UtcTimestamp>,
    terminal_at: Option<UtcTimestamp>,
    channel_account_id: crate::domain::ChannelAccountId,
    provider_id: ChannelProviderId,
    external_conversation_id: ExternalConversationId,
    external_thread_id: Option<ExternalThreadId>,
    payload_sha256: Sha256Digest,
    part_ordinal: u16,
    part_count: u16,
}

fn consistency_error() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

fn decode_optional_timestamp_for_consistency(
    value: Option<String>,
) -> Result<Option<UtcTimestamp>, SqliteAdapterError> {
    value
        .map(|value| decode_timestamp(&value).map_err(|_| consistency_error()))
        .transpose()
}

fn decode_delivery_parent_evidence(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DeliveryParentEvidence, SqliteAdapterError> {
    Ok(DeliveryParentEvidence {
        state: parse_state(&row.try_get::<String, _>("state")?).map_err(|_| consistency_error())?,
        attempt_count: u16::try_from(row.try_get::<i64, _>("attempt_count")?)
            .map_err(|_| consistency_error())?,
        dispatch_runtime_instance_id: row
            .try_get::<Option<String>, _>("dispatch_runtime_instance_id")?
            .map(|value| value.parse().map_err(|_| consistency_error()))
            .transpose()?,
        next_attempt_at: decode_optional_timestamp_for_consistency(
            row.try_get("next_attempt_at")?,
        )?,
        accepted_external_message_id: row
            .try_get::<Option<String>, _>("accepted_external_message_id")?
            .map(|value| ExternalMessageId::try_new(value).map_err(|_| consistency_error()))
            .transpose()?,
        failure_class: row
            .try_get::<Option<String>, _>("failure_class")?
            .map(|value| parse_failure(&value).map_err(|_| consistency_error()))
            .transpose()?,
        failure_code: row.try_get("failure_code")?,
        created_at: decode_timestamp(&row.try_get::<String, _>("created_at")?)
            .map_err(|_| consistency_error())?,
        updated_at: decode_timestamp(&row.try_get::<String, _>("updated_at")?)
            .map_err(|_| consistency_error())?,
        accepted_at: decode_optional_timestamp_for_consistency(row.try_get("accepted_at")?)?,
        terminal_at: decode_optional_timestamp_for_consistency(row.try_get("terminal_at")?)?,
        channel_account_id: row
            .try_get::<String, _>("channel_account_id")?
            .parse()
            .map_err(|_| consistency_error())?,
        provider_id: ChannelProviderId::try_new(row.try_get::<String, _>("provider_key")?)
            .map_err(|_| consistency_error())?,
        external_conversation_id: ExternalConversationId::try_new(
            row.try_get::<String, _>("external_conversation_id")?,
        )
        .map_err(|_| consistency_error())?,
        external_thread_id: row
            .try_get::<Option<String>, _>("external_thread_id")?
            .map(ExternalThreadId::try_new)
            .transpose()
            .map_err(|_| consistency_error())?,
        payload_sha256: Sha256Digest::parse_canonical(&row.try_get::<String, _>("payload_sha256")?)
            .map_err(|_| consistency_error())?,
        part_ordinal: u16::try_from(row.try_get::<i64, _>("part_ordinal")?)
            .map_err(|_| consistency_error())?,
        part_count: u16::try_from(row.try_get::<i64, _>("part_count")?)
            .map_err(|_| consistency_error())?,
    })
}

fn decode_delivery_attempt_evidence(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DeliveryAttemptEvidence, SqliteAdapterError> {
    Ok(DeliveryAttemptEvidence {
        runtime_instance_id: row
            .try_get::<String, _>("runtime_instance_id")?
            .parse()
            .map_err(|_| consistency_error())?,
        attempt_number: u16::try_from(row.try_get::<i64, _>("attempt_number")?)
            .map_err(|_| consistency_error())?,
        prior_state: parse_state(&row.try_get::<String, _>("prior_state")?)
            .map_err(|_| consistency_error())?,
        dispatch_material_version: row.try_get("dispatch_material_version")?,
        dispatch_material_sha256: Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("dispatch_material_sha256")?,
        )
        .map_err(|_| consistency_error())?,
        started_at: decode_timestamp(&row.try_get::<String, _>("started_at")?)
            .map_err(|_| consistency_error())?,
        completed_at: decode_optional_timestamp_for_consistency(row.try_get("completed_at")?)?,
        result_kind: row
            .try_get::<Option<String>, _>("result_kind")?
            .map(|value| parse_attempt_result(&value).map_err(|_| consistency_error()))
            .transpose()?,
        failure_class: row
            .try_get::<Option<String>, _>("failure_class")?
            .map(|value| parse_failure(&value).map_err(|_| consistency_error()))
            .transpose()?,
        failure_code: row.try_get("failure_code")?,
        provider_retry_after_ms: row.try_get("provider_retry_after_ms")?,
        selected_retry_delay_ms: row.try_get("selected_retry_delay_ms")?,
        scheduled_next_attempt_at: decode_optional_timestamp_for_consistency(
            row.try_get("scheduled_next_attempt_at")?,
        )?,
        accepted_external_message_id: row
            .try_get::<Option<String>, _>("accepted_external_message_id")?
            .map(|value| ExternalMessageId::try_new(value).map_err(|_| consistency_error()))
            .transpose()?,
    })
}

fn retry_schedule_is_canonical(
    attempt: &DeliveryAttemptEvidence,
) -> Result<bool, SqliteAdapterError> {
    let Some(selected_ms) = attempt.selected_retry_delay_ms else {
        return Ok(false);
    };
    if selected_ms < 1_000
        || attempt
            .provider_retry_after_ms
            .is_some_and(|provider_ms| provider_ms < 0 || provider_ms > selected_ms)
    {
        return Ok(false);
    }
    let selected_ms = u64::try_from(selected_ms).map_err(|_| consistency_error())?;
    let expected = add_duration(
        attempt.completed_at.ok_or_else(consistency_error)?,
        Duration::from_millis(selected_ms),
    )
    .map_err(|_| consistency_error())?;
    Ok(attempt
        .scheduled_next_attempt_at
        .is_none_or(|scheduled| scheduled == expected))
}

fn is_zero_attempt_permanent_failure(failure: DeliveryFailureClass) -> bool {
    matches!(
        failure,
        DeliveryFailureClass::BindingRevoked
            | DeliveryFailureClass::ChannelAccountDisabled
            | DeliveryFailureClass::UnsupportedPayload
            | DeliveryFailureClass::ProfileUnavailable
            | DeliveryFailureClass::PayloadTooLarge
            | DeliveryFailureClass::PriorPartPermanentFailure
            | DeliveryFailureClass::PriorPartOutcomeUnknown
            | DeliveryFailureClass::DeliveryDeadlineExceeded
    )
}

fn is_predispatch_permanent_failure(failure: DeliveryFailureClass) -> bool {
    matches!(
        failure,
        DeliveryFailureClass::BindingRevoked
            | DeliveryFailureClass::ChannelAccountDisabled
            | DeliveryFailureClass::PriorPartPermanentFailure
            | DeliveryFailureClass::PriorPartOutcomeUnknown
            | DeliveryFailureClass::DeliveryDeadlineExceeded
    )
}

/// Reconciles the mutable delivery projection with its complete ordered attempt history.
/// This is deliberately separate from source/topology verification below.
fn verify_delivery_attempt_reconciliation(
    delivery_row: &sqlx::sqlite::SqliteRow,
    attempt_rows: &[sqlx::sqlite::SqliteRow],
) -> Result<(), SqliteAdapterError> {
    let delivery = decode_delivery_parent_evidence(delivery_row)?;
    let attempts = attempt_rows
        .iter()
        .map(decode_delivery_attempt_evidence)
        .collect::<Result<Vec<_>, _>>()?;
    if attempts.len() != usize::from(delivery.attempt_count) {
        return Err(consistency_error());
    }

    let expected_material = dispatch_material_sha256_v1(
        delivery.channel_account_id,
        &delivery.provider_id,
        &delivery.external_conversation_id,
        delivery.external_thread_id.as_ref(),
        delivery.payload_sha256,
        delivery.part_ordinal,
        delivery.part_count,
    );
    let mut open_attempts = 0_u16;
    for (index, attempt) in attempts.iter().enumerate() {
        let expected_number = u16::try_from(index + 1).map_err(|_| consistency_error())?;
        let expected_prior = if expected_number == 1 {
            OutboundDeliveryState::Queued
        } else {
            OutboundDeliveryState::RetryWait
        };
        if attempt.attempt_number != expected_number
            || attempt.prior_state != expected_prior
            || attempt.dispatch_material_version != 1
            || attempt.dispatch_material_sha256 != expected_material
            || attempt.started_at < delivery.created_at
            || attempt
                .completed_at
                .is_some_and(|completed| completed < attempt.started_at)
        {
            return Err(consistency_error());
        }

        match (attempt.completed_at, attempt.result_kind) {
            (None, None) => {
                open_attempts = open_attempts.checked_add(1).ok_or_else(consistency_error)?;
                if attempt.failure_class.is_some()
                    || attempt.failure_code.is_some()
                    || attempt.provider_retry_after_ms.is_some()
                    || attempt.selected_retry_delay_ms.is_some()
                    || attempt.scheduled_next_attempt_at.is_some()
                    || attempt.accepted_external_message_id.is_some()
                {
                    return Err(consistency_error());
                }
            }
            (Some(_), Some(DeliveryAttemptResultKind::Accepted)) => {
                if attempt.failure_class.is_some()
                    || attempt.failure_code.is_some()
                    || attempt.provider_retry_after_ms.is_some()
                    || attempt.selected_retry_delay_ms.is_some()
                    || attempt.scheduled_next_attempt_at.is_some()
                {
                    return Err(consistency_error());
                }
            }
            (Some(_), Some(DeliveryAttemptResultKind::RetryableFailure)) => {
                if attempt.failure_class != Some(DeliveryFailureClass::ProviderRetryable)
                    || attempt.accepted_external_message_id.is_some()
                    || !retry_schedule_is_canonical(attempt)?
                {
                    return Err(consistency_error());
                }
            }
            (Some(_), Some(DeliveryAttemptResultKind::PermanentFailure)) => {
                if attempt.failure_class.is_none()
                    || matches!(
                        attempt.failure_class,
                        Some(
                            DeliveryFailureClass::ProviderRetryable
                                | DeliveryFailureClass::ProviderOutcomeUnknown
                                | DeliveryFailureClass::StaleDispatch
                                | DeliveryFailureClass::ShutdownInterrupted
                        )
                    )
                    || attempt.provider_retry_after_ms.is_some()
                    || attempt.selected_retry_delay_ms.is_some()
                    || attempt.scheduled_next_attempt_at.is_some()
                    || attempt.accepted_external_message_id.is_some()
                {
                    return Err(consistency_error());
                }
            }
            (Some(_), Some(DeliveryAttemptResultKind::OutcomeUnknown)) => {
                if !matches!(
                    attempt.failure_class,
                    Some(
                        DeliveryFailureClass::ProviderOutcomeUnknown
                            | DeliveryFailureClass::StaleDispatch
                            | DeliveryFailureClass::ShutdownInterrupted
                    )
                ) || attempt.provider_retry_after_ms.is_some()
                    || attempt.selected_retry_delay_ms.is_some()
                    || attempt.scheduled_next_attempt_at.is_some()
                    || attempt.accepted_external_message_id.is_some()
                {
                    return Err(consistency_error());
                }
            }
            _ => return Err(consistency_error()),
        }

        if let Some(next) = attempts.get(index + 1)
            && (attempt.result_kind != Some(DeliveryAttemptResultKind::RetryableFailure)
                || attempt.failure_class != Some(DeliveryFailureClass::ProviderRetryable)
                || attempt.scheduled_next_attempt_at.is_none()
                || next.started_at
                    < attempt
                        .scheduled_next_attempt_at
                        .ok_or_else(consistency_error)?)
        {
            return Err(consistency_error());
        }
    }

    let latest = attempts.last();
    if delivery.state == OutboundDeliveryState::Dispatching {
        let latest = latest.ok_or_else(consistency_error)?;
        if open_attempts != 1
            || latest.completed_at.is_some()
            || latest.attempt_number != delivery.attempt_count
            || delivery.dispatch_runtime_instance_id != Some(latest.runtime_instance_id)
            || delivery.updated_at != latest.started_at
        {
            return Err(consistency_error());
        }
    } else if open_attempts != 0 || delivery.dispatch_runtime_instance_id.is_some() {
        return Err(consistency_error());
    }

    match delivery.state {
        OutboundDeliveryState::Queued => {
            if !attempts.is_empty()
                || delivery.next_attempt_at != Some(delivery.created_at)
                || delivery.updated_at != delivery.created_at
            {
                return Err(consistency_error());
            }
        }
        OutboundDeliveryState::Dispatching => {}
        OutboundDeliveryState::RetryWait => {
            let latest = latest.ok_or_else(consistency_error)?;
            if latest.result_kind != Some(DeliveryAttemptResultKind::RetryableFailure)
                || latest.failure_class != Some(DeliveryFailureClass::ProviderRetryable)
                || latest.scheduled_next_attempt_at.is_none()
                || latest.scheduled_next_attempt_at != delivery.next_attempt_at
                || latest.completed_at != Some(delivery.updated_at)
            {
                return Err(consistency_error());
            }
        }
        OutboundDeliveryState::Accepted => {
            let latest = latest.ok_or_else(consistency_error)?;
            if latest.result_kind != Some(DeliveryAttemptResultKind::Accepted)
                || latest.accepted_external_message_id != delivery.accepted_external_message_id
                || latest.completed_at != Some(delivery.updated_at)
                || delivery.accepted_at != latest.completed_at
                || delivery.terminal_at != latest.completed_at
            {
                return Err(consistency_error());
            }
        }
        OutboundDeliveryState::PermanentFailure => {
            let failure = delivery.failure_class.ok_or_else(consistency_error)?;
            if delivery.terminal_at != Some(delivery.updated_at) {
                return Err(consistency_error());
            }
            match latest {
                None => {
                    if !is_zero_attempt_permanent_failure(failure)
                        || delivery.failure_code.is_some()
                    {
                        return Err(consistency_error());
                    }
                }
                Some(latest)
                    if latest.result_kind == Some(DeliveryAttemptResultKind::PermanentFailure) =>
                {
                    if latest.failure_class != Some(failure)
                        || latest.failure_code != delivery.failure_code
                        || latest.completed_at != delivery.terminal_at
                    {
                        return Err(consistency_error());
                    }
                }
                Some(latest)
                    if latest.result_kind == Some(DeliveryAttemptResultKind::RetryableFailure) =>
                {
                    let terminalized_by_result = latest.scheduled_next_attempt_at.is_none();
                    let allowed = if terminalized_by_result {
                        matches!(
                            failure,
                            DeliveryFailureClass::RetryExhausted
                                | DeliveryFailureClass::DeliveryDeadlineExceeded
                        ) && latest.completed_at == delivery.terminal_at
                    } else {
                        is_predispatch_permanent_failure(failure)
                            && latest
                                .completed_at
                                .is_some_and(|completed| completed <= delivery.updated_at)
                    };
                    if !allowed || delivery.failure_code.is_some() {
                        return Err(consistency_error());
                    }
                }
                Some(_) => return Err(consistency_error()),
            }
        }
        OutboundDeliveryState::OutcomeUnknown => {
            let latest = latest.ok_or_else(consistency_error)?;
            if latest.result_kind != Some(DeliveryAttemptResultKind::OutcomeUnknown)
                || latest.failure_class != delivery.failure_class
                || latest.failure_code != delivery.failure_code
                || latest.completed_at != Some(delivery.updated_at)
                || delivery.terminal_at != latest.completed_at
            {
                return Err(consistency_error());
            }
        }
    }
    Ok(())
}

fn decode_dispatch_row(row: &sqlx::sqlite::SqliteRow) -> Result<DispatchRow, DeliveryStoreError> {
    let ordinal =
        u16::try_from(row.try_get::<i64, _>("part_ordinal")?).map_err(|_| inconsistent())?;
    let count = u16::try_from(row.try_get::<i64, _>("part_count")?).map_err(|_| inconsistent())?;
    Ok(DispatchRow {
        id: row
            .try_get::<String, _>("outbound_delivery_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        craxii_id: row.try_get("craxii_id")?,
        binding_id: row.try_get("conversation_binding_id")?,
        account_id: row
            .try_get::<String, _>("channel_account_id")?
            .parse()
            .map_err(|_| inconsistent())?,
        provider: ChannelProviderId::try_new(row.try_get::<String, _>("provider_key")?)
            .map_err(|_| inconsistent())?,
        destination: ExternalConversationId::try_new(
            row.try_get::<String, _>("external_conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        thread: row
            .try_get::<Option<String>, _>("external_thread_id")?
            .map(ExternalThreadId::try_new)
            .transpose()
            .map_err(|_| inconsistent())?,
        payload: row.try_get("payload_text")?,
        payload_sha256: Sha256Digest::parse_canonical(&row.try_get::<String, _>("payload_sha256")?)
            .map_err(|_| inconsistent())?,
        ordinal,
        count,
        state: parse_state(&row.try_get::<String, _>("state")?)?,
        attempts: u16::try_from(row.try_get::<i64, _>("attempt_count")?)
            .map_err(|_| inconsistent())?,
        deadline: decode_timestamp(&row.try_get::<String, _>("delivery_deadline_at")?)
            .map_err(|_| inconsistent())?,
        source_message_id: row.try_get("source_message_id")?,
        source_inbound_delivery_id: row.try_get("source_inbound_delivery_id")?,
    })
}

async fn mark_permanent_before_dispatch(
    transaction: &mut WriteTransaction,
    row: &DispatchRow,
    failure: DeliveryFailureClass,
    now: UtcTimestamp,
) -> Result<u64, DeliveryStoreError> {
    let changed = sqlx::query(
        "UPDATE outbound_deliveries SET state = 'permanent_failure', next_attempt_at = NULL, \
                failure_class = ?, failure_code = NULL, terminal_at = ?, updated_at = ? \
         WHERE outbound_delivery_id = ? AND state IN ('queued', 'retry_wait')",
    )
    .bind(failure.as_str())
    .bind(now.to_string())
    .bind(now.to_string())
    .bind(row.id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if changed != 1 {
        return Err(conflict());
    }
    cascade_later(
        transaction,
        row,
        DeliveryFailureClass::PriorPartPermanentFailure,
        now,
    )
    .await
}

async fn cascade_later(
    transaction: &mut WriteTransaction,
    row: &DispatchRow,
    failure: DeliveryFailureClass,
    now: UtcTimestamp,
) -> Result<u64, DeliveryStoreError> {
    sqlx::query(
        "UPDATE outbound_deliveries SET state = 'permanent_failure', next_attempt_at = NULL, \
                dispatch_runtime_instance_id = NULL, failure_class = ?, failure_code = NULL, \
                terminal_at = ?, updated_at = ? \
         WHERE part_ordinal > ? AND state IN ('queued', 'retry_wait') AND ( \
             (source_kind = 'assistant' AND source_message_id = ?) OR \
             (source_kind = 'control' AND source_inbound_delivery_id = ?))",
    )
    .bind(failure.as_str())
    .bind(now.to_string())
    .bind(now.to_string())
    .bind(i64::from(row.ordinal))
    .bind(row.source_message_id.as_deref())
    .bind(row.source_inbound_delivery_id.as_deref())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)
    .map(|result| result.rows_affected())
}

async fn claim(
    store: &SqliteStateStore,
    request: ClaimDeliveryRequest,
) -> Result<DeliveryClaim, DeliveryStoreError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "claim_outbound_delivery")
        .await
        .map_err(map_sqlite)?;
    let selected = sqlx::query(
        "SELECT d.* FROM outbound_deliveries d \
         WHERE d.state IN ('queued', 'retry_wait') AND d.next_attempt_at <= ? \
           AND NOT EXISTS (SELECT 1 FROM outbound_deliveries p WHERE p.part_ordinal < d.part_ordinal \
             AND p.state <> 'accepted' AND ((p.source_kind = 'assistant' AND p.source_message_id = d.source_message_id) \
               OR (p.source_kind = 'control' AND p.source_inbound_delivery_id = d.source_inbound_delivery_id))) \
         ORDER BY d.next_attempt_at ASC, d.created_at ASC, d.outbound_delivery_id ASC LIMIT 1",
    )
    .bind(request.now.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let Some(selected) = selected else {
        transaction.commit().await.map_err(map_sqlite)?;
        return Ok(DeliveryClaim::NoneDue);
    };
    let row = decode_dispatch_row(&selected)?;
    if Sha256Digest::hash_bytes(row.payload.as_bytes()) != row.payload_sha256 {
        return Err(inconsistent());
    }
    let (attempt_rows, maximum_attempt, open_attempts): (i64, Option<i64>, i64) = sqlx::query_as(
        "SELECT COUNT(*), MAX(attempt_number), \
                    COALESCE(SUM(CASE WHEN completed_at IS NULL THEN 1 ELSE 0 END), 0) \
             FROM outbound_delivery_attempts WHERE outbound_delivery_id = ?",
    )
    .bind(row.id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    if attempt_rows != i64::from(row.attempts)
        || maximum_attempt != (row.attempts != 0).then_some(i64::from(row.attempts))
        || open_attempts != 0
    {
        return Err(inconsistent());
    }
    let route = sqlx::query(
        "SELECT b.craxii_id, b.channel_account_id, b.external_conversation_id, \
                b.external_thread_id, b.lifecycle_state AS binding_state, \
                a.provider_key, a.lifecycle_state AS account_state \
         FROM conversation_bindings b JOIN channel_accounts a \
           ON a.channel_account_id = b.channel_account_id AND a.craxii_id = b.craxii_id \
         WHERE b.conversation_binding_id = ?",
    )
    .bind(&row.binding_id)
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .ok_or_else(inconsistent)?;
    let route_thread: Option<String> = route.try_get("external_thread_id")?;
    if route.try_get::<String, _>("craxii_id")? != row.craxii_id
        || route.try_get::<String, _>("channel_account_id")? != row.account_id.to_string()
        || route.try_get::<String, _>("external_conversation_id")? != row.destination.as_str()
        || route_thread.as_deref() != row.thread.as_ref().map(ExternalThreadId::as_str)
        || route.try_get::<String, _>("provider_key")? != row.provider.as_str()
    {
        return Err(inconsistent());
    }
    let predispatch_failure = if route.try_get::<String, _>("binding_state")? != "active" {
        Some(DeliveryFailureClass::BindingRevoked)
    } else if route.try_get::<String, _>("account_state")? != "active" {
        Some(DeliveryFailureClass::ChannelAccountDisabled)
    } else if request.now >= row.deadline {
        Some(DeliveryFailureClass::DeliveryDeadlineExceeded)
    } else if row.attempts >= MAX_ATTEMPTS {
        Some(DeliveryFailureClass::RetryExhausted)
    } else {
        None
    };
    if let Some(failure) = predispatch_failure {
        mark_permanent_before_dispatch(&mut transaction, &row, failure, request.now).await?;
        transaction.commit().await.map_err(map_sqlite)?;
        return Ok(DeliveryClaim::StateAdvanced);
    }

    let attempt_number = row.attempts.checked_add(1).ok_or_else(inconsistent)?;
    let dispatch_material = dispatch_material_sha256_v1(
        row.account_id,
        &row.provider,
        &row.destination,
        row.thread.as_ref(),
        row.payload_sha256,
        row.ordinal,
        row.count,
    );
    let changed = sqlx::query(
        "UPDATE outbound_deliveries SET state = 'dispatching', attempt_count = ?, \
                dispatch_runtime_instance_id = ?, next_attempt_at = NULL, updated_at = ? \
         WHERE outbound_delivery_id = ? AND state = ? AND attempt_count = ?",
    )
    .bind(i64::from(attempt_number))
    .bind(request.runtime_instance_id.to_string())
    .bind(request.now.to_string())
    .bind(row.id.to_string())
    .bind(row.state.as_str())
    .bind(i64::from(row.attempts))
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if changed != 1 {
        return Err(conflict());
    }
    sqlx::query(
        "INSERT INTO outbound_delivery_attempts (outbound_delivery_attempt_id, \
         outbound_delivery_id, runtime_instance_id, attempt_number, prior_state, \
         dispatch_material_version, dispatch_material_sha256, started_at) \
         VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(request.outbound_delivery_attempt_id.to_string())
    .bind(row.id.to_string())
    .bind(request.runtime_instance_id.to_string())
    .bind(i64::from(attempt_number))
    .bind(row.state.as_str())
    .bind(dispatch_material.to_string())
    .bind(request.now.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    transaction.commit().await.map_err(map_sqlite)?;
    Ok(DeliveryClaim::Dispatch(Box::new(PreparedChannelDispatch {
        outbound_delivery_id: row.id,
        outbound_delivery_attempt_id: request.outbound_delivery_attempt_id,
        attempt_number,
        channel_account_id: row.account_id,
        provider_id: row.provider,
        external_conversation_id: row.destination,
        external_thread_id: row.thread,
        text: row.payload,
        payload_sha256: row.payload_sha256,
        dispatch_material_sha256: dispatch_material,
        part_ordinal: row.ordinal,
        part_count: row.count,
    })))
}

fn duration_ms(duration: Duration) -> Result<i64, DeliveryStoreError> {
    let rounded_up = duration
        .as_nanos()
        .checked_add(999_999)
        .ok_or_else(invalid)?
        / 1_000_000;
    i64::try_from(rounded_up).map_err(|_| invalid())
}

fn retry_timing(
    local: Duration,
    provider: Option<Duration>,
) -> Result<(Option<i64>, i64, Duration), DeliveryStoreError> {
    let local_ms = duration_ms(local.max(Duration::from_secs(1)))?;
    let provider_ms = provider.map(duration_ms).transpose()?;
    let selected_ms = provider_ms.map_or(local_ms, |value| value.max(local_ms));
    let selected = Duration::from_millis(u64::try_from(selected_ms).map_err(|_| invalid())?);
    Ok((provider_ms, selected_ms, selected))
}

fn add_duration(
    timestamp: UtcTimestamp,
    duration: Duration,
) -> Result<UtcTimestamp, DeliveryStoreError> {
    let nanos = i64::try_from(duration.as_nanos()).map_err(|_| invalid())?;
    UtcTimestamp::from_offset_datetime(
        timestamp
            .to_offset_datetime()
            .checked_add(time::Duration::nanoseconds(nanos))
            .ok_or_else(invalid)?,
    )
    .map_err(|_| invalid())
}

fn result_evidence(
    result: &ChannelDispatchResult,
) -> (
    &'static str,
    Option<&DeliveryFailure>,
    Option<&ExternalMessageId>,
) {
    match result {
        ChannelDispatchResult::Accepted {
            external_message_id,
        } => ("accepted", None, external_message_id.as_ref()),
        ChannelDispatchResult::RetryableFailure { failure, .. } => {
            ("retryable_failure", Some(failure), None)
        }
        ChannelDispatchResult::PermanentFailure { failure } => {
            ("permanent_failure", Some(failure), None)
        }
        ChannelDispatchResult::OutcomeUnknown { failure } => {
            ("outcome_unknown", Some(failure), None)
        }
    }
}

async fn persist_result(
    store: &SqliteStateStore,
    request: PersistDispatchResultRequest,
) -> Result<PersistDispatchResultDisposition, DeliveryStoreError> {
    match &request.result {
        ChannelDispatchResult::RetryableFailure { failure, .. }
            if failure.class() != DeliveryFailureClass::ProviderRetryable =>
        {
            return Err(invalid());
        }
        ChannelDispatchResult::PermanentFailure { failure }
            if matches!(
                failure.class(),
                DeliveryFailureClass::ProviderRetryable
                    | DeliveryFailureClass::ProviderOutcomeUnknown
                    | DeliveryFailureClass::StaleDispatch
                    | DeliveryFailureClass::ShutdownInterrupted
            ) =>
        {
            return Err(invalid());
        }
        ChannelDispatchResult::OutcomeUnknown { failure }
            if failure.class() != DeliveryFailureClass::ProviderOutcomeUnknown =>
        {
            return Err(invalid());
        }
        _ => {}
    }
    if Sha256Digest::hash_bytes(request.dispatch.text.as_bytes()) != request.dispatch.payload_sha256
        || dispatch_material_sha256_v1(
            request.dispatch.channel_account_id,
            &request.dispatch.provider_id,
            &request.dispatch.external_conversation_id,
            request.dispatch.external_thread_id.as_ref(),
            request.dispatch.payload_sha256,
            request.dispatch.part_ordinal,
            request.dispatch.part_count,
        ) != request.dispatch.dispatch_material_sha256
    {
        return Err(invalid());
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "persist_delivery_result")
        .await
        .map_err(map_sqlite)?;
    let delivery_row =
        sqlx::query("SELECT * FROM outbound_deliveries WHERE outbound_delivery_id = ?")
            .bind(request.dispatch.outbound_delivery_id.to_string())
            .fetch_optional(transaction.connection())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(conflict)?;
    let row = decode_dispatch_row(&delivery_row)?;
    let attempt = sqlx::query(
        "SELECT * FROM outbound_delivery_attempts WHERE outbound_delivery_attempt_id = ? \
         AND outbound_delivery_id = ? AND attempt_number = ?",
    )
    .bind(request.dispatch.outbound_delivery_attempt_id.to_string())
    .bind(request.dispatch.outbound_delivery_id.to_string())
    .bind(i64::from(request.dispatch.attempt_number))
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .ok_or_else(conflict)?;
    let (attempt_rows, maximum_attempt, open_attempts): (i64, Option<i64>, i64) = sqlx::query_as(
        "SELECT COUNT(*), MAX(attempt_number), \
                    COALESCE(SUM(CASE WHEN completed_at IS NULL THEN 1 ELSE 0 END), 0) \
             FROM outbound_delivery_attempts WHERE outbound_delivery_id = ?",
    )
    .bind(request.dispatch.outbound_delivery_id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    if attempt_rows != i64::from(row.attempts)
        || maximum_attempt != Some(i64::from(row.attempts))
        || open_attempts != i64::from(row.state == OutboundDeliveryState::Dispatching)
    {
        return Err(inconsistent());
    }
    let (result_kind, failure, accepted_id) = result_evidence(&request.result);
    if row.state != OutboundDeliveryState::Dispatching {
        let persisted_kind: Option<String> = attempt.try_get("result_kind")?;
        let persisted_failure: Option<String> = attempt.try_get("failure_class")?;
        let persisted_code: Option<String> = attempt.try_get("failure_code")?;
        let persisted_accepted: Option<String> = attempt.try_get("accepted_external_message_id")?;
        let persisted_completed: Option<String> = attempt.try_get("completed_at")?;
        let persisted_provider_delay: Option<i64> = attempt.try_get("provider_retry_after_ms")?;
        let persisted_selected_delay: Option<i64> = attempt.try_get("selected_retry_delay_ms")?;
        let persisted_scheduled: Option<String> = attempt.try_get("scheduled_next_attempt_at")?;
        let requested_completed = request.completed_at.to_string();
        let timing_exact = match &request.result {
            ChannelDispatchResult::RetryableFailure { retry_after, .. } => {
                let (provider_delay, selected_delay, selected) =
                    retry_timing(request.local_retry_delay.ok_or_else(invalid)?, *retry_after)?;
                let next = add_duration(request.completed_at, selected)?;
                let scheduled =
                    (row.attempts < MAX_ATTEMPTS && next < row.deadline).then(|| next.to_string());
                persisted_provider_delay == provider_delay
                    && persisted_selected_delay == Some(selected_delay)
                    && persisted_scheduled == scheduled
            }
            _ => {
                persisted_provider_delay.is_none()
                    && persisted_selected_delay.is_none()
                    && persisted_scheduled.is_none()
            }
        };
        let exact = persisted_kind.as_deref() == Some(result_kind)
            && persisted_failure.as_deref() == failure.map(|value| value.class().as_str())
            && persisted_code.as_deref()
                == failure
                    .and_then(DeliveryFailure::code)
                    .map(|value| value.as_str())
            && persisted_accepted.as_deref() == accepted_id.map(ExternalMessageId::as_str)
            && persisted_completed.as_deref() == Some(requested_completed.as_str())
            && timing_exact;
        transaction.commit().await.map_err(map_sqlite)?;
        return if row.state.is_terminal() && exact {
            Ok(PersistDispatchResultDisposition::Idempotent)
        } else {
            Err(conflict())
        };
    }
    let owner: Option<String> = delivery_row.try_get("dispatch_runtime_instance_id")?;
    let completed_at: Option<String> = attempt.try_get("completed_at")?;
    let attempt_runtime: String = attempt.try_get("runtime_instance_id")?;
    let material: String = attempt.try_get("dispatch_material_sha256")?;
    if owner.as_deref() != Some(request.runtime_instance_id.to_string().as_str())
        || attempt_runtime != request.runtime_instance_id.to_string()
        || completed_at.is_some()
        || material != request.dispatch.dispatch_material_sha256.to_string()
        || row.attempts != request.dispatch.attempt_number
        || row.account_id != request.dispatch.channel_account_id
        || row.provider != request.dispatch.provider_id
        || row.destination != request.dispatch.external_conversation_id
        || row.thread != request.dispatch.external_thread_id
        || row.payload != request.dispatch.text
        || row.ordinal != request.dispatch.part_ordinal
        || row.count != request.dispatch.part_count
    {
        return Err(conflict());
    }

    let failure_class = failure.map(|value| value.class().as_str());
    let failure_code = failure
        .and_then(DeliveryFailure::code)
        .map(|value| value.as_str());
    let mut provider_retry_after_ms = None;
    let mut selected_retry_delay_ms = None;
    let mut scheduled_next = None;
    let (delivery_state, delivery_failure, accepted_at) = match &request.result {
        ChannelDispatchResult::Accepted { .. } => (
            OutboundDeliveryState::Accepted,
            None,
            Some(request.completed_at),
        ),
        ChannelDispatchResult::RetryableFailure {
            failure,
            retry_after,
        } => {
            if failure.class() != DeliveryFailureClass::ProviderRetryable {
                return Err(invalid());
            }
            let (provider_delay, selected_delay, selected) =
                retry_timing(request.local_retry_delay.ok_or_else(invalid)?, *retry_after)?;
            provider_retry_after_ms = provider_delay;
            selected_retry_delay_ms = Some(selected_delay);
            let next = add_duration(request.completed_at, selected)?;
            if row.attempts >= MAX_ATTEMPTS {
                (
                    OutboundDeliveryState::PermanentFailure,
                    Some(DeliveryFailureClass::RetryExhausted),
                    None,
                )
            } else if next >= row.deadline {
                (
                    OutboundDeliveryState::PermanentFailure,
                    Some(DeliveryFailureClass::DeliveryDeadlineExceeded),
                    None,
                )
            } else {
                scheduled_next = Some(next);
                (OutboundDeliveryState::RetryWait, None, None)
            }
        }
        ChannelDispatchResult::PermanentFailure { .. } => (
            OutboundDeliveryState::PermanentFailure,
            failure.map(DeliveryFailure::class),
            None,
        ),
        ChannelDispatchResult::OutcomeUnknown { .. } => (
            OutboundDeliveryState::OutcomeUnknown,
            failure.map(DeliveryFailure::class),
            None,
        ),
    };
    let terminal_at = delivery_state.is_terminal().then_some(request.completed_at);
    let attempt_changed = sqlx::query(
        "UPDATE outbound_delivery_attempts SET completed_at = ?, result_kind = ?, \
                failure_class = ?, failure_code = ?, provider_retry_after_ms = ?, \
                selected_retry_delay_ms = ?, scheduled_next_attempt_at = ?, \
                accepted_external_message_id = ? \
         WHERE outbound_delivery_attempt_id = ? AND completed_at IS NULL",
    )
    .bind(request.completed_at.to_string())
    .bind(result_kind)
    .bind(failure_class)
    .bind(failure_code)
    .bind(provider_retry_after_ms)
    .bind(selected_retry_delay_ms)
    .bind(scheduled_next.map(|value| value.to_string()))
    .bind(accepted_id.map(ExternalMessageId::as_str))
    .bind(request.dispatch.outbound_delivery_attempt_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if attempt_changed != 1 {
        return Err(conflict());
    }
    let delivery_changed = sqlx::query(
        "UPDATE outbound_deliveries SET state = ?, dispatch_runtime_instance_id = NULL, \
                next_attempt_at = ?, accepted_external_message_id = ?, failure_class = ?, \
                failure_code = ?, updated_at = ?, accepted_at = ?, terminal_at = ? \
         WHERE outbound_delivery_id = ? AND state = 'dispatching' \
           AND dispatch_runtime_instance_id = ? AND attempt_count = ?",
    )
    .bind(delivery_state.as_str())
    .bind(scheduled_next.map(|value| value.to_string()))
    .bind(accepted_id.map(ExternalMessageId::as_str))
    .bind(delivery_failure.map(DeliveryFailureClass::as_str))
    .bind(if delivery_failure == failure.map(DeliveryFailure::class) {
        failure_code
    } else {
        None
    })
    .bind(request.completed_at.to_string())
    .bind(accepted_at.map(|value| value.to_string()))
    .bind(terminal_at.map(|value| value.to_string()))
    .bind(request.dispatch.outbound_delivery_id.to_string())
    .bind(request.runtime_instance_id.to_string())
    .bind(i64::from(request.dispatch.attempt_number))
    .execute(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .rows_affected();
    if delivery_changed != 1 {
        return Err(conflict());
    }
    if delivery_state == OutboundDeliveryState::PermanentFailure {
        cascade_later(
            &mut transaction,
            &row,
            DeliveryFailureClass::PriorPartPermanentFailure,
            request.completed_at,
        )
        .await?;
    } else if delivery_state == OutboundDeliveryState::OutcomeUnknown {
        cascade_later(
            &mut transaction,
            &row,
            DeliveryFailureClass::PriorPartOutcomeUnknown,
            request.completed_at,
        )
        .await?;
    }
    transaction.commit().await.map_err(map_sqlite)?;
    Ok(PersistDispatchResultDisposition::Applied)
}

async fn recover_matching(
    store: &SqliteStateStore,
    runtime: Option<crate::domain::RuntimeInstanceId>,
    at: UtcTimestamp,
    failure: DeliveryFailureClass,
) -> Result<DeliveryRecoveryReceipt, DeliveryStoreError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "recover_outbound_deliveries")
        .await
        .map_err(map_sqlite)?;
    let rows = if let Some(runtime) = runtime {
        sqlx::query("SELECT * FROM outbound_deliveries WHERE state = 'dispatching' AND dispatch_runtime_instance_id = ? ORDER BY created_at, outbound_delivery_id")
            .bind(runtime.to_string())
            .fetch_all(transaction.connection()).await.map_err(map_sqlx)?
    } else {
        sqlx::query("SELECT * FROM outbound_deliveries WHERE state = 'dispatching' ORDER BY created_at, outbound_delivery_id")
            .fetch_all(transaction.connection()).await.map_err(map_sqlx)?
    };
    let mut receipt = DeliveryRecoveryReceipt::default();
    for raw in &rows {
        let row = decode_dispatch_row(raw)?;
        let owner: String = raw
            .try_get::<Option<String>, _>("dispatch_runtime_instance_id")?
            .ok_or_else(inconsistent)?;
        let attempt = sqlx::query(
            "SELECT outbound_delivery_attempt_id FROM outbound_delivery_attempts \
             WHERE outbound_delivery_id = ? AND attempt_number = ? AND runtime_instance_id = ? \
               AND completed_at IS NULL",
        )
        .bind(row.id.to_string())
        .bind(i64::from(row.attempts))
        .bind(&owner)
        .fetch_all(transaction.connection())
        .await
        .map_err(map_sqlx)?;
        let [attempt] = attempt.as_slice() else {
            return Err(inconsistent());
        };
        let attempt_id: String = attempt.try_get("outbound_delivery_attempt_id")?;
        let changed = sqlx::query(
            "UPDATE outbound_delivery_attempts SET completed_at = ?, result_kind = 'outcome_unknown', \
                    failure_class = ?, failure_code = NULL \
             WHERE outbound_delivery_attempt_id = ? AND completed_at IS NULL",
        )
        .bind(at.to_string())
        .bind(failure.as_str())
        .bind(attempt_id)
        .execute(transaction.connection())
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        let changed = sqlx::query(
            "UPDATE outbound_deliveries SET state = 'outcome_unknown', \
                    dispatch_runtime_instance_id = NULL, failure_class = ?, failure_code = NULL, \
                    updated_at = ?, terminal_at = ? \
             WHERE outbound_delivery_id = ? AND state = 'dispatching'",
        )
        .bind(failure.as_str())
        .bind(at.to_string())
        .bind(at.to_string())
        .bind(row.id.to_string())
        .execute(transaction.connection())
        .await
        .map_err(map_sqlx)?
        .rows_affected();
        if changed != 1 {
            return Err(conflict());
        }
        receipt.dispatches_marked_unknown += 1;
        receipt.later_parts_blocked += cascade_later(
            &mut transaction,
            &row,
            DeliveryFailureClass::PriorPartOutcomeUnknown,
            at,
        )
        .await?;
    }
    transaction.commit().await.map_err(map_sqlite)?;
    Ok(receipt)
}

fn decode_source(row: &sqlx::sqlite::SqliteRow) -> Result<DeliverySource, DeliveryStoreError> {
    match row.try_get::<String, _>("source_kind")?.as_str() {
        "assistant" => Ok(DeliverySource::AssistantMessage {
            message_id: row
                .try_get::<String, _>("source_message_id")?
                .parse()
                .map_err(|_| inconsistent())?,
            work_id: row
                .try_get::<String, _>("source_work_id")?
                .parse()
                .map_err(|_| inconsistent())?,
        }),
        "control" => Ok(DeliverySource::ControlAcknowledgement {
            inbound_delivery_id: row
                .try_get::<String, _>("source_inbound_delivery_id")?
                .parse()
                .map_err(|_| inconsistent())?,
            outcome: match row.try_get::<String, _>("control_outcome")?.as_str() {
                "applied" => ControlAcknowledgementOutcome::Applied,
                "no_op" => ControlAcknowledgementOutcome::NoOp,
                _ => return Err(inconsistent()),
            },
        }),
        _ => Err(inconsistent()),
    }
}

async fn list_summaries(
    store: &SqliteStateStore,
    request: ListDeliverySummariesRequest,
) -> Result<Vec<DeliverySummary>, DeliveryStoreError> {
    if request.limit == 0 || request.limit > 100 || request.states.is_empty() {
        return Err(invalid());
    }
    let mut connection = store.runtime.acquire().await.map_err(map_sqlite)?;
    let rows =
        sqlx::query("SELECT * FROM outbound_deliveries ORDER BY created_at, outbound_delivery_id")
            .fetch_all(&mut *connection)
            .await
            .map_err(map_sqlx)?;
    let mut after_seen = request.after.is_none();
    let mut summaries = Vec::new();
    for row in &rows {
        let id: OutboundDeliveryId = row
            .try_get::<String, _>("outbound_delivery_id")?
            .parse()
            .map_err(|_| inconsistent())?;
        if !after_seen {
            if Some(id) == request.after {
                after_seen = true;
            }
            continue;
        }
        let state = parse_state(&row.try_get::<String, _>("state")?)?;
        if !request.states.contains(&state) {
            continue;
        }
        let failure_class = row
            .try_get::<Option<String>, _>("failure_class")?
            .map(|value| parse_failure(&value))
            .transpose()?;
        summaries.push(DeliverySummary {
            outbound_delivery_id: id,
            source: decode_source(row)?,
            channel_account_id: row
                .try_get::<String, _>("channel_account_id")?
                .parse()
                .map_err(|_| inconsistent())?,
            provider_id: ChannelProviderId::try_new(row.try_get::<String, _>("provider_key")?)
                .map_err(|_| inconsistent())?,
            part_ordinal: u16::try_from(row.try_get::<i64, _>("part_ordinal")?)
                .map_err(|_| inconsistent())?,
            part_count: u16::try_from(row.try_get::<i64, _>("part_count")?)
                .map_err(|_| inconsistent())?,
            state,
            attempt_count: u16::try_from(row.try_get::<i64, _>("attempt_count")?)
                .map_err(|_| inconsistent())?,
            created_at: decode_timestamp(&row.try_get::<String, _>("created_at")?)
                .map_err(|_| inconsistent())?,
            updated_at: decode_timestamp(&row.try_get::<String, _>("updated_at")?)
                .map_err(|_| inconsistent())?,
            next_attempt_at: row
                .try_get::<Option<String>, _>("next_attempt_at")?
                .map(|value| decode_timestamp(&value).map_err(|_| inconsistent()))
                .transpose()?,
            delivery_deadline_at: decode_timestamp(
                &row.try_get::<String, _>("delivery_deadline_at")?,
            )
            .map_err(|_| inconsistent())?,
            terminal_at: row
                .try_get::<Option<String>, _>("terminal_at")?
                .map(|value| decode_timestamp(&value).map_err(|_| inconsistent()))
                .transpose()?,
            payload_sha256: Sha256Digest::parse_canonical(
                &row.try_get::<String, _>("payload_sha256")?,
            )
            .map_err(|_| inconsistent())?,
            failure_class,
            failure_code: row.try_get("failure_code")?,
        });
        if summaries.len() == usize::from(request.limit) {
            break;
        }
    }
    if request.after.is_some() && !after_seen {
        return Err(invalid());
    }
    Ok(summaries)
}

pub(super) async fn verify_delivery_consistency_inner(
    connection: &mut sqlx::SqliteConnection,
) -> Result<u64, SqliteAdapterError> {
    let rows = sqlx::query("SELECT * FROM outbound_deliveries ORDER BY source_kind, COALESCE(source_message_id, source_inbound_delivery_id), part_ordinal")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let mut groups: std::collections::BTreeMap<(String, String), Vec<&sqlx::sqlite::SqliteRow>> =
        std::collections::BTreeMap::new();
    for row in &rows {
        let payload: String = row.try_get("payload_text")?;
        let digest = Sha256Digest::parse_canonical(&row.try_get::<String, _>("payload_sha256")?)
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        if Sha256Digest::hash_bytes(payload.as_bytes()) != digest {
            return Err(SqliteAdapterError::new(
                SqliteFailureKind::InconsistentSchema,
            ));
        }
        let created_at = decode_timestamp(&row.try_get::<String, _>("created_at")?)
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        let deadline = decode_timestamp(&row.try_get::<String, _>("delivery_deadline_at")?)
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        if Some(deadline) != expected_deadline(created_at) {
            return Err(SqliteAdapterError::new(
                SqliteFailureKind::InconsistentSchema,
            ));
        }
        let route = sqlx::query(
            "SELECT b.craxii_id, b.channel_account_id, b.external_conversation_id, \
                    b.external_thread_id, a.provider_key \
             FROM conversation_bindings b JOIN channel_accounts a \
               ON a.channel_account_id = b.channel_account_id AND a.craxii_id = b.craxii_id \
             WHERE b.conversation_binding_id = ?",
        )
        .bind(row.try_get::<String, _>("conversation_binding_id")?)
        .fetch_optional(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .ok_or_else(|| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        if route.try_get::<String, _>("craxii_id")? != row.try_get::<String, _>("craxii_id")?
            || route.try_get::<String, _>("channel_account_id")?
                != row.try_get::<String, _>("channel_account_id")?
            || route.try_get::<String, _>("external_conversation_id")?
                != row.try_get::<String, _>("external_conversation_id")?
            || route.try_get::<Option<String>, _>("external_thread_id")?
                != row.try_get::<Option<String>, _>("external_thread_id")?
            || route.try_get::<String, _>("provider_key")?
                != row.try_get::<String, _>("provider_key")?
        {
            return Err(SqliteAdapterError::new(
                SqliteFailureKind::InconsistentSchema,
            ));
        }
        let kind: String = row.try_get("source_kind")?;
        let source = if kind == "assistant" {
            row.try_get::<Option<String>, _>("source_message_id")?
        } else {
            row.try_get::<Option<String>, _>("source_inbound_delivery_id")?
        }
        .ok_or_else(|| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        groups.entry((kind, source)).or_default().push(row);

        let attempts = sqlx::query(
            "SELECT * FROM outbound_delivery_attempts WHERE outbound_delivery_id = ? \
             ORDER BY attempt_number",
        )
        .bind(row.try_get::<String, _>("outbound_delivery_id")?)
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        verify_delivery_attempt_reconciliation(row, &attempts)?;
    }
    for ((kind, source_id), parts) in groups {
        let count = i64::try_from(parts.len())
            .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
        let first = parts[0];
        let route = (
            first.try_get::<String, _>("craxii_id")?,
            first.try_get::<String, _>("conversation_binding_id")?,
            first.try_get::<String, _>("channel_account_id")?,
            first.try_get::<String, _>("provider_key")?,
            first.try_get::<String, _>("external_conversation_id")?,
            first.try_get::<Option<String>, _>("external_thread_id")?,
            first.try_get::<Option<String>, _>("source_work_id")?,
            first.try_get::<Option<String>, _>("control_outcome")?,
            first.try_get::<String, _>("created_at")?,
            first.try_get::<String, _>("delivery_deadline_at")?,
        );
        let mut concatenated = String::new();
        let mut predecessor_block: Option<DeliveryFailureClass> = None;
        for (index, part) in parts.iter().enumerate() {
            if part.try_get::<i64, _>("part_ordinal")? != i64::try_from(index + 1).unwrap_or(-1)
                || part.try_get::<i64, _>("part_count")? != count
                || (
                    part.try_get::<String, _>("craxii_id")?,
                    part.try_get::<String, _>("conversation_binding_id")?,
                    part.try_get::<String, _>("channel_account_id")?,
                    part.try_get::<String, _>("provider_key")?,
                    part.try_get::<String, _>("external_conversation_id")?,
                    part.try_get::<Option<String>, _>("external_thread_id")?,
                    part.try_get::<Option<String>, _>("source_work_id")?,
                    part.try_get::<Option<String>, _>("control_outcome")?,
                    part.try_get::<String, _>("created_at")?,
                    part.try_get::<String, _>("delivery_deadline_at")?,
                ) != route
            {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
            if let Some(expected_failure) = predecessor_block {
                if part.try_get::<String, _>("state")? != "permanent_failure"
                    || part
                        .try_get::<Option<String>, _>("failure_class")?
                        .as_deref()
                        != Some(expected_failure.as_str())
                {
                    return Err(SqliteAdapterError::new(
                        SqliteFailureKind::InconsistentSchema,
                    ));
                }
            } else {
                match part.try_get::<String, _>("state")?.as_str() {
                    "outcome_unknown" => {
                        predecessor_block = Some(DeliveryFailureClass::PriorPartOutcomeUnknown);
                    }
                    "permanent_failure" => {
                        predecessor_block = Some(DeliveryFailureClass::PriorPartPermanentFailure);
                    }
                    _ => {}
                }
            }
            concatenated.push_str(&part.try_get::<String, _>("payload_text")?);
        }
        if kind == "assistant" {
            let planning_failure = parts.iter().any(|part| {
                part.try_get::<Option<String>, _>("failure_class")
                    .ok()
                    .flatten()
                    .is_some_and(|failure| {
                        matches!(
                            failure.as_str(),
                            "unsupported_payload" | "profile_unavailable" | "payload_too_large"
                        )
                    })
            });
            if planning_failure && parts.len() != 1 {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
            let message_row = sqlx::query("SELECT m.*, w.reply_binding_id, w.work_id FROM messages m JOIN work_items w ON w.work_id = m.produced_by_work_id WHERE m.message_id = ?")
                .bind(&source_id).fetch_optional(&mut *connection).await.map_err(SqliteAdapterError::from_sqlx)?
                .ok_or_else(|| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
            let message = decode_message_row(&message_row)?;
            if message
                .produced_by_work_id()
                .map(|id| id.to_string())
                .as_deref()
                != first
                    .try_get::<Option<String>, _>("source_work_id")?
                    .as_deref()
                || message_row
                    .try_get::<Option<String>, _>("reply_binding_id")?
                    .as_deref()
                    != Some(route.1.as_str())
            {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
            let rendered = render_assistant_v1(message.content())
                .map_err(|_| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
            if concatenated != rendered {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
        } else {
            let inbound = sqlx::query("SELECT classification, control_target_work_id, conversation_binding_id FROM inbound_deliveries WHERE inbound_delivery_id = ?")
                .bind(&source_id).fetch_optional(&mut *connection).await.map_err(SqliteAdapterError::from_sqlx)?
                .ok_or_else(|| SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema))?;
            let target: Option<String> = inbound.try_get("control_target_work_id")?;
            let expected = if target.is_some() {
                ControlAcknowledgementOutcome::Applied
            } else {
                ControlAcknowledgementOutcome::NoOp
            };
            if inbound.try_get::<String, _>("classification")? != "control"
                || inbound.try_get::<String, _>("conversation_binding_id")? != route.1
                || first.try_get::<String, _>("control_outcome")? != expected.as_str()
                || concatenated != render_control_acknowledgement(expected)
            {
                return Err(SqliteAdapterError::new(
                    SqliteFailureKind::InconsistentSchema,
                ));
            }
        }
    }
    Ok(u64::try_from(rows.len()).unwrap_or(u64::MAX))
}

impl DeliveryStore for SqliteStateStore {
    fn load_delivery_route(
        &self,
        request: LoadDeliveryRouteRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRoute> {
        Box::pin(async move { load_route(self, request).await })
    }

    fn claim_next_delivery(
        &self,
        request: ClaimDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryClaim> {
        Box::pin(async move { claim(self, request).await })
    }

    fn persist_dispatch_result(
        &self,
        request: PersistDispatchResultRequest,
    ) -> DeliveryStoreFuture<'_, PersistDispatchResultDisposition> {
        Box::pin(async move { persist_result(self, request).await })
    }

    fn recover_stale_deliveries(
        &self,
        request: RecoverDeliveriesRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt> {
        Box::pin(async move {
            recover_matching(
                self,
                None,
                request.recovered_at,
                DeliveryFailureClass::StaleDispatch,
            )
            .await
        })
    }

    fn interrupt_owned_delivery(
        &self,
        request: ShutdownDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt> {
        Box::pin(async move {
            recover_matching(
                self,
                Some(request.runtime_instance_id),
                request.interrupted_at,
                DeliveryFailureClass::ShutdownInterrupted,
            )
            .await
        })
    }

    fn list_delivery_summaries(
        &self,
        request: ListDeliverySummariesRequest,
    ) -> DeliveryStoreFuture<'_, Vec<DeliverySummary>> {
        Box::pin(async move { list_summaries(self, request).await })
    }

    fn verify_delivery_consistency(&self) -> DeliveryStoreFuture<'_, u64> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(map_sqlite)?;
            verify_delivery_consistency_inner(&mut connection)
                .await
                .map_err(map_sqlite)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ChannelAccountId;

    #[test]
    fn dispatch_material_digest_is_deterministic_and_presence_framed() {
        let account: ChannelAccountId = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap();
        let provider = ChannelProviderId::try_new("fake").unwrap();
        let destination = ExternalConversationId::try_new("destination").unwrap();
        let payload = Sha256Digest::hash_bytes(b"payload");
        let first =
            dispatch_material_sha256_v1(account, &provider, &destination, None, payload, 1, 2);
        assert_eq!(
            first.to_string(),
            "d4d136fa838a5351e1b8644548e467f61ff2dc09b1c581eb7ec5dc791495996b"
        );
        assert_eq!(
            first,
            dispatch_material_sha256_v1(account, &provider, &destination, None, payload, 1, 2)
        );
        assert_ne!(
            first,
            dispatch_material_sha256_v1(
                account,
                &provider,
                &destination,
                Some(&ExternalThreadId::try_new("thread").unwrap()),
                payload,
                1,
                2
            )
        );
        assert_ne!(
            first,
            dispatch_material_sha256_v1(account, &provider, &destination, None, payload, 2, 2)
        );
    }
}
