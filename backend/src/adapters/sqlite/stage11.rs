//! Stage 11 atomic client snapshot and bounded durable replay reads.

use sqlx::Row;

use crate::domain::{
    AgentStepNo, Conversation, ConversationId, ConversationWorkOrdinal, CraxiiId, CraxiiPrincipal,
    CraxiiPrincipalInput, JournalOffset, JournalWorkTerminalReason, MessageId, SchemaVersion,
    ToolExecutionId, ToolExecutionState, ToolName, ToolOrdinal, WorkId,
};
use crate::ports::state_store::{
    ClientBootstrapCandidate, ClientMessageCandidate, ClientToolCandidate, ClientWorkCandidate,
    ListPublicJournalRequest, PublicJournalPage, ReplayStateStore, StateStoreFuture,
};

use super::codec::{
    decode_message_row, decode_optional_timestamp, decode_timestamp, decode_work_state,
};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::decode_event_row;
use super::stage8_codec::validate_tool_result;
use super::state_store::SqliteStateStore;

const MAX_TERMINAL_WORK: i64 = 512;
const MAX_MESSAGES_PLUS_ONE: i64 = 2_049;
const MAX_TOOLS_PLUS_ONE: i64 = 2_049;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage11SnapshotPoint {
    BeforeHeadRead,
    AfterHeadRead,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct Stage11SnapshotBarrier {
    reached: tokio::sync::Semaphore,
    released: tokio::sync::Semaphore,
}

#[cfg(test)]
impl Stage11SnapshotBarrier {
    pub(super) fn new() -> Self {
        Self {
            reached: tokio::sync::Semaphore::new(0),
            released: tokio::sync::Semaphore::new(0),
        }
    }

    pub(super) async fn wait_until_reached(&self) {
        self.reached
            .acquire()
            .await
            .expect("snapshot barrier must remain open")
            .forget();
    }

    pub(super) fn release(&self) {
        self.released.add_permits(1);
    }

    async fn hold(&self) {
        self.reached.add_permits(1);
        self.released
            .acquire()
            .await
            .expect("snapshot barrier must remain open")
            .forget();
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct Stage11SnapshotTestHook {
    point: Stage11SnapshotPoint,
    barrier: std::sync::Arc<Stage11SnapshotBarrier>,
}

#[cfg(test)]
impl Stage11SnapshotTestHook {
    pub(super) fn new(
        point: Stage11SnapshotPoint,
        barrier: std::sync::Arc<Stage11SnapshotBarrier>,
    ) -> Self {
        Self { point, barrier }
    }
}

impl SqliteStateStore {
    pub(super) async fn current_high_water_inner(
        &self,
    ) -> Result<JournalOffset, SqliteAdapterError> {
        let value: i64 = sqlx::query_scalar("SELECT max(journal_offset) FROM journal_events")
            .fetch_one(&self.runtime.inner.pool)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        JournalOffset::try_new(value).map_err(|_| inconsistent())
    }

    pub(super) async fn load_client_bootstrap_inner(
        &self,
    ) -> Result<ClientBootstrapCandidate, SqliteAdapterError> {
        let mut transaction = self
            .runtime
            .inner
            .pool
            .begin()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;

        #[cfg(test)]
        self.fire_stage11_snapshot_hook(Stage11SnapshotPoint::BeforeHeadRead)
            .await;
        // This MUST be the first read: it establishes the SQLite snapshot used by all projections.
        let head_value: i64 = sqlx::query_scalar("SELECT max(journal_offset) FROM journal_events")
            .fetch_one(&mut *transaction)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let snapshot_cursor = JournalOffset::try_new(head_value).map_err(|_| inconsistent())?;
        #[cfg(test)]
        self.fire_stage11_snapshot_hook(Stage11SnapshotPoint::AfterHeadRead)
            .await;

        let root_rows = sqlx::query(
            "SELECT p.craxii_id, p.display_name, p.owner_label, p.primary_conversation_id, \
                    p.default_workspace_id, p.created_at AS principal_created_at, \
                    p.architecture_revision, p.schema_revision, p.lifecycle_state AS principal_lifecycle, \
                    c.craxii_id AS conversation_owner, c.kind AS conversation_kind, \
                    c.lifecycle_state AS conversation_lifecycle, c.created_at AS conversation_created_at, \
                    c.next_work_ordinal, c.state_version \
             FROM craxii_principals p \
             JOIN conversations c ON c.conversation_id = p.primary_conversation_id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let [root] = root_rows.as_slice() else {
            return Err(inconsistent());
        };
        let craxii_id = CraxiiId::parse_canonical(&root.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?;
        let conversation_id =
            ConversationId::parse_canonical(&root.try_get::<String, _>("primary_conversation_id")?)
                .map_err(|_| inconsistent())?;
        let principal = CraxiiPrincipal::try_new(CraxiiPrincipalInput {
            craxii_id,
            display_name: root.try_get("display_name")?,
            owner_label: root.try_get("owner_label")?,
            primary_conversation_id: conversation_id,
            default_workspace_id: crate::domain::WorkspaceId::parse_canonical(
                &root.try_get::<String, _>("default_workspace_id")?,
            )
            .map_err(|_| inconsistent())?,
            created_at: decode_timestamp(&root.try_get::<String, _>("principal_created_at")?)?,
            architecture_revision: root.try_get("architecture_revision")?,
            schema_revision: SchemaVersion::try_new(root.try_get("schema_revision")?)
                .map_err(|_| inconsistent())?,
        })
        .map_err(|_| inconsistent())?;
        if root.try_get::<String, _>("principal_lifecycle")? != "active"
            || root.try_get::<String, _>("conversation_kind")? != "primary"
            || root.try_get::<String, _>("conversation_lifecycle")? != "active"
            || CraxiiId::parse_canonical(&root.try_get::<String, _>("conversation_owner")?)
                .map_err(|_| inconsistent())?
                != craxii_id
        {
            return Err(inconsistent());
        }
        let primary_conversation = Conversation::new(
            conversation_id,
            craxii_id,
            decode_timestamp(&root.try_get::<String, _>("conversation_created_at")?)?,
            ConversationWorkOrdinal::try_new(root.try_get("next_work_ordinal")?)
                .map_err(|_| inconsistent())?,
            crate::domain::ProjectionVersion::try_new(root.try_get("state_version")?)
                .map_err(|_| inconsistent())?,
        );

        let message_rows = sqlx::query(
            "SELECT m.*, j.stream_seq AS conversation_sequence \
             FROM messages m \
             JOIN journal_events j \
               ON json_extract(j.payload_json, '$.message_id') = m.message_id \
              AND j.event_type IN ('message.accepted', 'assistant.message_committed') \
             WHERE m.conversation_id = ? \
             ORDER BY j.stream_seq ASC \
             LIMIT ?",
        )
        .bind(conversation_id.to_string())
        .bind(MAX_MESSAGES_PLUS_ONE)
        .fetch_all(&mut *transaction)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let source_message_json_bytes = message_rows.iter().try_fold(0_usize, |total, row| {
            let json: String = row.try_get("content_json")?;
            total.checked_add(json.len()).ok_or_else(inconsistent)
        })?;
        let messages = message_rows
            .iter()
            .map(|row| {
                Ok(ClientMessageCandidate {
                    message: decode_message_row(row)?,
                    conversation_sequence: row.try_get("conversation_sequence")?,
                })
            })
            .collect::<Result<Vec<_>, SqliteAdapterError>>()?;

        let work_rows = sqlx::query(
            "WITH recent_terminal AS ( \
                 SELECT work_id FROM work_items WHERE state IN \
                    ('completed','failed','cancelled','interrupted') \
                 ORDER BY conversation_work_ordinal DESC LIMIT ? \
             ), selected_work AS ( \
                 SELECT work_id FROM work_items WHERE state IN \
                    ('queued','running','waiting_on_model','waiting_on_tool','cancel_requested') \
                 UNION ALL \
                 SELECT work_id FROM recent_terminal \
             ) \
             SELECT w.*, json_extract(j.payload_json, '$.message_id') AS trigger_message_id \
             FROM work_items w \
             JOIN selected_work s ON s.work_id = w.work_id \
             JOIN work_item_inputs i ON i.work_id = w.work_id AND i.relationship = 'trigger' \
             JOIN journal_events j ON j.event_id = i.input_event_id AND j.event_type = 'message.accepted' \
             WHERE w.conversation_id = ? \
             ORDER BY w.conversation_work_ordinal ASC",
        )
        .bind(MAX_TERMINAL_WORK)
        .bind(conversation_id.to_string())
        .fetch_all(&mut *transaction)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let work_items = work_rows
            .iter()
            .map(decode_client_work)
            .collect::<Result<Vec<_>, _>>()?;

        let tool_rows = sqlx::query(
            "WITH recent_terminal AS ( \
                 SELECT work_id FROM work_items WHERE state IN \
                    ('completed','failed','cancelled','interrupted') \
                 ORDER BY conversation_work_ordinal DESC LIMIT ? \
             ), selected_work AS ( \
                 SELECT work_id FROM work_items WHERE state IN \
                    ('queued','running','waiting_on_model','waiting_on_tool','cancel_requested') \
                 UNION ALL \
                 SELECT work_id FROM recent_terminal \
             ) \
             SELECT t.tool_execution_id, t.work_id, w.conversation_work_ordinal, t.agent_step_no, \
                    t.tool_ordinal, t.tool_name, t.state, t.result_json, t.requested_at, \
                    t.started_at, t.completed_at, t.cleanup_confirmed \
             FROM tool_executions t \
             JOIN selected_work s ON s.work_id = t.work_id \
             JOIN work_items w ON w.work_id = t.work_id \
             ORDER BY w.conversation_work_ordinal, t.agent_step_no, t.tool_ordinal \
             LIMIT ?",
        )
        .bind(MAX_TERMINAL_WORK)
        .bind(MAX_TOOLS_PLUS_ONE)
        .fetch_all(&mut *transaction)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let tool_summaries = tool_rows
            .iter()
            .map(decode_client_tool)
            .collect::<Result<Vec<_>, _>>()?;

        transaction
            .commit()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        Ok(ClientBootstrapCandidate {
            snapshot_cursor,
            principal,
            primary_conversation,
            messages,
            work_items,
            tool_summaries,
            source_message_json_bytes,
        })
    }

    pub(super) async fn list_replay_page_inner(
        &self,
        request: ListPublicJournalRequest,
    ) -> Result<PublicJournalPage, SqliteAdapterError> {
        if request.limit == 0 || request.limit > crate::protocol::REPLAY_PAGE_ROWS {
            return Err(inconsistent());
        }
        let after = request.after.map_or(0, JournalOffset::get);
        if after >= request.through.get() {
            return Err(inconsistent());
        }
        let rows = sqlx::query(
            "SELECT * FROM journal_events \
             WHERE journal_offset > ? AND journal_offset <= ? \
             ORDER BY journal_offset ASC LIMIT ?",
        )
        .bind(after)
        .bind(request.through.get())
        .bind(i64::from(request.limit))
        .fetch_all(&self.runtime.inner.pool)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let candidates = rows
            .iter()
            .map(decode_event_row)
            .collect::<Result<Vec<_>, _>>()?;
        let last = candidates.last().map(|event| event.journal_offset);
        let has_more = if let Some(last) = last {
            sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM journal_events \
                 WHERE journal_offset > ? AND journal_offset <= ?)",
            )
            .bind(last.get())
            .bind(request.through.get())
            .fetch_one(&self.runtime.inner.pool)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?
                == 1
        } else {
            false
        };
        let scanned_through = if has_more {
            last.ok_or_else(inconsistent)?
        } else {
            request.through
        };
        Ok(PublicJournalPage {
            candidates,
            scanned_through,
            has_more,
        })
    }

    #[cfg(test)]
    pub(super) fn set_stage11_snapshot_hook(&self, hook: Option<Stage11SnapshotTestHook>) {
        *self.stage11_snapshot_hook.lock().unwrap() = hook;
    }

    #[cfg(test)]
    async fn fire_stage11_snapshot_hook(&self, point: Stage11SnapshotPoint) {
        let barrier = self
            .stage11_snapshot_hook
            .lock()
            .unwrap()
            .as_ref()
            .filter(|hook| hook.point == point)
            .map(|hook| std::sync::Arc::clone(&hook.barrier));
        if let Some(barrier) = barrier {
            barrier.hold().await;
        }
    }
}

impl ReplayStateStore for SqliteStateStore {
    fn current_journal_high_water(&self) -> StateStoreFuture<'_, JournalOffset> {
        Box::pin(async move {
            self.current_high_water_inner()
                .await
                .map_err(super::state_store::map_port_error)
        })
    }

    fn load_client_bootstrap_snapshot(&self) -> StateStoreFuture<'_, ClientBootstrapCandidate> {
        Box::pin(async move {
            self.load_client_bootstrap_inner()
                .await
                .map_err(super::state_store::map_port_error)
        })
    }

    fn list_public_journal_replay_candidates(
        &self,
        request: ListPublicJournalRequest,
    ) -> StateStoreFuture<'_, PublicJournalPage> {
        Box::pin(async move {
            self.list_replay_page_inner(request)
                .await
                .map_err(super::state_store::map_port_error)
        })
    }
}

