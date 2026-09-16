//! SQLite-private shared persisted cancellation mutation.

use sqlx::Row;

use crate::domain::{
    CancellationCheckpoint, CancellationDecision, ConversationId, CorrelationId, CraxiiId,
    CurrentWorkAttempt, DeviceId, InboundDeliveryId, JournalActor, JournalCurrentAttempt,
    JournalEventId, JournalEventPayload, JournalOffset, JournalStreamId, JournalWorkTerminalReason,
    ModelInvocationId, ProjectionVersion, RuntimeInstanceId, ToolExecutionId, UserId, UtcTimestamp,
    WorkCancellationReason, WorkCancellationV2, WorkCompletionReason, WorkFailureReason, WorkId,
    WorkInterruptionReason, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput, WorkState,
    WorkTerminalReason, WorkTransitionV1, decide_cancellation,
};

use super::codec::{
    decode_cancellation_reason, decode_optional_id, decode_optional_timestamp, decode_work_state,
};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{JournalAppendIntent, append_event, prepare_event};
use super::projection::{ProjectionMutationError, WorkProjectionTimes, guarded_work_update};
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

pub(super) struct LoadedCancellationWork {
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub correlation_id: CorrelationId,
    pub snapshot: WorkLifecycleSnapshot,
    pub started_at: Option<UtcTimestamp>,
}

pub(super) fn decode_loaded_cancellation_work(
    row: &sqlx::sqlite::SqliteRow,
    work_id: WorkId,
) -> Result<LoadedCancellationWork, SqliteAdapterError> {
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
    Ok(LoadedCancellationWork {
        craxii_id: CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        conversation_id: ConversationId::parse_canonical(
            &row.try_get::<String, _>("conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
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

fn journal_attempt(value: CurrentWorkAttempt) -> JournalCurrentAttempt {
    match value {
        CurrentWorkAttempt::None => JournalCurrentAttempt::None,
        CurrentWorkAttempt::Model(id) => JournalCurrentAttempt::Model(id),
        CurrentWorkAttempt::Tool(id) => JournalCurrentAttempt::Tool(id),
    }
}

fn cancellation_transition(
    current: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    requested_at: UtcTimestamp,
) -> Result<WorkTransitionV1, SqliteAdapterError> {
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
    Ok(WorkTransitionV1 {
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
    })
}

#[derive(Clone, Copy)]
pub(super) enum CancellationJournalOrigin {
    Native {
        device_id: DeviceId,
    },
    Channel {
        user_id: UserId,
        inbound_delivery_id: InboundDeliveryId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CancellationMutation {
    pub resulting_state: WorkState,
    pub committed_cursor: Option<JournalOffset>,
}

pub(super) async fn apply_cancellation_decision(
    transaction: &mut WriteTransaction,
    work: &LoadedCancellationWork,
    requested_at: UtcTimestamp,
    event_id: JournalEventId,
    origin: CancellationJournalOrigin,
) -> Result<CancellationMutation, SqliteAdapterError> {
    let decision = decide_cancellation(
        &work.snapshot,
        CancellationCheckpoint::BeforeNextIteration,
        WorkCancellationReason::UserRequest,
    )
    .map_err(|_| inconsistent())?;
    match &decision {
        CancellationDecision::DirectCancelled { transition, .. }
        | CancellationDecision::CancellationRequested { transition, .. } => {
            let next = transition.next();
            guarded_work_update(
                transaction,
                &work.snapshot,
                next,
                WorkProjectionTimes {
                    started_at: work.started_at,
                    cancel_requested_at: (next.state() == WorkState::CancelRequested)
                        .then_some(requested_at),
                    terminal_at: (next.state() == WorkState::Cancelled).then_some(requested_at),
                },
            )
            .await
            .map_err(map_projection_error)?;
            let causation = latest_work_event(transaction, work.snapshot.work_id()).await?;
            let transition = cancellation_transition(&work.snapshot, next, requested_at)?;
            let (actor, payload) = match origin {
                CancellationJournalOrigin::Native { device_id } => (
                    JournalActor::User(Some(device_id)),
                    match next.state() {
                        WorkState::CancelRequested => {
                            JournalEventPayload::WorkCancelRequested(transition)
                        }
                        WorkState::Cancelled => JournalEventPayload::WorkCancelled(transition),
                        _ => return Err(invalid()),
                    },
                ),
                CancellationJournalOrigin::Channel {
                    user_id,
                    inbound_delivery_id,
                } => (
                    JournalActor::UserV2(user_id),
                    match next.state() {
                        WorkState::CancelRequested => {
                            JournalEventPayload::WorkCancelRequestedV2(WorkCancellationV2 {
                                transition,
                                inbound_delivery_id,
                            })
                        }
                        WorkState::Cancelled => {
                            JournalEventPayload::WorkCancelledV2(WorkCancellationV2 {
                                transition,
                                inbound_delivery_id,
                            })
                        }
                        _ => return Err(invalid()),
                    },
                ),
            };
            let position = append_event(
                transaction,
                prepare_event(JournalAppendIntent {
                    event_id,
                    craxii_id: work.craxii_id,
                    stream_id: JournalStreamId::Work(work.snapshot.work_id()),
                    conversation_id: Some(work.conversation_id),
                    work_id: Some(work.snapshot.work_id()),
                    causation_event_id: Some(causation),
                    correlation_id: work.correlation_id,
                    actor,
                    runtime_instance_id: next.runtime_owner(),
                    payload,
                    recorded_at: requested_at,
                    occurred_at: None,
                })?,
            )
            .await?;
            Ok(CancellationMutation {
                resulting_state: next.state(),
                committed_cursor: Some(position.offset),
            })
        }
        CancellationDecision::AlreadyRequestedNoOp { .. } => Ok(CancellationMutation {
            resulting_state: WorkState::CancelRequested,
            committed_cursor: None,
        }),
        CancellationDecision::AlreadyTerminalNoOp { state, .. } => Ok(CancellationMutation {
            resulting_state: *state,
            committed_cursor: None,
        }),
    }
}
