use std::collections::{HashMap, HashSet};

use sqlx::Row;

use crate::domain::{
    Certainty, CleanupStatus, ConversationId, ConversationWorkOrdinal, CorrelationId, CraxiiId,
    CurrentWorkAttempt, JournalActor, JournalCurrentAttempt, JournalEvent, JournalEventId,
    JournalEventPayload, JournalStreamId, JournalWorkTerminalReason, ModelInvocationEventV1,
    ModelInvocationId, ModelInvocationState, NormalizedError, ProjectionVersion, RuntimeInstanceId,
    RuntimeShutdownReason, RuntimeStartedV1, ToolExecutionEventV1, ToolExecutionId,
    ToolExecutionState, UtcTimestamp, WorkCancellationReason, WorkId, WorkInterruptionReason,
    WorkItem, WorkItemInputData, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput, WorkState,
    WorkTerminalReason, WorkTransitionGuard, WorkTransitionRequest, WorkTransitionV1, WorkspaceId,
    decide_work_transition,
};
use crate::ports::state_store::{
    AppendRecoverySummaryRequest, BeginRuntimeStoppingReceipt, BeginRuntimeStoppingRequest,
    CancelRequestedWork, ClaimNextWorkRequest, ClaimedWork, ClassifyShutdownWorkRequest,
    CommitReceipt, CommittedEventRange, CreateRuntimeReceipt, CreateRuntimeRequest,
    EnumerateStaleRuntimesRequest, FinishCancellationRequest, FinishRuntimeRequest,
    HeartbeatRuntimeReceipt, HeartbeatRuntimeRequest, InterruptOwnedWorkRequest,
    RecoverStaleRuntimeRequest, RecoveryReceipt, RecoveryStateStore,
    RequestOwnedCancellationRequest, RuntimeStateStore, SchedulerStateStore, StateStoreFuture,
};

use super::codec::{decode_optional_id, decode_optional_timestamp, decode_work_state};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{JournalAppendIntent, append_event, decode_event_row, prepare_event};
use super::projection::{WorkProjectionTimes, guarded_work_update};
use super::stage8_codec::encode_attempt_error;
use super::state_store::{SqliteStateStore, map_port_error};
use super::transaction::WriteTransaction;

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

fn conflict() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::StateConflict)
}

fn invariant() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InternalInvariant)
}

pub(super) struct ActiveWorkRow {
    pub(super) work: WorkItem,
    pub(super) lifecycle: WorkLifecycleSnapshot,
    pub(super) started_at: Option<UtcTimestamp>,
    pub(super) cancel_requested_at: Option<UtcTimestamp>,
}

fn decode_cancellation_reason(
    value: Option<&str>,
) -> Result<Option<WorkCancellationReason>, SqliteAdapterError> {
    match value {
        None => Ok(None),
        Some("user_request") => Ok(Some(WorkCancellationReason::UserRequest)),
        Some("graceful_shutdown") => Ok(Some(WorkCancellationReason::GracefulShutdown)),
        Some(_) => Err(inconsistent()),
    }
}

fn decode_current_attempt(
    model: Option<&str>,
    tool: Option<&str>,
) -> Result<CurrentWorkAttempt, SqliteAdapterError> {
    match (model, tool) {
        (None, None) => Ok(CurrentWorkAttempt::None),
        (Some(model), None) => ModelInvocationId::parse_canonical(model)
            .map(CurrentWorkAttempt::Model)
            .map_err(|_| inconsistent()),
        (None, Some(tool)) => ToolExecutionId::parse_canonical(tool)
            .map(CurrentWorkAttempt::Tool)
            .map_err(|_| inconsistent()),
        (Some(_), Some(_)) => Err(inconsistent()),
    }
}

pub(super) fn decode_active_work_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ActiveWorkRow, SqliteAdapterError> {
    let work_id = WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
        .map_err(|_| inconsistent())?;
    let state = decode_work_state(&row.try_get::<String, _>("state")?)?;
    let runtime_owner = decode_optional_id(
        row.try_get::<Option<String>, _>("runtime_instance_id")?
            .as_deref(),
    )?;
    let model = row.try_get::<Option<String>, _>("current_model_invocation_id")?;
    let tool = row.try_get::<Option<String>, _>("current_tool_execution_id")?;
    let current_attempt = decode_current_attempt(model.as_deref(), tool.as_deref())?;
    let cancellation = row.try_get::<Option<String>, _>("cancellation_reason_code")?;
    let lifecycle = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id,
        state,
        projection_version: ProjectionVersion::try_new(row.try_get("state_version")?)
            .map_err(|_| inconsistent())?,
        runtime_owner,
        current_attempt,
        cancellation_reason: decode_cancellation_reason(cancellation.as_deref())?,
        terminal_reason: None,
    })
    .map_err(|_| inconsistent())?;
    let work = WorkItem::new(WorkItemInputData {
        work_id,
        craxii_id: CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        conversation_id: ConversationId::parse_canonical(
            &row.try_get::<String, _>("conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        conversation_work_ordinal: ConversationWorkOrdinal::try_new(
            row.try_get("conversation_work_ordinal")?,
        )
        .map_err(|_| inconsistent())?,
        workspace_id: WorkspaceId::parse_canonical(&row.try_get::<String, _>("workspace_id")?)
            .map_err(|_| inconsistent())?,
        correlation_id: CorrelationId::parse_canonical(
            &row.try_get::<String, _>("correlation_id")?,
        )
        .map_err(|_| inconsistent())?,
        created_at: row
            .try_get::<String, _>("created_at")?
            .parse()
            .map_err(|_| inconsistent())?,
        queued_at: row
            .try_get::<String, _>("queued_at")?
            .parse()
            .map_err(|_| inconsistent())?,
    });
    if row.try_get::<i64, _>("priority")? != 0
        || row.try_get::<String, _>("kind")? != "conversational"
    {
        return Err(inconsistent());
    }
    Ok(ActiveWorkRow {
        work,
        lifecycle,
        started_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("started_at")?.as_deref(),
        )?,
        cancel_requested_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("cancel_requested_at")?
                .as_deref(),
        )?,
    })
}

async fn latest_stream_event(
    transaction: &mut WriteTransaction,
    stream: JournalStreamId,
) -> Result<JournalEventId, SqliteAdapterError> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT event_id FROM journal_events WHERE stream_id = ? ORDER BY stream_seq DESC LIMIT 1",
    )
    .bind(stream.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    JournalEventId::parse_canonical(value.as_deref().ok_or_else(inconsistent)?)
        .map_err(|_| inconsistent())
}

fn journal_attempt(value: CurrentWorkAttempt) -> JournalCurrentAttempt {
    match value {
        CurrentWorkAttempt::None => JournalCurrentAttempt::None,
        CurrentWorkAttempt::Model(id) => JournalCurrentAttempt::Model(id),
        CurrentWorkAttempt::Tool(id) => JournalCurrentAttempt::Tool(id),
    }
}

fn terminal_reason(value: Option<&WorkTerminalReason>) -> Option<JournalWorkTerminalReason> {
    match value {
        Some(WorkTerminalReason::Cancellation(WorkCancellationReason::UserRequest)) => {
            Some(JournalWorkTerminalReason::UserRequest)
        }
        Some(WorkTerminalReason::Cancellation(WorkCancellationReason::GracefulShutdown)) => {
            Some(JournalWorkTerminalReason::GracefulShutdown)
        }
        Some(WorkTerminalReason::Interruption(WorkInterruptionReason::RuntimeOwnershipLost)) => {
            Some(JournalWorkTerminalReason::RuntimeOwnershipLost)
        }
        Some(WorkTerminalReason::Interruption(WorkInterruptionReason::ProviderOutcomeUnknown)) => {
            Some(JournalWorkTerminalReason::ProviderOutcomeUnknown)
        }
        Some(WorkTerminalReason::Interruption(
            WorkInterruptionReason::ToolInterruptedBeforeDispatch,
        )) => Some(JournalWorkTerminalReason::ToolInterruptedBeforeDispatch),
        Some(WorkTerminalReason::Interruption(WorkInterruptionReason::ToolOutcomeUnknown)) => {
            Some(JournalWorkTerminalReason::ToolOutcomeUnknown)
        }
        Some(WorkTerminalReason::Interruption(WorkInterruptionReason::CleanupUnconfirmed)) => {
            Some(JournalWorkTerminalReason::CleanupUnconfirmed)
        }
        None => None,
        _ => None,
    }
}

fn transition_payload(
    current: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    at: UtcTimestamp,
) -> WorkTransitionV1 {
    WorkTransitionV1 {
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
        terminal_reason: terminal_reason(next.terminal_reason()),
        transitioned_at: at,
    }
}