fn decode_client_work(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ClientWorkCandidate, SqliteAdapterError> {
    Ok(ClientWorkCandidate {
        work_id: WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
            .map_err(|_| inconsistent())?,
        conversation_id: ConversationId::parse_canonical(
            &row.try_get::<String, _>("conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        conversation_work_ordinal: ConversationWorkOrdinal::try_new(
            row.try_get("conversation_work_ordinal")?,
        )
        .map_err(|_| inconsistent())?,
        state: decode_work_state(&row.try_get::<String, _>("state")?)?,
        trigger_message_id: MessageId::parse_canonical(
            &row.try_get::<String, _>("trigger_message_id")?,
        )
        .map_err(|_| inconsistent())?,
        created_at: decode_timestamp(&row.try_get::<String, _>("created_at")?)?,
        queued_at: decode_timestamp(&row.try_get::<String, _>("queued_at")?)?,
        started_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("started_at")?.as_deref(),
        )?,
        cancel_requested_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("cancel_requested_at")?
                .as_deref(),
        )?,
        terminal_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("terminal_at")?.as_deref(),
        )?,
        terminal_reason: row
            .try_get::<Option<String>, _>("terminal_reason_code")?
            .map(|value| decode_terminal_reason(&value))
            .transpose()?,
    })
}

