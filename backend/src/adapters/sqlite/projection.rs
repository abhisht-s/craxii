use serde::Serialize;
use sqlx::Row;

use crate::domain::{
    ConversationId, ConversationWorkOrdinal, CurrentWorkAttempt, ModelInvocationId,
    ProjectionVersion, RuntimeInstanceId, ToolExecutionId, UtcTimestamp, WorkFailureReason,
    WorkLifecycleSnapshot, WorkState, WorkTerminalReason, is_legal_work_pair,
};

use super::codec::{decode_optional_id, decode_work_state, encode_normalized_error_detail};
use super::error::SqliteAdapterError;
use super::transaction::WriteTransaction;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConversationGuardConflict {
    Missing,
    StaleVersion,
    StaleOrdinal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkGuardConflict {
    Missing,
    StaleState,
    StaleVersion,
    StaleOwner,
    WrongCurrentModel,
    WrongCurrentTool,
}

#[derive(Debug)]
pub(super) enum ProjectionMutationError<C> {
    Conflict(C),
    Storage(SqliteAdapterError),
    Invariant,
}

impl<C> From<SqliteAdapterError> for ProjectionMutationError<C> {
    fn from(error: SqliteAdapterError) -> Self {
        Self::Storage(error)
    }
}

pub(super) async fn advance_conversation_ordinal(
    transaction: &mut WriteTransaction,
    conversation_id: ConversationId,
    expected_version: ProjectionVersion,
    expected_ordinal: ConversationWorkOrdinal,
) -> Result<
    (ProjectionVersion, ConversationWorkOrdinal),
    ProjectionMutationError<ConversationGuardConflict>,
> {
    let next_version = expected_version
        .checked_increment()
        .map_err(|_| ProjectionMutationError::Invariant)?;
    let next_ordinal = expected_ordinal
        .checked_increment()
        .map_err(|_| ProjectionMutationError::Invariant)?;
    let result = sqlx::query(
        "UPDATE conversations \
         SET next_work_ordinal = ?, state_version = ? \
         WHERE conversation_id = ? AND state_version = ? AND next_work_ordinal = ?",
    )
    .bind(next_ordinal.get())
    .bind(next_version.get())
    .bind(conversation_id.to_string())
    .bind(expected_version.get())
    .bind(expected_ordinal.get())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;

    match result.rows_affected() {
        1 => Ok((next_version, next_ordinal)),
        0 => {
            let current = sqlx::query(
                "SELECT state_version, next_work_ordinal FROM conversations WHERE conversation_id = ?",
            )
            .bind(conversation_id.to_string())
            .fetch_optional(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            let Some(current) = current else {
                return Err(ProjectionMutationError::Conflict(
                    ConversationGuardConflict::Missing,
                ));
            };
            let current_version: i64 = current
                .try_get("state_version")
                .map_err(SqliteAdapterError::from_sqlx)?;
            let current_ordinal: i64 = current
                .try_get("next_work_ordinal")
                .map_err(SqliteAdapterError::from_sqlx)?;
            if current_version != expected_version.get() {
                Err(ProjectionMutationError::Conflict(
                    ConversationGuardConflict::StaleVersion,
                ))
            } else if current_ordinal != expected_ordinal.get() {
                Err(ProjectionMutationError::Conflict(
                    ConversationGuardConflict::StaleOrdinal,
                ))
            } else {
                Err(ProjectionMutationError::Invariant)
            }
        }
        _ => Err(ProjectionMutationError::Invariant),
    }
}

pub(super) struct WorkProjectionTimes {
    pub(super) started_at: Option<UtcTimestamp>,
    pub(super) cancel_requested_at: Option<UtcTimestamp>,
    pub(super) terminal_at: Option<UtcTimestamp>,
}

pub(super) async fn guarded_work_update(
    transaction: &mut WriteTransaction,
    expected: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    times: WorkProjectionTimes,
) -> Result<(), ProjectionMutationError<WorkGuardConflict>> {
    if expected.work_id() != next.work_id()
        || expected.state().is_terminal()
        || next.projection_version().get()
            != expected
                .projection_version()
                .get()
                .checked_add(1)
                .ok_or(ProjectionMutationError::Invariant)?
        || !is_legal_work_pair(expected.state(), next.state())
        || !valid_time_shape(next.state(), &times)
    {
        return Err(ProjectionMutationError::Invariant);
    }

    let (expected_model, expected_tool) = attempt_columns(expected.current_attempt());
    let (next_model, next_tool) = attempt_columns(next.current_attempt());
    let (terminal_reason, terminal_detail) = terminal_columns(next.terminal_reason())?;
    let cancellation_reason = next.cancellation_reason().map(|reason| reason.as_str());
    let next_owner = next.runtime_owner().map(|id| id.to_string());
    let expected_owner = expected.runtime_owner().map(|id| id.to_string());
    let result = sqlx::query(
        "UPDATE work_items SET \
            state = ?, state_version = ?, runtime_instance_id = ?, \
            current_model_invocation_id = ?, current_tool_execution_id = ?, \
            started_at = ?, cancel_requested_at = ?, cancellation_reason_code = ?, \
            terminal_at = ?, terminal_reason_code = ?, terminal_detail_json = ? \
         WHERE work_id = ? AND state = ? AND state_version = ? \
            AND runtime_instance_id IS ? \
            AND current_model_invocation_id IS ? \
            AND current_tool_execution_id IS ?",
    )
    .bind(next.state().as_str())
    .bind(next.projection_version().get())
    .bind(next_owner.as_deref())
    .bind(next_model.as_deref())
    .bind(next_tool.as_deref())
    .bind(times.started_at.map(|value| value.to_string()))
    .bind(times.cancel_requested_at.map(|value| value.to_string()))
    .bind(cancellation_reason)
    .bind(times.terminal_at.map(|value| value.to_string()))
    .bind(terminal_reason)
    .bind(terminal_detail)
    .bind(expected.work_id().to_string())
    .bind(expected.state().as_str())
    .bind(expected.projection_version().get())
    .bind(expected_owner.as_deref())
    .bind(expected_model.as_deref())
    .bind(expected_tool.as_deref())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;

    match result.rows_affected() {
        1 => Ok(()),
        0 => classify_work_conflict(transaction, expected).await,
        _ => Err(ProjectionMutationError::Invariant),
    }
}

async fn classify_work_conflict(
    transaction: &mut WriteTransaction,
    expected: &WorkLifecycleSnapshot,
) -> Result<(), ProjectionMutationError<WorkGuardConflict>> {
    let row = sqlx::query(
        "SELECT state, state_version, runtime_instance_id, current_model_invocation_id, \
         current_tool_execution_id FROM work_items WHERE work_id = ?",
    )
    .bind(expected.work_id().to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let Some(row) = row else {
        return Err(ProjectionMutationError::Conflict(
            WorkGuardConflict::Missing,
        ));
    };

    let state = decode_work_state(
        &row.try_get::<String, _>("state")
            .map_err(SqliteAdapterError::from_sqlx)?,
    )?;
    let version = row
        .try_get::<i64, _>("state_version")
        .map_err(SqliteAdapterError::from_sqlx)?;
    let owner: Option<RuntimeInstanceId> = decode_optional_id(
        row.try_get::<Option<String>, _>("runtime_instance_id")
            .map_err(SqliteAdapterError::from_sqlx)?
            .as_deref(),
    )?;
    let model: Option<ModelInvocationId> = decode_optional_id(
        row.try_get::<Option<String>, _>("current_model_invocation_id")
            .map_err(SqliteAdapterError::from_sqlx)?
            .as_deref(),
    )?;
    let tool: Option<ToolExecutionId> = decode_optional_id(
        row.try_get::<Option<String>, _>("current_tool_execution_id")
            .map_err(SqliteAdapterError::from_sqlx)?
            .as_deref(),
    )?;
    let (expected_model, expected_tool) = match expected.current_attempt() {
        CurrentWorkAttempt::None => (None, None),
        CurrentWorkAttempt::Model(id) => (Some(id), None),
        CurrentWorkAttempt::Tool(id) => (None, Some(id)),
    };

    let conflict = if state != expected.state() {
        WorkGuardConflict::StaleState
    } else if version != expected.projection_version().get() {
        WorkGuardConflict::StaleVersion
    } else if owner != expected.runtime_owner() {
        WorkGuardConflict::StaleOwner
    } else if model != expected_model {
        WorkGuardConflict::WrongCurrentModel
    } else if tool != expected_tool {
        WorkGuardConflict::WrongCurrentTool
    } else {
        return Err(ProjectionMutationError::Invariant);
    };
    Err(ProjectionMutationError::Conflict(conflict))
}

fn attempt_columns(attempt: CurrentWorkAttempt) -> (Option<String>, Option<String>) {
    match attempt {
        CurrentWorkAttempt::None => (None, None),
        CurrentWorkAttempt::Model(id) => (Some(id.to_string()), None),
        CurrentWorkAttempt::Tool(id) => (None, Some(id.to_string())),
    }
}

fn valid_time_shape(state: WorkState, times: &WorkProjectionTimes) -> bool {
    match state {
        WorkState::Queued => {
            times.started_at.is_none()
                && times.cancel_requested_at.is_none()
                && times.terminal_at.is_none()
        }
        WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool => {
            times.started_at.is_some()
                && times.cancel_requested_at.is_none()
                && times.terminal_at.is_none()
        }
        WorkState::CancelRequested => {
            times.started_at.is_some()
                && times.cancel_requested_at.is_some()
                && times.terminal_at.is_none()
        }
        WorkState::Completed | WorkState::Failed | WorkState::Interrupted => {
            times.started_at.is_some()
                && times.cancel_requested_at.is_none()
                && times.terminal_at.is_some()
        }
        WorkState::Cancelled => times.cancel_requested_at.is_none() && times.terminal_at.is_some(),
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct InvalidModelOutputDetail {
    version: u8,
    failure: &'static str,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct LifecycleLimitDetail {
    version: u8,
    failure: &'static str,
    limit: &'static str,
}

fn terminal_columns(
    reason: Option<&WorkTerminalReason>,
) -> Result<(Option<&'static str>, Option<String>), ProjectionMutationError<WorkGuardConflict>> {
    let Some(reason) = reason else {
        return Ok((None, None));
    };
    match reason {
        WorkTerminalReason::Completion(reason) => Ok((Some(reason.as_str()), None)),
        WorkTerminalReason::Cancellation(reason) => Ok((Some(reason.as_str()), None)),
        WorkTerminalReason::Interruption(reason) => Ok((Some(reason.as_str()), None)),
        WorkTerminalReason::Failure(WorkFailureReason::ProviderExhausted) => {
            Ok((Some("provider_exhausted"), None))
        }
        WorkTerminalReason::Failure(WorkFailureReason::Definite(error)) => Ok((
            Some("definite_normalized_error"),
            Some(encode_normalized_error_detail(error)?),
        )),
        WorkTerminalReason::Failure(WorkFailureReason::InvalidModelOutput(reason)) => Ok((
            Some("invalid_model_output"),
            Some(
                serde_json::to_string(&InvalidModelOutputDetail {
                    version: 1,
                    failure: "invalid_model_output",
                    reason: reason.as_str(),
                })
                .map_err(|_| ProjectionMutationError::Invariant)?,
            ),
        )),
        WorkTerminalReason::Failure(WorkFailureReason::Limit(limit)) => Ok((
            Some("lifecycle_limit"),
            Some(
                serde_json::to_string(&LifecycleLimitDetail {
                    version: 1,
                    failure: "lifecycle_limit",
                    limit: limit.as_str(),
                })
                .map_err(|_| ProjectionMutationError::Invariant)?,
            ),
        )),
    }
}