async fn append_work_transition(
    transaction: &mut WriteTransaction,
    row: &ActiveWorkRow,
    current_runtime: RuntimeInstanceId,
    event_id: JournalEventId,
    cause: JournalEventId,
    next: &WorkLifecycleSnapshot,
    at: UtcTimestamp,
) -> Result<super::journal::CommittedJournalPosition, SqliteAdapterError> {
    let value = transition_payload(&row.lifecycle, next, at);
    let payload = match next.state() {
        WorkState::Running => JournalEventPayload::WorkStarted(value),
        WorkState::CancelRequested => JournalEventPayload::WorkCancelRequested(value),
        WorkState::Cancelled => JournalEventPayload::WorkCancelled(value),
        WorkState::Interrupted => JournalEventPayload::WorkInterrupted(value),
        _ => return Err(invariant()),
    };
    append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id,
            craxii_id: row.work.craxii_id(),
            stream_id: JournalStreamId::Work(row.work.work_id()),
            conversation_id: Some(row.work.conversation_id()),
            work_id: Some(row.work.work_id()),
            causation_event_id: Some(cause),
            correlation_id: row.work.correlation_id(),
            actor: JournalActor::Runtime(current_runtime),
            runtime_instance_id: next.runtime_owner(),
            payload,
            recorded_at: at,
            occurred_at: None,
        })?,
    )
    .await
}

async fn claim_next(
    store: &SqliteStateStore,
    request: ClaimNextWorkRequest,
) -> Result<Option<ClaimedWork>, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "claim_next_work").await?;
    let runtime_state: Option<String> =
        sqlx::query_scalar("SELECT state FROM runtime_instances WHERE runtime_instance_id = ?")
            .bind(request.runtime_id.to_string())
            .fetch_optional(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    if runtime_state.as_deref() != Some("running") {
        return Err(conflict());
    }
    let row = sqlx::query(
        "SELECT * FROM work_items w WHERE w.conversation_id = ? AND w.state = 'queued' \
         AND NOT EXISTS (SELECT 1 FROM work_items active WHERE active.conversation_id = w.conversation_id \
         AND active.state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested')) \
         ORDER BY w.conversation_work_ordinal ASC, w.work_id ASC LIMIT 1",
    )
    .bind(request.conversation_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let Some(row) = row else {
        transaction.commit().await?;
        return Ok(None);
    };
    let decoded = decode_active_work_row(&row)?;
    if decoded.lifecycle.state() != WorkState::Queued
        || decoded.lifecycle.runtime_owner().is_some()
        || decoded.lifecycle.current_attempt() != CurrentWorkAttempt::None
        || decoded.started_at.is_some()
        || decoded.cancel_requested_at.is_some()
    {
        return Err(inconsistent());
    }
    let decision = decide_work_transition(
        &decoded.lifecycle,
        WorkTransitionGuard::for_snapshot(&decoded.lifecycle),
        WorkTransitionRequest::Start {
            runtime_owner: request.runtime_id,
        },
    )
    .map_err(|_| invariant())?;
    let next = decision.into_next();
    guarded_work_update(
        &mut transaction,
        &decoded.lifecycle,
        &next,
        WorkProjectionTimes {
            started_at: Some(request.claimed_at),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .map_err(|_| conflict())?;
    let cause = latest_stream_event(
        &mut transaction,
        JournalStreamId::Work(decoded.work.work_id()),
    )
    .await?;
    let position = append_work_transition(
        &mut transaction,
        &decoded,
        request.runtime_id,
        request.event_id,
        cause,
        &next,
        request.claimed_at,
    )
    .await?;
    transaction.commit().await?;
    let committed_version = next.projection_version();
    Ok(Some(ClaimedWork {
        work: decoded.work,
        lifecycle: next,
        commit: CommitReceipt {
            committed_version: Some(committed_version),
            events: Some(CommittedEventRange {
                first: position.offset,
                last: position.offset,
            }),
        },
    }))
}

async fn load_work_by_id(
    transaction: &mut WriteTransaction,
    work_id: WorkId,
) -> Result<ActiveWorkRow, SqliteAdapterError> {
    let row = sqlx::query("SELECT * FROM work_items WHERE work_id = ?")
        .bind(work_id.to_string())
        .fetch_optional(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
        .ok_or_else(conflict)?;
    decode_active_work_row(&row)
}

async fn terminalize_work(
    store: &SqliteStateStore,
    work_id: WorkId,
    runtime_id: RuntimeInstanceId,
    at: UtcTimestamp,
    event_id: JournalEventId,
    request: WorkTransitionRequest,
    transaction_name: &'static str,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, transaction_name).await?;
    let row = load_work_by_id(&mut transaction, work_id).await?;
    if row.lifecycle.runtime_owner() != Some(runtime_id) {
        return Err(conflict());
    }
    let decision = decide_work_transition(
        &row.lifecycle,
        WorkTransitionGuard::for_snapshot(&row.lifecycle),
        request,
    )
    .map_err(|_| conflict())?;
    let next = decision.into_next();
    let cause = latest_stream_event(&mut transaction, JournalStreamId::Work(work_id)).await?;
    guarded_work_update(
        &mut transaction,
        &row.lifecycle,
        &next,
        WorkProjectionTimes {
            started_at: row.started_at,
            cancel_requested_at: None,
            terminal_at: Some(at),
        },
    )
    .await
    .map_err(|_| conflict())?;
    let position = append_work_transition(
        &mut transaction,
        &row,
        runtime_id,
        event_id,
        cause,
        &next,
        at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(next.projection_version()),
        events: Some(CommittedEventRange {
            first: position.offset,
            last: position.offset,
        }),
    })
}

async fn finish_cancellation(
    store: &SqliteStateStore,
    request: FinishCancellationRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "finish_cancellation").await?;
    let row = load_work_by_id(&mut transaction, request.work_id).await?;
    if row.lifecycle.runtime_owner() != Some(request.runtime_id)
        || row.lifecycle.state() != WorkState::CancelRequested
        || row.lifecycle.current_attempt() != CurrentWorkAttempt::None
    {
        return Err(conflict());
    }
    let reason = row
        .lifecycle
        .cancellation_reason()
        .ok_or_else(inconsistent)?;
    drop(transaction);
    terminalize_work(
        store,
        request.work_id,
        request.runtime_id,
        request.confirmed_at,
        request.event_id,
        WorkTransitionRequest::Cancel {
            reason,
            cleanup_status: CleanupStatus::NotRequired,
        },
        "finish_cancellation",
    )
    .await
}

async fn interrupt_owned_work(
    store: &SqliteStateStore,
    request: InterruptOwnedWorkRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "inspect_abnormal_runner").await?;
    let row = load_work_by_id(&mut transaction, request.work_id).await?;
    if row.lifecycle.state().is_terminal() {
        transaction.commit().await?;
        return Ok(CommitReceipt {
            committed_version: Some(row.lifecycle.projection_version()),
            events: None,
        });
    }
    if row.lifecycle.runtime_owner() != Some(request.runtime_id)
        || row.lifecycle.current_attempt() != CurrentWorkAttempt::None
    {
        return Err(inconsistent());
    }
    drop(transaction);
    terminalize_work(
        store,
        request.work_id,
        request.runtime_id,
        request.interrupted_at,
        request.event_id,
        WorkTransitionRequest::Interrupt {
            reason: WorkInterruptionReason::RuntimeOwnershipLost,
        },
        "interrupt_abnormal_runner",
    )
    .await
}

async fn request_owned_cancellation(
    store: &SqliteStateStore,
    request: RequestOwnedCancellationRequest,
) -> Result<Vec<WorkId>, SqliteAdapterError> {
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "request_owned_work_cancellation").await?;
    let rows = sqlx::query(
        "SELECT * FROM work_items WHERE runtime_instance_id = ? AND state IN \
         ('running','waiting_on_model','waiting_on_tool') ORDER BY conversation_id, conversation_work_ordinal",
    )
    .bind(request.runtime_id.to_string())
    .fetch_all(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let mut work_ids = Vec::with_capacity(rows.len());
    for raw in rows {
        let row = decode_active_work_row(&raw)?;
        let decision = decide_work_transition(
            &row.lifecycle,
            WorkTransitionGuard::for_snapshot(&row.lifecycle),
            WorkTransitionRequest::RequestCancellation {
                reason: WorkCancellationReason::GracefulShutdown,
            },
        )
        .map_err(|_| conflict())?;
        let next = decision.into_next();
        let cause =
            latest_stream_event(&mut transaction, JournalStreamId::Work(row.work.work_id()))
                .await?;
        guarded_work_update(
            &mut transaction,
            &row.lifecycle,
            &next,
            WorkProjectionTimes {
                started_at: row.started_at,
                cancel_requested_at: Some(request.requested_at),
                terminal_at: None,
            },
        )
        .await
        .map_err(|_| conflict())?;
        append_work_transition(
            &mut transaction,
            &row,
            request.runtime_id,
            JournalEventId::generate(),
            cause,
            &next,
            request.requested_at,
        )
        .await?;
        work_ids.push(row.work.work_id());
    }
    transaction.commit().await?;
    Ok(work_ids)
}

async fn create_runtime(
    store: &SqliteStateStore,
    request: CreateRuntimeRequest,
) -> Result<CreateRuntimeReceipt, SqliteAdapterError> {
    let evidence = &request.evidence;
    let linux_boot_id = evidence.linux_boot_id().ok_or_else(invariant)?;
    let process_id = evidence.diagnostic_pid().ok_or_else(invariant)?;
    if evidence.schema_version().get() != 4 {
        return Err(invariant());
    }
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "create_runtime_and_started_event").await?;
    sqlx::query(
        "INSERT INTO runtime_instances (runtime_instance_id, craxii_id, workstation_id, \
         workstation_generation, linux_boot_id, process_id, binary_version, git_revision, \
         schema_version, state, started_at, last_heartbeat_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'running', ?, ?)",
    )
    .bind(evidence.runtime_instance_id().to_string())
    .bind(evidence.craxii_id().to_string())
    .bind(evidence.workstation_id().to_string())
    .bind(evidence.workstation_generation().get())
    .bind(linux_boot_id.as_str())
    .bind(process_id.get())
    .bind(evidence.package_version().as_str())
    .bind(evidence.git_revision().as_str())
    .bind(evidence.schema_version().get())
    .bind(evidence.started_at().to_string())
    .bind(evidence.started_at().to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let position = append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.event_id,
            craxii_id: evidence.craxii_id(),
            stream_id: JournalStreamId::Runtime(evidence.runtime_instance_id()),
            conversation_id: None,
            work_id: None,
            causation_event_id: None,
            correlation_id: request.correlation_id,
            actor: JournalActor::Runtime(evidence.runtime_instance_id()),
            runtime_instance_id: Some(evidence.runtime_instance_id()),
            payload: JournalEventPayload::RuntimeStarted(RuntimeStartedV1 {
                runtime_instance_id: evidence.runtime_instance_id(),
                craxii_id: evidence.craxii_id(),
                workstation_id: evidence.workstation_id(),
                workstation_generation: evidence.workstation_generation(),
                linux_boot_id: linux_boot_id.clone(),
                process_id,
                binary_version: evidence.package_version().clone(),
                git_revision: evidence.git_revision().clone(),
                schema_version: evidence.schema_version(),
                started_at: evidence.started_at(),
            }),
            recorded_at: evidence.started_at(),
            occurred_at: None,
        })?,
    )
    .await?;
    transaction.commit().await?;
    Ok(CreateRuntimeReceipt {
        runtime_instance_id: evidence.runtime_instance_id(),
        started_event_id: request.event_id,
        commit: CommitReceipt {
            committed_version: None,
            events: Some(CommittedEventRange {
                first: position.offset,
                last: position.offset,
            }),
        },
    })
}