fn decode_client_tool(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ClientToolCandidate, SqliteAdapterError> {
    let state = match row.try_get::<String, _>("state")?.as_str() {
        "requested" => ToolExecutionState::Requested,
        "dispatching" => ToolExecutionState::Dispatching,
        "completed" => ToolExecutionState::Completed,
        "interrupted_before_dispatch" => ToolExecutionState::InterruptedBeforeDispatch,
        "outcome_unknown" => ToolExecutionState::OutcomeUnknown,
        _ => return Err(inconsistent()),
    };
    let cleanup_confirmed = row
        .try_get::<Option<i64>, _>("cleanup_confirmed")?
        .map(|value| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(inconsistent()),
        })
        .transpose()?;
    let result_class = row
        .try_get::<Option<String>, _>("result_json")?
        .map(|json| validate_tool_result(&json))
        .transpose()?;
    Ok(ClientToolCandidate {
        work_id: WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
            .map_err(|_| inconsistent())?,
        work_ordinal: ConversationWorkOrdinal::try_new(row.try_get("conversation_work_ordinal")?)
            .map_err(|_| inconsistent())?,
        agent_step_no: AgentStepNo::try_new(row.try_get("agent_step_no")?)
            .map_err(|_| inconsistent())?,
        tool_ordinal: ToolOrdinal::try_new(row.try_get("tool_ordinal")?)
            .map_err(|_| inconsistent())?,
        tool_execution_id: ToolExecutionId::parse_canonical(
            &row.try_get::<String, _>("tool_execution_id")?,
        )
        .map_err(|_| inconsistent())?,
        tool_name: ToolName::try_new(row.try_get::<String, _>("tool_name")?)
            .map_err(|_| inconsistent())?,
        state,
        result_class,
        requested_at: decode_timestamp(&row.try_get::<String, _>("requested_at")?)?,
        started_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("started_at")?.as_deref(),
        )?,
        completed_at: decode_optional_timestamp(
            row.try_get::<Option<String>, _>("completed_at")?.as_deref(),
        )?,
        cleanup_confirmed,
    })
}

fn decode_terminal_reason(value: &str) -> Result<JournalWorkTerminalReason, SqliteAdapterError> {
    use JournalWorkTerminalReason as Reason;
    match value {
        "answered" => Ok(Reason::Answered),
        "refused" => Ok(Reason::Refused),
        "definite_normalized_error" => Ok(Reason::DefiniteNormalizedError),
        "provider_exhausted" => Ok(Reason::ProviderExhausted),
        "invalid_model_output" => Ok(Reason::InvalidModelOutput),
        "lifecycle_limit" => Ok(Reason::LifecycleLimit),
        "user_request" => Ok(Reason::UserRequest),
        "graceful_shutdown" => Ok(Reason::GracefulShutdown),
        "runtime_ownership_lost" => Ok(Reason::RuntimeOwnershipLost),
        "provider_outcome_unknown" => Ok(Reason::ProviderOutcomeUnknown),
        "tool_interrupted_before_dispatch" => Ok(Reason::ToolInterruptedBeforeDispatch),
        "tool_outcome_unknown" => Ok(Reason::ToolOutcomeUnknown),
        "cleanup_unconfirmed" => Ok(Reason::CleanupUnconfirmed),
        _ => Err(inconsistent()),
    }
}

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}