async fn heartbeat_runtime(
    store: &SqliteStateStore,
    request: HeartbeatRuntimeRequest,
) -> Result<HeartbeatRuntimeReceipt, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(&store.runtime, "heartbeat_runtime").await?;
    let row = sqlx::query(
        "SELECT state, last_heartbeat_at FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(request.runtime_instance_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    if row.try_get::<String, _>("state")? != "running" {
        return Err(conflict());
    }
    let persisted: UtcTimestamp = row
        .try_get::<String, _>("last_heartbeat_at")?
        .parse()
        .map_err(|_| inconsistent())?;
    let advanced = request.observed_at > persisted;
    if advanced {
        let changed = sqlx::query(
            "UPDATE runtime_instances SET last_heartbeat_at = ? WHERE runtime_instance_id = ? \
             AND state = 'running' AND last_heartbeat_at = ?",
        )
        .bind(request.observed_at.to_string())
        .bind(request.runtime_instance_id.to_string())
        .bind(persisted.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if changed.rows_affected() != 1 {
            return Err(conflict());
        }
    }
    transaction.commit().await?;
    Ok(HeartbeatRuntimeReceipt {
        persisted_at: if advanced {
            request.observed_at
        } else {
            persisted
        },
        advanced,
    })
}

async fn begin_runtime_stopping(
    store: &SqliteStateStore,
    request: BeginRuntimeStoppingRequest,
) -> Result<BeginRuntimeStoppingReceipt, SqliteAdapterError> {
    let event = &request.event;
    if event.shutdown_reason != RuntimeShutdownReason::GracefulShutdown
        || event.grace_deadline < event.shutdown_requested_at
    {
        return Err(invariant());
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "begin_runtime_stopping").await?;
    let runtime = sqlx::query(
        "SELECT state, started_at FROM runtime_instances WHERE runtime_instance_id = ?",
    )
    .bind(event.runtime_instance_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let Some(runtime) = runtime else {
        return Err(conflict());
    };
    let state: String = runtime.try_get("state")?;
    let started_at: UtcTimestamp = runtime
        .try_get::<String, _>("started_at")?
        .parse()
        .map_err(|_| inconsistent())?;
    if state == "stopping" {
        transaction.commit().await?;
        return Ok(BeginRuntimeStoppingReceipt {
            began: false,
            commit: CommitReceipt {
                committed_version: None,
                events: None,
            },
        });
    }
    if state != "running" || event.shutdown_requested_at < started_at {
        return Err(conflict());
    }
    let cause = latest_stream_event(
        &mut transaction,
        JournalStreamId::Runtime(event.runtime_instance_id),
    )
    .await?;
    let changed = sqlx::query(
        "UPDATE runtime_instances SET state = 'stopping', stop_reason = 'graceful_shutdown' \
         WHERE runtime_instance_id = ? AND state = 'running'",
    )
    .bind(event.runtime_instance_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if changed.rows_affected() != 1 {
        return Err(conflict());
    }
    let craxii_id: String =
        sqlx::query_scalar("SELECT craxii_id FROM runtime_instances WHERE runtime_instance_id = ?")
            .bind(event.runtime_instance_id.to_string())
            .fetch_one(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    let position = append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.event_id,
            craxii_id: CraxiiId::parse_canonical(&craxii_id).map_err(|_| inconsistent())?,
            stream_id: JournalStreamId::Runtime(event.runtime_instance_id),
            conversation_id: None,
            work_id: None,
            causation_event_id: Some(cause),
            correlation_id: request.correlation_id,
            actor: JournalActor::Runtime(event.runtime_instance_id),
            runtime_instance_id: Some(event.runtime_instance_id),
            payload: JournalEventPayload::RuntimeStopping(event.clone()),
            recorded_at: event.shutdown_requested_at,
            occurred_at: None,
        })?,
    )
    .await?;
    transaction.commit().await?;
    Ok(BeginRuntimeStoppingReceipt {
        began: true,
        commit: CommitReceipt {
            committed_version: None,
            events: Some(CommittedEventRange {
                first: position.offset,
                last: position.offset,
            }),
        },
    })
}

async fn finish_runtime(
    store: &SqliteStateStore,
    request: FinishRuntimeRequest,
    graceful: bool,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let mut transaction = WriteTransaction::begin(
        &store.runtime,
        if graceful {
            "finish_runtime_graceful"
        } else {
            "mark_runtime_startup_failure"
        },
    )
    .await?;
    let expected = if graceful { "stopping" } else { "running" };
    let reason = if graceful {
        "graceful_shutdown"
    } else {
        "startup_failure"
    };
    let result = if graceful {
        sqlx::query(
            "UPDATE runtime_instances SET state = 'stopped', stopped_at = ?, stop_reason = ? \
             WHERE runtime_instance_id = ? AND state = ? AND started_at <= ?",
        )
        .bind(request.stopped_at.to_string())
        .bind(reason)
        .bind(request.runtime_instance_id.to_string())
        .bind(expected)
        .bind(request.stopped_at.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
    } else {
        sqlx::query(
            "UPDATE runtime_instances SET state = 'stopped', stopped_at = ?, stop_reason = ? \
             WHERE runtime_instance_id = ? AND state IN ('running','stopping') AND started_at <= ?",
        )
        .bind(request.stopped_at.to_string())
        .bind(reason)
        .bind(request.runtime_instance_id.to_string())
        .bind(request.stopped_at.to_string())
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?
    };
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: None,
        events: None,
    })
}

async fn enumerate_stale(
    store: &SqliteStateStore,
    request: EnumerateStaleRuntimesRequest,
) -> Result<Vec<RuntimeInstanceId>, SqliteAdapterError> {
    let mut connection = store.runtime.acquire().await?;
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT runtime_instance_id FROM runtime_instances INDEXED BY ix_runtime_instances_craxii_state \
         WHERE state IN ('running','stopping') AND runtime_instance_id <> ? ORDER BY started_at, runtime_instance_id",
    )
    .bind(request.current_runtime_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    rows.into_iter()
        .map(|value| RuntimeInstanceId::parse_canonical(&value).map_err(|_| inconsistent()))
        .collect()
}

async fn append_recovery_summary(
    store: &SqliteStateStore,
    request: AppendRecoverySummaryRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let summary = &request.summary;
    if !summary.counts_are_persistable() {
        return Err(invariant());
    }
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "append_recovery_summary").await?;
    let prior = sqlx::query(
        "SELECT * FROM journal_events WHERE stream_id = ? \
         AND event_type = 'runtime.recovery_performed'",
    )
    .bind(JournalStreamId::Runtime(summary.runtime_instance_id).to_string())
    .fetch_all(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if let [row] = prior.as_slice() {
        let event = decode_event_row(row)?;
        if event.payload != JournalEventPayload::RuntimeRecoveryPerformed(summary.clone())
            || event.causation_event_id != Some(request.started_event_id)
            || event.correlation_id != request.correlation_id
        {
            return Err(inconsistent());
        }
        transaction.commit().await?;
        return Ok(CommitReceipt {
            committed_version: None,
            events: None,
        });
    }
    if !prior.is_empty() {
        return Err(inconsistent());
    }
    let row = sqlx::query(
        "SELECT craxii_id, binary_version, schema_version, state FROM runtime_instances \
         WHERE runtime_instance_id = ?",
    )
    .bind(summary.runtime_instance_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    if row.try_get::<String, _>("state")? != "running"
        || row.try_get::<String, _>("binary_version")? != summary.binary_version.as_str()
        || row.try_get::<i64, _>("schema_version")? != summary.schema_version.get()
    {
        return Err(inconsistent());
    }
    let position = append_event(
        &mut transaction,
        prepare_event(JournalAppendIntent {
            event_id: request.event_id,
            craxii_id: CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
                .map_err(|_| inconsistent())?,
            stream_id: JournalStreamId::Runtime(summary.runtime_instance_id),
            conversation_id: None,
            work_id: None,
            causation_event_id: Some(request.started_event_id),
            correlation_id: request.correlation_id,
            actor: JournalActor::Runtime(summary.runtime_instance_id),
            runtime_instance_id: Some(summary.runtime_instance_id),
            payload: JournalEventPayload::RuntimeRecoveryPerformed(summary.clone()),
            recorded_at: summary.recovered_at,
            occurred_at: None,
        })?,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: None,
        events: Some(CommittedEventRange {
            first: position.offset,
            last: position.offset,
        }),
    })
}

#[derive(Default)]
struct RecoveryCounters {
    interrupted_work: u64,
    model_unknown: u64,
    model_terminal_preserved: u64,
    tool_predispatch: u64,
    tool_unknown: u64,
    tool_terminal_preserved: u64,
    drafts_abandoned: u64,
    cleanup_checks: u64,
    cleanup_unconfirmed: u64,
    first_offset: Option<crate::domain::JournalOffset>,
    last_offset: Option<crate::domain::JournalOffset>,
}

fn observe_position(
    counters: &mut RecoveryCounters,
    position: super::journal::CommittedJournalPosition,
) {
    counters.first_offset.get_or_insert(position.offset);
    counters.last_offset = Some(position.offset);
}

async fn append_model_recovery_event(
    transaction: &mut WriteTransaction,
    row: &ActiveWorkRow,
    current_runtime_id: RuntimeInstanceId,
    model_invocation_id: ModelInvocationId,
    state: ModelInvocationState,
    at: UtcTimestamp,
    cause: JournalEventId,
) -> Result<(JournalEventId, super::journal::CommittedJournalPosition), SqliteAdapterError> {
    let event_id = JournalEventId::generate();
    let logical_invocation_id = sqlx::query_scalar::<_, String>(
        "SELECT logical_invocation_id FROM model_invocations WHERE model_invocation_id = ?",
    )
    .bind(model_invocation_id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .parse()
    .map_err(|_| inconsistent())?;
    let position = append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id,
            craxii_id: row.work.craxii_id(),
            stream_id: JournalStreamId::Work(row.work.work_id()),
            conversation_id: Some(row.work.conversation_id()),
            work_id: Some(row.work.work_id()),
            causation_event_id: Some(cause),
            correlation_id: row.work.correlation_id(),
            actor: JournalActor::Runtime(current_runtime_id),
            runtime_instance_id: Some(current_runtime_id),
            payload: JournalEventPayload::ModelInvocationInterrupted(ModelInvocationEventV1 {
                work_id: row.work.work_id(),
                model_invocation_id,
                logical_invocation_id,
                state,
                observed_at: at,
            }),
            recorded_at: at,
            occurred_at: None,
        })?,
    )
    .await?;
    Ok((event_id, position))
}

async fn append_tool_recovery_event(
    transaction: &mut WriteTransaction,
    row: &ActiveWorkRow,
    current_runtime_id: RuntimeInstanceId,
    tool_execution_id: ToolExecutionId,
    state: ToolExecutionState,
    at: UtcTimestamp,
    cause: JournalEventId,
) -> Result<(JournalEventId, super::journal::CommittedJournalPosition), SqliteAdapterError> {
    let event_id = JournalEventId::generate();
    let fact = ToolExecutionEventV1 {
        work_id: row.work.work_id(),
        tool_execution_id,
        state,
        outcome_classification: None,
        observed_at: at,
    };
    let payload = match state {
        ToolExecutionState::InterruptedBeforeDispatch => {
            JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(fact)
        }
        ToolExecutionState::OutcomeUnknown => {
            JournalEventPayload::ToolExecutionOutcomeUnknown(fact)
        }
        _ => return Err(invariant()),
    };
    let position = append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id,
            craxii_id: row.work.craxii_id(),
            stream_id: JournalStreamId::Work(row.work.work_id()),
            conversation_id: Some(row.work.conversation_id()),
            work_id: Some(row.work.work_id()),
            causation_event_id: Some(cause),
            correlation_id: row.work.correlation_id(),
            actor: JournalActor::Runtime(current_runtime_id),
            runtime_instance_id: Some(current_runtime_id),
            payload,
            recorded_at: at,
            occurred_at: None,
        })?,
    )
    .await?;
    Ok((event_id, position))
}

async fn recover_work_unit(
    store: &SqliteStateStore,
    request: RecoverStaleRuntimeRequest,
    work_id: WorkId,
) -> Result<RecoveryCounters, SqliteAdapterError> {
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "recover_stale_work_unit").await?;
    let row = load_work_by_id(&mut transaction, work_id).await?;
    if row.lifecycle.state().is_terminal() {
        transaction.commit().await?;
        return Ok(RecoveryCounters::default());
    }
    if row.lifecycle.runtime_owner() != Some(request.stale_runtime_id) {
        return Err(inconsistent());
    }
    let mut counters = RecoveryCounters::default();
    let mut cause = latest_stream_event(&mut transaction, JournalStreamId::Work(work_id)).await?;
    let mut cancellation_can_finish = row.lifecycle.state() == WorkState::CancelRequested;
    let interruption_reason = match row.lifecycle.current_attempt() {
        CurrentWorkAttempt::None => WorkInterruptionReason::RuntimeOwnershipLost,
        CurrentWorkAttempt::Model(model_id) => {
            let attempt = sqlx::query(
                "SELECT state, runtime_instance_id, draft_exposed FROM model_invocations WHERE model_invocation_id = ? \
                 AND work_id = ?",
            )
            .bind(model_id.to_string())
            .bind(work_id.to_string())
            .fetch_optional(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?
            .ok_or_else(inconsistent)?;
            if attempt.try_get::<String, _>("runtime_instance_id")?
                != request.stale_runtime_id.to_string()
            {
                return Err(inconsistent());
            }
            let draft_exposed = attempt.try_get::<i64, _>("draft_exposed")? == 1;
            match attempt.try_get::<String, _>("state")?.as_str() {
                "requesting" | "streaming" => {
                    counters.drafts_abandoned += u64::from(draft_exposed);
                    let error = encode_attempt_error(
                        &NormalizedError::provider(Certainty::OutcomeUnknown, None),
                        true,
                    )?;
                    let changed = sqlx::query(
                        "UPDATE model_invocations SET state = 'provider_outcome_unknown', completed_at = ?, \
                         normalized_error_json = ? WHERE model_invocation_id = ? AND state IN ('requesting','streaming')",
                    )
                    .bind(request.recovered_at.to_string())
                    .bind(error)
                    .bind(model_id.to_string())
                    .execute(transaction.connection())
                    .await
                    .map_err(SqliteAdapterError::from_sqlx)?;
                    if changed.rows_affected() != 1 {
                        return Err(conflict());
                    }
                    let (event_id, position) = append_model_recovery_event(
                        &mut transaction,
                        &row,
                        request.current_runtime_id,
                        model_id,
                        ModelInvocationState::ProviderOutcomeUnknown,
                        request.recovered_at,
                        cause,
                    )
                    .await?;
                    observe_position(&mut counters, position);
                    cause = event_id;
                    counters.model_unknown += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ProviderOutcomeUnknown
                }
                "provider_outcome_unknown" => {
                    counters.drafts_abandoned += u64::from(draft_exposed);
                    counters.model_terminal_preserved += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ProviderOutcomeUnknown
                }
                "completed" | "failed" | "cancelled_locally" => {
                    counters.model_terminal_preserved += 1;
                    WorkInterruptionReason::RuntimeOwnershipLost
                }
                _ => return Err(inconsistent()),
            }
        }
        CurrentWorkAttempt::Tool(tool_id) => {
            let attempt = sqlx::query(
                "SELECT state, runtime_instance_id, cleanup_confirmed FROM tool_executions \
                 WHERE tool_execution_id = ? AND work_id = ?",
            )
            .bind(tool_id.to_string())
            .bind(work_id.to_string())
            .fetch_optional(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?
            .ok_or_else(inconsistent)?;
            if attempt.try_get::<String, _>("runtime_instance_id")?
                != request.stale_runtime_id.to_string()
            {
                return Err(inconsistent());
            }
            match attempt.try_get::<String, _>("state")?.as_str() {
                "requested" => {
                    let error = encode_attempt_error(
                        &NormalizedError::cancellation(Certainty::Definite),
                        false,
                    )?;
                    let changed = sqlx::query(
                        "UPDATE tool_executions SET state = 'interrupted_before_dispatch', completed_at = ?, \
                         normalized_error_json = ? WHERE tool_execution_id = ? AND state = 'requested'",
                    )
                    .bind(request.recovered_at.to_string())
                    .bind(error)
                    .bind(tool_id.to_string())
                    .execute(transaction.connection())
                    .await
                    .map_err(SqliteAdapterError::from_sqlx)?;
                    if changed.rows_affected() != 1 {
                        return Err(conflict());
                    }
                    let (event_id, position) = append_tool_recovery_event(
                        &mut transaction,
                        &row,
                        request.current_runtime_id,
                        tool_id,
                        ToolExecutionState::InterruptedBeforeDispatch,
                        request.recovered_at,
                        cause,
                    )
                    .await?;
                    observe_position(&mut counters, position);
                    cause = event_id;
                    counters.tool_predispatch += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ToolInterruptedBeforeDispatch
                }
                "dispatching" => {
                    counters.cleanup_checks += 1;
                    let error = encode_attempt_error(
                        &NormalizedError::workstation(Certainty::OutcomeUnknown, None),
                        true,
                    )?;
                    let changed = sqlx::query(
                        "UPDATE tool_executions SET state = 'outcome_unknown', completed_at = ?, \
                         cleanup_confirmed = 0, normalized_error_json = ? \
                         WHERE tool_execution_id = ? AND state = 'dispatching'",
                    )
                    .bind(request.recovered_at.to_string())
                    .bind(error)
                    .bind(tool_id.to_string())
                    .execute(transaction.connection())
                    .await
                    .map_err(SqliteAdapterError::from_sqlx)?;
                    if changed.rows_affected() != 1 {
                        return Err(conflict());
                    }
                    let (event_id, position) = append_tool_recovery_event(
                        &mut transaction,
                        &row,
                        request.current_runtime_id,
                        tool_id,
                        ToolExecutionState::OutcomeUnknown,
                        request.recovered_at,
                        cause,
                    )
                    .await?;
                    observe_position(&mut counters, position);
                    cause = event_id;
                    counters.tool_unknown += 1;
                    counters.cleanup_unconfirmed += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ToolOutcomeUnknown
                }
                "interrupted_before_dispatch" => {
                    counters.tool_terminal_preserved += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ToolInterruptedBeforeDispatch
                }
                "outcome_unknown" => {
                    counters.cleanup_checks += 1;
                    counters.tool_terminal_preserved += 1;
                    counters.cleanup_unconfirmed += 1;
                    cancellation_can_finish = false;
                    WorkInterruptionReason::ToolOutcomeUnknown
                }
                "completed" => {
                    counters.cleanup_checks += 1;
                    counters.tool_terminal_preserved += 1;
                    if attempt.try_get::<Option<i64>, _>("cleanup_confirmed")? == Some(0) {
                        counters.cleanup_unconfirmed += 1;
                        cancellation_can_finish = false;
                        WorkInterruptionReason::CleanupUnconfirmed
                    } else {
                        WorkInterruptionReason::RuntimeOwnershipLost
                    }
                }
                _ => return Err(inconsistent()),
            }
        }
    };

    let request_transition = if cancellation_can_finish {
        WorkTransitionRequest::Cancel {
            reason: row
                .lifecycle
                .cancellation_reason()
                .ok_or_else(inconsistent)?,
            cleanup_status: CleanupStatus::NotRequired,
        }
    } else {
        WorkTransitionRequest::Interrupt {
            reason: interruption_reason,
        }
    };
    let decision = decide_work_transition(
        &row.lifecycle,
        WorkTransitionGuard::for_snapshot(&row.lifecycle),
        request_transition,
    )
    .map_err(|_| conflict())?;
    let next = decision.into_next();
    guarded_work_update(
        &mut transaction,
        &row.lifecycle,
        &next,
        WorkProjectionTimes {
            started_at: row.started_at,
            cancel_requested_at: None,
            terminal_at: Some(request.recovered_at),
        },
    )
    .await
    .map_err(|_| conflict())?;
    let position = append_work_transition(
        &mut transaction,
        &row,
        request.current_runtime_id,
        JournalEventId::generate(),
        cause,
        &next,
        request.recovered_at,
    )
    .await?;
    observe_position(&mut counters, position);
    if next.state() == WorkState::Interrupted {
        counters.interrupted_work += 1;
    }
    transaction.commit().await?;
    Ok(counters)
}

async fn recover_stale_runtime(
    store: &SqliteStateStore,
    request: RecoverStaleRuntimeRequest,
) -> Result<RecoveryReceipt, SqliteAdapterError> {
    if request.stale_runtime_id == request.current_runtime_id {
        return Err(invariant());
    }
    let mut connection = store.runtime.acquire().await?;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM runtime_instances WHERE runtime_instance_id = ?")
            .bind(request.stale_runtime_id.to_string())
            .fetch_optional(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    if state.as_deref() == Some("stopped") {
        return Ok(RecoveryReceipt {
            stale_runtime_closed: false,
            interrupted_work: 0,
            model_attempts_provider_outcome_unknown: 0,
            model_attempts_terminal_preserved: 0,
            tool_attempts_interrupted_before_dispatch: 0,
            tool_attempts_outcome_unknown: 0,
            tool_attempts_terminal_preserved: 0,
            drafts_abandoned: 0,
            cleanup_checks_performed: 0,
            cleanup_unconfirmed: 0,
            commit: CommitReceipt {
                committed_version: None,
                events: None,
            },
        });
    }
    if !matches!(state.as_deref(), Some("running" | "stopping")) {
        return Err(inconsistent());
    }
    let work_ids: Vec<String> = sqlx::query_scalar(
        "SELECT work_id FROM work_items INDEXED BY ix_work_items_nonterminal_by_runtime \
         WHERE runtime_instance_id = ? AND state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
         ORDER BY conversation_id, conversation_work_ordinal",
    )
    .bind(request.stale_runtime_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    verify_nonterminal_attempt_ownership(&mut connection, request.stale_runtime_id, &work_ids)
        .await?;
    drop(connection);
    let mut total = RecoveryCounters::default();
    for value in work_ids {
        let work_id = WorkId::parse_canonical(&value).map_err(|_| inconsistent())?;
        let unit = recover_work_unit(store, request, work_id).await?;
        total.interrupted_work += unit.interrupted_work;
        total.model_unknown += unit.model_unknown;
        total.model_terminal_preserved += unit.model_terminal_preserved;
        total.tool_predispatch += unit.tool_predispatch;
        total.tool_unknown += unit.tool_unknown;
        total.tool_terminal_preserved += unit.tool_terminal_preserved;
        total.drafts_abandoned += unit.drafts_abandoned;
        total.cleanup_checks += unit.cleanup_checks;
        total.cleanup_unconfirmed += unit.cleanup_unconfirmed;
        if total.first_offset.is_none() {
            total.first_offset = unit.first_offset;
        }
        if unit.last_offset.is_some() {
            total.last_offset = unit.last_offset;
        }
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "close_stale_runtime").await?;
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_items WHERE runtime_instance_id = ? AND state IN \
         ('running','waiting_on_model','waiting_on_tool','cancel_requested')",
    )
    .bind(request.stale_runtime_id.to_string())
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if remaining != 0 {
        return Err(inconsistent());
    }
    let closed = sqlx::query(
        "UPDATE runtime_instances SET state = 'stopped', stopped_at = ?, stop_reason = 'startup_failure' \
         WHERE runtime_instance_id = ? AND state IN ('running','stopping') AND started_at <= ?",
    )
    .bind(request.recovered_at.to_string())
    .bind(request.stale_runtime_id.to_string())
    .bind(request.recovered_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if closed.rows_affected() != 1 {
        return Err(conflict());
    }
    transaction.commit().await?;
    Ok(RecoveryReceipt {
        stale_runtime_closed: true,
        interrupted_work: total.interrupted_work,
        model_attempts_provider_outcome_unknown: total.model_unknown,
        model_attempts_terminal_preserved: total.model_terminal_preserved,
        tool_attempts_interrupted_before_dispatch: total.tool_predispatch,
        tool_attempts_outcome_unknown: total.tool_unknown,
        tool_attempts_terminal_preserved: total.tool_terminal_preserved,
        drafts_abandoned: total.drafts_abandoned,
        cleanup_checks_performed: total.cleanup_checks,
        cleanup_unconfirmed: total.cleanup_unconfirmed,
        commit: CommitReceipt {
            committed_version: None,
            events: total.first_offset.map(|first| CommittedEventRange {
                first,
                last: total.last_offset.unwrap_or(first),
            }),
        },
    })
}

async fn verify_nonterminal_attempt_ownership(
    connection: &mut sqlx::SqliteConnection,
    runtime_id: RuntimeInstanceId,
    active_work_ids: &[String],
) -> Result<(), SqliteAdapterError> {
    let model_work_ids: Vec<String> = sqlx::query_scalar(
        "SELECT work_id FROM model_invocations INDEXED BY ix_model_invocations_runtime_nonterminal \
         WHERE runtime_instance_id = ? AND state IN ('requesting','streaming') ORDER BY work_id",
    )
    .bind(runtime_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let tool_work_ids: Vec<String> = sqlx::query_scalar(
        "SELECT work_id FROM tool_executions INDEXED BY ix_tool_executions_runtime_nonterminal \
         WHERE runtime_instance_id = ? AND state IN ('requested','dispatching') ORDER BY work_id",
    )
    .bind(runtime_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if model_work_ids
        .iter()
        .chain(tool_work_ids.iter())
        .any(|work_id| !active_work_ids.contains(work_id))
    {
        return Err(inconsistent());
    }
    Ok(())
}

async fn classify_shutdown_work(
    store: &SqliteStateStore,
    request: ClassifyShutdownWorkRequest,
) -> Result<RecoveryReceipt, SqliteAdapterError> {
    let mut connection = store.runtime.acquire().await?;
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM runtime_instances WHERE runtime_instance_id = ?")
            .bind(request.runtime_id.to_string())
            .fetch_optional(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    if !matches!(state.as_deref(), Some("running" | "stopping")) {
        return Err(conflict());
    }
    let work_ids: Vec<String> = sqlx::query_scalar(
        "SELECT work_id FROM work_items INDEXED BY ix_work_items_nonterminal_by_runtime \
         WHERE runtime_instance_id = ? AND state IN \
         ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
         ORDER BY conversation_id, conversation_work_ordinal",
    )
    .bind(request.runtime_id.to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    verify_nonterminal_attempt_ownership(&mut connection, request.runtime_id, &work_ids).await?;
    drop(connection);
    let mut total = RecoveryCounters::default();
    for value in work_ids {
        let work_id = WorkId::parse_canonical(&value).map_err(|_| inconsistent())?;
        let unit = recover_work_unit(
            store,
            RecoverStaleRuntimeRequest {
                stale_runtime_id: request.runtime_id,
                current_runtime_id: request.runtime_id,
                recovered_at: request.classified_at,
            },
            work_id,
        )
        .await?;
        total.interrupted_work += unit.interrupted_work;
        total.model_unknown += unit.model_unknown;
        total.model_terminal_preserved += unit.model_terminal_preserved;
        total.tool_predispatch += unit.tool_predispatch;
        total.tool_unknown += unit.tool_unknown;
        total.tool_terminal_preserved += unit.tool_terminal_preserved;
        total.drafts_abandoned += unit.drafts_abandoned;
        total.cleanup_checks += unit.cleanup_checks;
        total.cleanup_unconfirmed += unit.cleanup_unconfirmed;
        if total.first_offset.is_none() {
            total.first_offset = unit.first_offset;
        }
        if unit.last_offset.is_some() {
            total.last_offset = unit.last_offset;
        }
    }
    Ok(RecoveryReceipt {
        stale_runtime_closed: false,
        interrupted_work: total.interrupted_work,
        model_attempts_provider_outcome_unknown: total.model_unknown,
        model_attempts_terminal_preserved: total.model_terminal_preserved,
        tool_attempts_interrupted_before_dispatch: total.tool_predispatch,
        tool_attempts_outcome_unknown: total.tool_unknown,
        tool_attempts_terminal_preserved: total.tool_terminal_preserved,
        drafts_abandoned: total.drafts_abandoned,
        cleanup_checks_performed: total.cleanup_checks,
        cleanup_unconfirmed: total.cleanup_unconfirmed,
        commit: CommitReceipt {
            committed_version: None,
            events: total.first_offset.map(|first| CommittedEventRange {
                first,
                last: total.last_offset.unwrap_or(first),
            }),
        },
    })
}

#[derive(Default)]
struct ExactRecoveryCounters {
    retained_queued_work: u64,
    interrupted_work: u64,
    model_unknown: u64,
    model_terminal_preserved: u64,
    tool_predispatch: u64,
    tool_unknown: u64,
    tool_terminal_preserved: u64,
    drafts_abandoned: u64,
    cleanup_checks: u64,
    cleanup_unconfirmed: u64,
}

fn prior_stream_event(events: &[JournalEvent], index: usize) -> Option<&JournalEvent> {
    let stream = events.get(index)?.stream_id;
    events[..index]
        .iter()
        .rev()
        .find(|candidate| candidate.stream_id == stream)
}

fn recovery_terminal_transition(event: &JournalEvent) -> Option<&WorkTransitionV1> {
    match &event.payload {
        JournalEventPayload::WorkInterrupted(transition)
        | JournalEventPayload::WorkCancelled(transition) => Some(transition),
        _ => None,
    }
}

async fn verify_exact_recovery_summary(
    connection: &mut sqlx::SqliteConnection,
    events: &[JournalEvent],
    runtime_id: RuntimeInstanceId,
    summary: &crate::domain::RuntimeRecoveryPerformedV1,
) -> Result<(), SqliteAdapterError> {
    let started_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                JournalEventPayload::RuntimeStarted(started)
                    if started.runtime_instance_id == runtime_id
            )
        })
        .ok_or_else(inconsistent)?;
    let summary_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                JournalEventPayload::RuntimeRecoveryPerformed(candidate)
                    if candidate.runtime_instance_id == runtime_id
            )
        })
        .ok_or_else(inconsistent)?;
    if summary_index <= started_index {
        return Err(inconsistent());
    }

    let recovery_projection = crate::application::projector::project(&events[..summary_index])
        .map_err(|_| inconsistent())?;
    let mut exact = ExactRecoveryCounters {
        retained_queued_work: recovery_projection
            .works
            .values()
            .filter(|work| work.state == WorkState::Queued)
            .count()
            .try_into()
            .map_err(|_| inconsistent())?,
        ..ExactRecoveryCounters::default()
    };
    let mut terminals = HashMap::<WorkId, usize>::new();
    let mut model_classifications = HashMap::<ModelInvocationId, usize>::new();
    let mut tool_classifications = HashMap::<ToolExecutionId, usize>::new();

    for index in (started_index + 1)..summary_index {
        let event = &events[index];
        match &event.payload {
            JournalEventPayload::WorkInterrupted(transition)
            | JournalEventPayload::WorkCancelled(transition) => {
                let Some(stale_runtime_id) = transition.expected_runtime_owner else {
                    return Err(inconsistent());
                };
                let stale_state: Option<(String, Option<String>)> = sqlx::query_as(
                    "SELECT state, stop_reason FROM runtime_instances \
                     WHERE runtime_instance_id = ?",
                )
                .bind(stale_runtime_id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
                if stale_runtime_id == runtime_id
                    || event.actor != JournalActor::Runtime(runtime_id)
                    || event.runtime_instance_id.is_some()
                    || transition.runtime_owner.is_some()
                    || transition.current_attempt != JournalCurrentAttempt::None
                    || !matches!(
                        stale_state,
                        Some((ref state, Some(ref reason)))
                            if state == "stopped" && reason == "startup_failure"
                    )
                    || event.causation_event_id
                        != prior_stream_event(events, index).map(|prior| prior.event_id)
                    || terminals.insert(transition.work_id, index).is_some()
                {
                    return Err(inconsistent());
                }
                if matches!(event.payload, JournalEventPayload::WorkInterrupted(_)) {
                    exact.interrupted_work += 1;
                }
            }
            JournalEventPayload::ModelInvocationInterrupted(fact) => {
                if fact.state != ModelInvocationState::ProviderOutcomeUnknown
                    || event.actor != JournalActor::Runtime(runtime_id)
                    || event.runtime_instance_id != Some(runtime_id)
                    || event.causation_event_id
                        != prior_stream_event(events, index).map(|prior| prior.event_id)
                    || model_classifications
                        .insert(fact.model_invocation_id, index)
                        .is_some()
                {
                    return Err(inconsistent());
                }
            }
            JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(fact) => {
                if fact.state != ToolExecutionState::InterruptedBeforeDispatch
                    || event.actor != JournalActor::Runtime(runtime_id)
                    || event.runtime_instance_id != Some(runtime_id)
                    || event.causation_event_id
                        != prior_stream_event(events, index).map(|prior| prior.event_id)
                    || tool_classifications
                        .insert(fact.tool_execution_id, index)
                        .is_some()
                {
                    return Err(inconsistent());
                }
            }
            JournalEventPayload::ToolExecutionOutcomeUnknown(fact) => {
                if fact.state != ToolExecutionState::OutcomeUnknown
                    || event.actor != JournalActor::Runtime(runtime_id)
                    || event.runtime_instance_id != Some(runtime_id)
                    || event.causation_event_id
                        != prior_stream_event(events, index).map(|prior| prior.event_id)
                    || tool_classifications
                        .insert(fact.tool_execution_id, index)
                        .is_some()
                {
                    return Err(inconsistent());
                }
            }
            _ => return Err(inconsistent()),
        }
    }

    let mut attributed_models = HashSet::new();
    let mut attributed_tools = HashSet::new();
    for index in terminals.values().copied() {
        let event = &events[index];
        let transition = recovery_terminal_transition(event).ok_or_else(inconsistent)?;
        let stale_runtime_id = transition.expected_runtime_owner.ok_or_else(inconsistent)?;
        match transition.expected_current_attempt {
            JournalCurrentAttempt::None => {}
            JournalCurrentAttempt::Model(model_id) => {
                let row = sqlx::query(
                    "SELECT work_id, runtime_instance_id, state, draft_exposed \
                     FROM model_invocations WHERE model_invocation_id = ?",
                )
                .bind(model_id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?
                .ok_or_else(inconsistent)?;
                let state: String = row.try_get("state")?;
                if row.try_get::<String, _>("work_id")? != transition.work_id.to_string()
                    || row.try_get::<String, _>("runtime_instance_id")?
                        != stale_runtime_id.to_string()
                    || !matches!(
                        state.as_str(),
                        "provider_outcome_unknown" | "completed" | "failed" | "cancelled_locally"
                    )
                {
                    return Err(inconsistent());
                }
                if let Some(classification_index) = model_classifications.get(&model_id).copied() {
                    if event.causation_event_id != Some(events[classification_index].event_id)
                        || classification_index >= index
                        || state != "provider_outcome_unknown"
                        || !attributed_models.insert(model_id)
                    {
                        return Err(inconsistent());
                    }
                    exact.model_unknown += 1;
                } else {
                    exact.model_terminal_preserved += 1;
                }
                if state == "provider_outcome_unknown"
                    && row.try_get::<i64, _>("draft_exposed")? == 1
                {
                    exact.drafts_abandoned += 1;
                }
            }
            JournalCurrentAttempt::Tool(tool_id) => {
                let row = sqlx::query(
                    "SELECT work_id, runtime_instance_id, state, cleanup_confirmed \
                     FROM tool_executions WHERE tool_execution_id = ?",
                )
                .bind(tool_id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?
                .ok_or_else(inconsistent)?;
                let state: String = row.try_get("state")?;
                if row.try_get::<String, _>("work_id")? != transition.work_id.to_string()
                    || row.try_get::<String, _>("runtime_instance_id")?
                        != stale_runtime_id.to_string()
                    || !matches!(
                        state.as_str(),
                        "interrupted_before_dispatch" | "outcome_unknown" | "completed"
                    )
                {
                    return Err(inconsistent());
                }
                if let Some(classification_index) = tool_classifications.get(&tool_id).copied() {
                    let expected_state = match &events[classification_index].payload {
                        JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(_) => {
                            exact.tool_predispatch += 1;
                            "interrupted_before_dispatch"
                        }
                        JournalEventPayload::ToolExecutionOutcomeUnknown(_) => {
                            exact.tool_unknown += 1;
                            "outcome_unknown"
                        }
                        _ => return Err(inconsistent()),
                    };
                    if event.causation_event_id != Some(events[classification_index].event_id)
                        || classification_index >= index
                        || state != expected_state
                        || !attributed_tools.insert(tool_id)
                    {
                        return Err(inconsistent());
                    }
                } else {
                    exact.tool_terminal_preserved += 1;
                }
                match state.as_str() {
                    "outcome_unknown" => {
                        exact.cleanup_checks += 1;
                        exact.cleanup_unconfirmed += 1;
                    }
                    "completed" => {
                        exact.cleanup_checks += 1;
                        exact.cleanup_unconfirmed += u64::from(
                            row.try_get::<Option<i64>, _>("cleanup_confirmed")? == Some(0),
                        );
                    }
                    "interrupted_before_dispatch" => {}
                    _ => return Err(inconsistent()),
                }
            }
        }
    }
    if attributed_models.len() != model_classifications.len()
        || attributed_tools.len() != tool_classifications.len()
        || summary.retained_queued_work != exact.retained_queued_work
        || summary.interrupted_work != exact.interrupted_work
        || summary.model_attempts_provider_outcome_unknown != exact.model_unknown
        || summary.model_attempts_terminal_preserved != exact.model_terminal_preserved
        || summary.tool_attempts_interrupted_before_dispatch != exact.tool_predispatch
        || summary.tool_attempts_outcome_unknown != exact.tool_unknown
        || summary.tool_attempts_terminal_preserved != exact.tool_terminal_preserved
        || summary.drafts_abandoned != exact.drafts_abandoned
        || summary.cleanup_checks_performed != exact.cleanup_checks
        || summary.cleanup_unconfirmed != exact.cleanup_unconfirmed
    {
        return Err(inconsistent());
    }
    Ok(())
}

pub(super) async fn verify_stage10_consistency(
    connection: &mut sqlx::SqliteConnection,
    projected: &crate::application::projector::ProjectedState,
    events: &[JournalEvent],
) -> Result<u64, SqliteAdapterError> {
    for event in events {
        let JournalEventPayload::WorkCancelRequested(transition) = &event.payload else {
            continue;
        };
        if transition.cancellation_reason != Some(WorkCancellationReason::GracefulShutdown) {
            continue;
        }
        let Some(owner) = transition.runtime_owner else {
            return Err(inconsistent());
        };
        let previous = events.iter().find(|candidate| {
            candidate.stream_id == event.stream_id
                && candidate.stream_seq.get().checked_add(1) == Some(event.stream_seq.get())
        });
        if !matches!(
            transition.from_state,
            WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool
        ) || transition.to_state != WorkState::CancelRequested
            || transition.expected_cancellation_reason.is_some()
            || transition.expected_runtime_owner != Some(owner)
            || transition.expected_current_attempt != transition.current_attempt
            || transition.terminal_reason.is_some()
            || event.actor != JournalActor::Runtime(owner)
            || event.runtime_instance_id != Some(owner)
            || event.causation_event_id != previous.map(|value| value.event_id)
        {
            return Err(inconsistent());
        }
    }
    let rows = sqlx::query("SELECT * FROM runtime_instances ORDER BY runtime_instance_id")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if rows.len() != projected.runtimes.len() {
        return Err(inconsistent());
    }
    for row in rows {
        let runtime_id =
            RuntimeInstanceId::parse_canonical(&row.try_get::<String, _>("runtime_instance_id")?)
                .map_err(|_| inconsistent())?;
        let runtime = projected
            .runtimes
            .get(&runtime_id)
            .ok_or_else(inconsistent)?;
        let started = &runtime.started;
        let state: String = row.try_get("state")?;
        let stopped_at: Option<String> = row.try_get("stopped_at")?;
        let stop_reason: Option<String> = row.try_get("stop_reason")?;
        let heartbeat: UtcTimestamp = row
            .try_get::<String, _>("last_heartbeat_at")?
            .parse()
            .map_err(|_| inconsistent())?;
        if row.try_get::<String, _>("craxii_id")? != started.craxii_id.to_string()
            || row.try_get::<String, _>("workstation_id")? != started.workstation_id.to_string()
            || row.try_get::<i64, _>("workstation_generation")?
                != started.workstation_generation.get()
            || row.try_get::<String, _>("linux_boot_id")? != started.linux_boot_id.as_str()
            || row.try_get::<i64, _>("process_id")? != started.process_id.get()
            || row.try_get::<String, _>("binary_version")? != started.binary_version.as_str()
            || row.try_get::<String, _>("git_revision")? != started.git_revision.as_str()
            || row.try_get::<i64, _>("schema_version")? != started.schema_version.get()
            || row.try_get::<String, _>("started_at")? != started.started_at.to_string()
            || heartbeat < started.started_at
        {
            return Err(inconsistent());
        }
        if let Some(recovery) = &runtime.recovery {
            if recovery.stale_runtimes_closed > recovery.stale_runtimes_observed
                || recovery.runtime_instance_id != runtime_id
                || recovery.recovered_at < started.started_at
            {
                return Err(inconsistent());
            }
            verify_exact_recovery_summary(connection, events, runtime_id, recovery).await?;
        }
        match state.as_str() {
            "running" => {
                if stopped_at.is_some() || stop_reason.is_some() || runtime.stopping.is_some() {
                    return Err(inconsistent());
                }
            }
            "stopping" => {
                if stopped_at.is_some()
                    || stop_reason.as_deref() != Some("graceful_shutdown")
                    || runtime.stopping.is_none()
                {
                    return Err(inconsistent());
                }
            }
            "stopped" => match stop_reason.as_deref() {
                Some("graceful_shutdown") if runtime.stopping.is_some() && stopped_at.is_some() => {
                }
                Some("startup_failure") if stopped_at.is_some() => {}
                _ => return Err(inconsistent()),
            },
            _ => return Err(inconsistent()),
        }
        if let Some(stopping) = &runtime.stopping
            && (stopping.shutdown_requested_at < started.started_at
                || stopping.grace_deadline < stopping.shutdown_requested_at)
        {
            return Err(inconsistent());
        }
    }

    let invalid_ownership: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_items w LEFT JOIN runtime_instances r \
         ON r.runtime_instance_id = w.runtime_instance_id WHERE \
         (w.state = 'queued' AND w.runtime_instance_id IS NOT NULL) OR \
         (w.state IN ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
          AND (w.runtime_instance_id IS NULL OR r.runtime_instance_id IS NULL \
               OR r.state NOT IN ('running','stopping'))) OR \
         (w.state IN ('completed','failed','cancelled','interrupted') \
          AND (w.runtime_instance_id IS NOT NULL OR w.current_model_invocation_id IS NOT NULL \
               OR w.current_tool_execution_id IS NOT NULL))",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let duplicate_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT conversation_id FROM work_items WHERE state IN \
         ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
         GROUP BY conversation_id HAVING COUNT(*) > 1)",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if invalid_ownership != 0 || duplicate_active != 0 {
        return Err(inconsistent());
    }
    Ok(8)
}

impl SchedulerStateStore for SqliteStateStore {
    fn claim_next_work(
        &self,
        request: ClaimNextWorkRequest,
    ) -> StateStoreFuture<'_, Option<ClaimedWork>> {
        Box::pin(async move { claim_next(self, request).await.map_err(map_port_error) })
    }

    fn list_current_runtime_cancel_requested(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> StateStoreFuture<'_, Vec<CancelRequestedWork>> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(map_port_error)?;
            let rows = sqlx::query(
                "SELECT work_id, current_model_invocation_id, current_tool_execution_id \
                 FROM work_items INDEXED BY ix_work_items_nonterminal_by_runtime \
                 WHERE runtime_instance_id = ? AND state IN \
                 ('running','waiting_on_model','waiting_on_tool','cancel_requested') \
                 AND state = 'cancel_requested' \
                 ORDER BY conversation_id, conversation_work_ordinal",
            )
            .bind(runtime_id.to_string())
            .fetch_all(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)
            .map_err(map_port_error)?;
            rows.into_iter()
                .map(|row| {
                    let work_id = WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
                        .map_err(|_| inconsistent())?;
                    let model = row.try_get::<Option<String>, _>("current_model_invocation_id")?;
                    let tool = row.try_get::<Option<String>, _>("current_tool_execution_id")?;
                    Ok(CancelRequestedWork {
                        work_id,
                        current_attempt: decode_current_attempt(model.as_deref(), tool.as_deref())?,
                    })
                })
                .collect::<Result<Vec<_>, SqliteAdapterError>>()
                .map_err(map_port_error)
        })
    }

    fn finish_cancellation(
        &self,
        request: FinishCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            finish_cancellation(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn interrupt_abnormal_runner(
        &self,
        request: InterruptOwnedWorkRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            interrupt_owned_work(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn request_owned_work_cancellation(
        &self,
        request: RequestOwnedCancellationRequest,
    ) -> StateStoreFuture<'_, Vec<WorkId>> {
        Box::pin(async move {
            request_owned_cancellation(self, request)
                .await
                .map_err(map_port_error)
        })
    }
}

impl RuntimeStateStore for SqliteStateStore {
    fn create_runtime_and_started_event(
        &self,
        request: CreateRuntimeRequest,
    ) -> StateStoreFuture<'_, CreateRuntimeReceipt> {
        Box::pin(async move { create_runtime(self, request).await.map_err(map_port_error) })
    }

    fn heartbeat_runtime(
        &self,
        request: HeartbeatRuntimeRequest,
    ) -> StateStoreFuture<'_, HeartbeatRuntimeReceipt> {
        Box::pin(async move {
            heartbeat_runtime(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn begin_runtime_stopping(
        &self,
        request: BeginRuntimeStoppingRequest,
    ) -> StateStoreFuture<'_, BeginRuntimeStoppingReceipt> {
        Box::pin(async move {
            begin_runtime_stopping(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn finish_runtime_graceful(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            finish_runtime(self, request, true)
                .await
                .map_err(map_port_error)
        })
    }

    fn mark_runtime_startup_failure(
        &self,
        request: FinishRuntimeRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            finish_runtime(self, request, false)
                .await
                .map_err(map_port_error)
        })
    }

    fn enumerate_stale_runtimes(
        &self,
        request: EnumerateStaleRuntimesRequest,
    ) -> StateStoreFuture<'_, Vec<RuntimeInstanceId>> {
        Box::pin(async move { enumerate_stale(self, request).await.map_err(map_port_error) })
    }

    fn append_recovery_summary(
        &self,
        request: AppendRecoverySummaryRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            append_recovery_summary(self, request)
                .await
                .map_err(map_port_error)
        })
    }
}

impl RecoveryStateStore for SqliteStateStore {
    fn count_retained_queued_work(&self) -> StateStoreFuture<'_, u64> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(map_port_error)?;
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM work_items WHERE state = 'queued' AND runtime_instance_id IS NULL",
            )
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)
            .map_err(map_port_error)?;
            u64::try_from(count).map_err(|_| {
                crate::ports::state_store::StateStoreError::new(
                    crate::ports::state_store::StateStoreErrorKind::InternalInvariant,
                )
            })
        })
    }

    fn recover_stale_runtime_ownership(
        &self,
        request: RecoverStaleRuntimeRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt> {
        Box::pin(async move {
            recover_stale_runtime(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn classify_unresolved_shutdown_work(
        &self,
        request: ClassifyShutdownWorkRequest,
    ) -> StateStoreFuture<'_, RecoveryReceipt> {
        Box::pin(async move {
            classify_shutdown_work(self, request)
                .await
                .map_err(map_port_error)
        })
    }
}
