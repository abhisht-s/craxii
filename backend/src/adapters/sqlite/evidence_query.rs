//! SQLite-backed, read-only Stage 23 evidence projections.

use sqlx::{Row, SqliteConnection};

use crate::application::observability::SafeProviderCorrelation;
use crate::domain::{RuntimeInstanceId, WorkId};
use crate::ports::artifact_store::ArtifactStore;
use crate::ports::evidence_query::{
    ArtifactObservation, ContextObservation, EvidenceExport, EvidenceFuture, EvidencePreflight,
    EvidenceQueryError, EvidenceQueryErrorKind, EvidenceQueryStore, JournalObservation,
    ModelAttemptObservation, RecoveryObservation, RuntimeEvidence, StateVerification,
    ToolExecutionObservation, VerificationIssue, WorkEvidence,
};
use crate::ports::state_store::BootstrapStateStore;

use super::{SqliteRuntime, SqliteStateStore};

#[derive(Clone, Debug)]
pub struct SqliteEvidenceQueryStore {
    runtime: SqliteRuntime,
}

impl SqliteEvidenceQueryStore {
    #[must_use]
    pub const fn new(runtime: SqliteRuntime) -> Self {
        Self { runtime }
    }

    async fn preflight_inner(&self) -> Result<EvidencePreflight, EvidenceQueryError> {
        let mut connection = self.connection().await?;
        let row = sqlx::query(
            "SELECT (SELECT MAX(version) FROM _sqlx_migrations) AS schema_version, \
             (SELECT MAX(journal_offset) FROM journal_events) AS journal_head, \
             (SELECT COUNT(*) FROM work_items) AS work_count, \
             (SELECT COUNT(*) FROM runtime_instances) AS runtime_count, \
             (SELECT COUNT(*) FROM model_invocations) AS model_attempt_count, \
             (SELECT COUNT(*) FROM tool_executions) AS tool_execution_count, \
             (SELECT COUNT(*) FROM artifacts) AS artifact_count",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(map_sqlx)?;
        Ok(EvidencePreflight {
            schema_version: required_u64(&row, "schema_version")?,
            database_disposition: "current",
            journal_head: optional_u64(&row, "journal_head")?,
            work_count: required_u64(&row, "work_count")?,
            runtime_count: required_u64(&row, "runtime_count")?,
            model_attempt_count: required_u64(&row, "model_attempt_count")?,
            tool_execution_count: required_u64(&row, "tool_execution_count")?,
            artifact_count: required_u64(&row, "artifact_count")?,
        })
    }

    async fn verify_state_inner(
        &self,
        artifacts: &dyn ArtifactStore,
    ) -> Result<StateVerification, EvidenceQueryError> {
        let state_store = SqliteStateStore::new(self.runtime.clone());
        let mut report = StateVerification {
            consistent: true,
            checked_invariants: None,
            journal_head: None,
            referenced_artifact_count: 0,
            verified_artifact_count: 0,
            issues: Vec::new(),
        };
        match state_store.verify_application_consistency().await {
            Ok(receipt) => {
                report.checked_invariants = Some(receipt.checked_invariants);
                report.journal_head = receipt
                    .journal_head
                    .map(|value| u64::try_from(value.get()).map_err(|_| integrity()))
                    .transpose()?;
            }
            Err(_) => {
                report.consistent = false;
                report
                    .issues
                    .push(VerificationIssue::JournalProjectionInconsistent);
            }
        }
        match state_store.load_referenced_artifacts().await {
            Ok(references) => {
                report.referenced_artifact_count = u64::try_from(references.len())
                    .map_err(|_| EvidenceQueryError::new(EvidenceQueryErrorKind::Integrity))?;
                for reference in references {
                    if artifacts.verify(&reference).is_ok() {
                        report.verified_artifact_count += 1;
                    } else {
                        report.consistent = false;
                        if !report
                            .issues
                            .contains(&VerificationIssue::ReferencedArtifactMissingOrCorrupt)
                        {
                            report
                                .issues
                                .push(VerificationIssue::ReferencedArtifactMissingOrCorrupt);
                        }
                    }
                }
            }
            Err(_) => {
                report.consistent = false;
                report
                    .issues
                    .push(VerificationIssue::ArtifactMetadataInconsistent);
            }
        }
        Ok(report)
    }

    async fn inspect_work_inner(
        &self,
        work_id: WorkId,
    ) -> Result<WorkEvidence, EvidenceQueryError> {
        let mut connection = self.connection().await?;
        let id = work_id.to_string();
        let row = sqlx::query(
            "SELECT work_id, craxii_id, conversation_id, conversation_work_ordinal, workspace_id, \
             correlation_id, state, state_version, runtime_instance_id, current_model_invocation_id, \
             current_tool_execution_id, created_at, queued_at, started_at, cancel_requested_at, \
             cancellation_reason_code, terminal_at, terminal_reason_code FROM work_items WHERE work_id = ?",
        )
        .bind(&id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(not_found)?;
        let journal = load_journal(&mut connection, "work_id", &id).await?;
        let contexts = load_contexts(&mut connection, &id).await?;
        let model_attempts = load_models(&mut connection, &id).await?;
        let tool_executions = load_tools(&mut connection, &id).await?;
        let artifacts = load_artifacts(&mut connection, &id).await?;
        Ok(WorkEvidence {
            work_id: required_string(&row, "work_id")?,
            craxii_id: required_string(&row, "craxii_id")?,
            conversation_id: required_string(&row, "conversation_id")?,
            conversation_work_ordinal: required_u64(&row, "conversation_work_ordinal")?,
            workspace_id: required_string(&row, "workspace_id")?,
            correlation_id: required_string(&row, "correlation_id")?,
            state: required_string(&row, "state")?,
            state_version: required_u64(&row, "state_version")?,
            runtime_instance_id: optional_string(&row, "runtime_instance_id")?,
            current_model_invocation_id: optional_string(&row, "current_model_invocation_id")?,
            current_tool_execution_id: optional_string(&row, "current_tool_execution_id")?,
            created_at: required_string(&row, "created_at")?,
            queued_at: required_string(&row, "queued_at")?,
            started_at: optional_string(&row, "started_at")?,
            cancel_requested_at: optional_string(&row, "cancel_requested_at")?,
            cancellation_reason_code: optional_string(&row, "cancellation_reason_code")?,
            terminal_at: optional_string(&row, "terminal_at")?,
            terminal_reason_code: optional_string(&row, "terminal_reason_code")?,
            journal,
            contexts,
            model_attempts,
            tool_executions,
            artifacts,
        })
    }

    async fn inspect_runtime_inner(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> Result<RuntimeEvidence, EvidenceQueryError> {
        let mut connection = self.connection().await?;
        let id = runtime_id.to_string();
        let row = sqlx::query(
            "SELECT runtime_instance_id, craxii_id, workstation_id, workstation_generation, \
             binary_version, git_revision, schema_version, state, started_at, last_heartbeat_at, \
             stopped_at, stop_reason, (SELECT COUNT(*) FROM work_items WHERE runtime_instance_id = ?) AS owned_work_count, \
             (SELECT COUNT(*) FROM model_invocations WHERE runtime_instance_id = ?) AS model_attempt_count, \
             (SELECT COUNT(*) FROM tool_executions WHERE runtime_instance_id = ?) AS tool_execution_count \
             FROM runtime_instances WHERE runtime_instance_id = ?",
        )
        .bind(&id)
        .bind(&id)
        .bind(&id)
        .bind(&id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(not_found)?;
        let journal = load_journal(&mut connection, "runtime_instance_id", &id).await?;
        let recovery = load_recovery(&mut connection, &id).await?;
        Ok(RuntimeEvidence {
            runtime_instance_id: required_string(&row, "runtime_instance_id")?,
            craxii_id: required_string(&row, "craxii_id")?,
            workstation_id: required_string(&row, "workstation_id")?,
            workstation_generation: required_u64(&row, "workstation_generation")?,
            binary_version: required_string(&row, "binary_version")?,
            git_revision: required_string(&row, "git_revision")?,
            schema_version: required_u64(&row, "schema_version")?,
            state: required_string(&row, "state")?,
            started_at: required_string(&row, "started_at")?,
            last_heartbeat_at: optional_string(&row, "last_heartbeat_at")?,
            stopped_at: optional_string(&row, "stopped_at")?,
            stop_reason: optional_string(&row, "stop_reason")?,
            owned_work_count: required_u64(&row, "owned_work_count")?,
            model_attempt_count: required_u64(&row, "model_attempt_count")?,
            tool_execution_count: required_u64(&row, "tool_execution_count")?,
            journal,
            recovery,
        })
    }

    async fn connection(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, EvidenceQueryError> {
        self.runtime
            .acquire()
            .await
            .map_err(|_| EvidenceQueryError::new(EvidenceQueryErrorKind::Storage))
    }
}

impl EvidenceQueryStore for SqliteEvidenceQueryStore {
    fn preflight(&self) -> EvidenceFuture<'_, EvidencePreflight> {
        Box::pin(self.preflight_inner())
    }

    fn verify_state<'a>(
        &'a self,
        artifacts: &'a dyn ArtifactStore,
    ) -> EvidenceFuture<'a, StateVerification> {
        Box::pin(self.verify_state_inner(artifacts))
    }

    fn inspect_work(&self, work_id: WorkId) -> EvidenceFuture<'_, WorkEvidence> {
        Box::pin(self.inspect_work_inner(work_id))
    }

    fn inspect_runtime(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> EvidenceFuture<'_, RuntimeEvidence> {
        Box::pin(self.inspect_runtime_inner(runtime_id))
    }

    fn export<'a>(
        &'a self,
        artifacts: &'a dyn ArtifactStore,
    ) -> EvidenceFuture<'a, EvidenceExport> {
        Box::pin(async move {
            let preflight = self.preflight_inner().await?;
            let verification = self.verify_state_inner(artifacts).await?;
            let mut connection = self.connection().await?;
            let work_ids: Vec<String> =
                sqlx::query_scalar("SELECT work_id FROM work_items ORDER BY work_id")
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(map_sqlx)?;
            let runtime_ids: Vec<String> = sqlx::query_scalar(
                "SELECT runtime_instance_id FROM runtime_instances ORDER BY runtime_instance_id",
            )
            .fetch_all(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            drop(connection);
            let mut works = Vec::with_capacity(work_ids.len());
            for id in work_ids {
                works.push(
                    self.inspect_work_inner(WorkId::parse_canonical(&id).map_err(|_| integrity())?)
                        .await?,
                );
            }
            let mut runtimes = Vec::with_capacity(runtime_ids.len());
            for id in runtime_ids {
                runtimes.push(
                    self.inspect_runtime_inner(
                        RuntimeInstanceId::parse_canonical(&id).map_err(|_| integrity())?,
                    )
                    .await?,
                );
            }
            Ok(EvidenceExport {
                preflight,
                verification,
                works,
                runtimes,
            })
        })
    }
}

async fn load_journal(
    connection: &mut SqliteConnection,
    column: &'static str,
    value: &str,
) -> Result<Vec<JournalObservation>, EvidenceQueryError> {
    let sql = match column {
        "work_id" => {
            "SELECT journal_offset, event_id, stream_seq, event_type, event_version, causation_event_id, \
             correlation_id, runtime_instance_id, payload_sha256, recorded_at, occurred_at \
             FROM journal_events WHERE work_id = ? ORDER BY journal_offset"
        }
        "runtime_instance_id" => {
            "SELECT journal_offset, event_id, stream_seq, event_type, event_version, causation_event_id, \
             correlation_id, runtime_instance_id, payload_sha256, recorded_at, occurred_at \
             FROM journal_events WHERE runtime_instance_id = ? ORDER BY journal_offset"
        }
        _ => return Err(integrity()),
    };
    sqlx::query(sql)
        .bind(value)
        .fetch_all(connection)
        .await
        .map_err(map_sqlx)?
        .iter()
        .map(journal_from_row)
        .collect()
}

fn journal_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<JournalObservation, EvidenceQueryError> {
    Ok(JournalObservation {
        journal_offset: required_u64(row, "journal_offset")?,
        event_id: required_string(row, "event_id")?,
        stream_sequence: required_u64(row, "stream_seq")?,
        event_type: required_string(row, "event_type")?,
        event_version: required_u64(row, "event_version")?,
        causation_event_id: optional_string(row, "causation_event_id")?,
        correlation_id: required_string(row, "correlation_id")?,
        runtime_instance_id: optional_string(row, "runtime_instance_id")?,
        payload_sha256: required_string(row, "payload_sha256")?,
        recorded_at: required_string(row, "recorded_at")?,
        occurred_at: optional_string(row, "occurred_at")?,
    })
}

async fn load_contexts(
    connection: &mut SqliteConnection,
    work_id: &str,
) -> Result<Vec<ContextObservation>, EvidenceQueryError> {
    let rows = sqlx::query(
        "SELECT context_manifest_id, logical_invocation_id, model_target_id, provider_id, provider_model_id, \
         target_configuration_version, assembler_version, context_policy_version, source_count, canonical_byte_count, \
         rendered_request_byte_count, estimated_input_tokens, token_estimator_id, context_window_tokens, \
         reserved_output_tokens, utilization_basis_points, manifest_sha256, rendered_request_sha256, \
         rendered_request_artifact_id, created_at FROM context_manifests WHERE work_id = ? \
         ORDER BY created_at, context_manifest_id",
    )
    .bind(work_id)
    .fetch_all(connection)
    .await
    .map_err(map_sqlx)?;
    rows.iter()
        .map(|row| {
            Ok(ContextObservation {
                context_manifest_id: required_string(row, "context_manifest_id")?,
                logical_invocation_id: required_string(row, "logical_invocation_id")?,
                target: required_string(row, "model_target_id")?,
                provider: required_string(row, "provider_id")?,
                model: required_string(row, "provider_model_id")?,
                target_configuration_version: required_u64(row, "target_configuration_version")?,
                assembler_version: required_string(row, "assembler_version")?,
                context_policy_version: required_string(row, "context_policy_version")?,
                source_count: required_u64(row, "source_count")?,
                canonical_byte_count: required_u64(row, "canonical_byte_count")?,
                rendered_request_byte_count: required_u64(row, "rendered_request_byte_count")?,
                estimated_input_tokens: required_u64(row, "estimated_input_tokens")?,
                token_estimator_id: required_string(row, "token_estimator_id")?,
                context_window_tokens: required_u64(row, "context_window_tokens")?,
                reserved_output_tokens: required_u64(row, "reserved_output_tokens")?,
                utilization_basis_points: required_u64(row, "utilization_basis_points")?,
                manifest_sha256: required_string(row, "manifest_sha256")?,
                rendered_request_sha256: required_string(row, "rendered_request_sha256")?,
                rendered_request_artifact_id: optional_string(row, "rendered_request_artifact_id")?,
                created_at: required_string(row, "created_at")?,
            })
        })
        .collect()
}

async fn load_models(
    connection: &mut SqliteConnection,
    work_id: &str,
) -> Result<Vec<ModelAttemptObservation>, EvidenceQueryError> {
    let rows = sqlx::query(
        "SELECT model_invocation_id, logical_invocation_id, runtime_instance_id, context_manifest_id, \
         agent_step_no, attempt_no, retry_of_invocation_id, model_target_id, provider_id, provider_model_id, \
         selection_reason, state, request_sha256, response_sha256, request_artifact_id, response_artifact_id, \
         started_at, first_byte_at, first_output_at, completed_at, usage_status, input_tokens, cached_input_tokens, \
         output_tokens, reasoning_tokens, total_tokens, stop_reason, tool_call_count, draft_exposed, \
         provider_request_id, provider_response_id, provider_error_kind, provider_outcome_certainty, retry_reason, \
         retry_delay_ms, provider_retry_after_ms, billing_ambiguity FROM model_invocations WHERE work_id = ? \
         ORDER BY agent_step_no, attempt_no, model_invocation_id",
    )
    .bind(work_id)
    .fetch_all(connection)
    .await
    .map_err(map_sqlx)?;
    rows.iter().map(model_from_row).collect()
}

fn model_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ModelAttemptObservation, EvidenceQueryError> {
    let request_id = optional_string(row, "provider_request_id")?;
    let response_id = optional_string(row, "provider_response_id")?;
    Ok(ModelAttemptObservation {
        model_invocation_id: required_string(row, "model_invocation_id")?,
        logical_invocation_id: required_string(row, "logical_invocation_id")?,
        runtime_instance_id: required_string(row, "runtime_instance_id")?,
        context_manifest_id: required_string(row, "context_manifest_id")?,
        agent_step: required_u64(row, "agent_step_no")?,
        attempt: required_u64(row, "attempt_no")?,
        retry_of_invocation_id: optional_string(row, "retry_of_invocation_id")?,
        target: required_string(row, "model_target_id")?,
        provider: required_string(row, "provider_id")?,
        model: required_string(row, "provider_model_id")?,
        selection_reason: required_string(row, "selection_reason")?,
        state: required_string(row, "state")?,
        request_sha256: required_string(row, "request_sha256")?,
        response_sha256: optional_string(row, "response_sha256")?,
        request_artifact_id: optional_string(row, "request_artifact_id")?,
        response_artifact_id: optional_string(row, "response_artifact_id")?,
        started_at: required_string(row, "started_at")?,
        first_byte_at: optional_string(row, "first_byte_at")?,
        first_output_at: optional_string(row, "first_output_at")?,
        completed_at: optional_string(row, "completed_at")?,
        usage_status: required_string(row, "usage_status")?,
        input_tokens: optional_u64(row, "input_tokens")?,
        cached_input_tokens: optional_u64(row, "cached_input_tokens")?,
        output_tokens: optional_u64(row, "output_tokens")?,
        reasoning_tokens: optional_u64(row, "reasoning_tokens")?,
        total_tokens: optional_u64(row, "total_tokens")?,
        stop_reason: optional_string(row, "stop_reason")?,
        tool_call_count: optional_u64(row, "tool_call_count")?,
        draft_exposed: required_bool(row, "draft_exposed")?,
        provider_request_digest: request_id
            .as_deref()
            .map(SafeProviderCorrelation::from_untrusted)
            .map(|value| value.as_str().to_owned()),
        provider_response_digest: response_id
            .as_deref()
            .map(SafeProviderCorrelation::from_untrusted)
            .map(|value| value.as_str().to_owned()),
        provider_error_kind: optional_string(row, "provider_error_kind")?,
        provider_outcome_certainty: optional_string(row, "provider_outcome_certainty")?,
        retry_reason: optional_string(row, "retry_reason")?,
        retry_delay_ms: optional_u64(row, "retry_delay_ms")?,
        provider_retry_after_ms: optional_u64(row, "provider_retry_after_ms")?,
        billing_ambiguity: required_bool(row, "billing_ambiguity")?,
    })
}

async fn load_tools(
    connection: &mut SqliteConnection,
    work_id: &str,
) -> Result<Vec<ToolExecutionObservation>, EvidenceQueryError> {
    let rows = sqlx::query(
        "SELECT tool_execution_id, execution_id, source_model_invocation_id, runtime_instance_id, agent_step_no, \
         tool_ordinal, tool_name, tool_version, tool_schema_version, arguments_sha256, workstation_id, \
         workstation_generation, workspace_id, requested_privilege, effective_privilege, timeout_ms, state, \
         dispatch_intent_at, requested_at, started_at, completed_at, json_extract(result_json, '$.result_kind') AS result_class, \
         exit_code, signal, timed_out, cancelled, cleanup_confirmed, stdout_artifact_id, stderr_artifact_id, \
         stdout_observed_bytes, stdout_captured_bytes, stderr_observed_bytes, stderr_captured_bytes, truncated \
         FROM tool_executions WHERE work_id = ? ORDER BY agent_step_no, tool_ordinal, tool_execution_id",
    )
    .bind(work_id)
    .fetch_all(connection)
    .await
    .map_err(map_sqlx)?;
    rows.iter().map(tool_from_row).collect()
}

fn tool_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ToolExecutionObservation, EvidenceQueryError> {
    Ok(ToolExecutionObservation {
        tool_execution_id: required_string(row, "tool_execution_id")?,
        workstation_execution_id: required_string(row, "execution_id")?,
        source_model_invocation_id: required_string(row, "source_model_invocation_id")?,
        runtime_instance_id: required_string(row, "runtime_instance_id")?,
        agent_step: required_u64(row, "agent_step_no")?,
        tool_ordinal: required_u64(row, "tool_ordinal")?,
        tool_name: required_string(row, "tool_name")?,
        tool_version: required_string(row, "tool_version")?,
        tool_schema_version: required_u64(row, "tool_schema_version")?,
        arguments_sha256: required_string(row, "arguments_sha256")?,
        workstation_id: required_string(row, "workstation_id")?,
        workstation_generation: required_u64(row, "workstation_generation")?,
        workspace_id: required_string(row, "workspace_id")?,
        requested_privilege: required_string(row, "requested_privilege")?,
        effective_privilege: optional_string(row, "effective_privilege")?,
        timeout_ms: required_u64(row, "timeout_ms")?,
        state: required_string(row, "state")?,
        dispatch_intent_at: optional_string(row, "dispatch_intent_at")?,
        requested_at: required_string(row, "requested_at")?,
        started_at: optional_string(row, "started_at")?,
        completed_at: optional_string(row, "completed_at")?,
        result_class: optional_string(row, "result_class")?,
        exit_code: row.try_get("exit_code").map_err(|_| integrity())?,
        signal: row.try_get("signal").map_err(|_| integrity())?,
        timed_out: optional_bool(row, "timed_out")?,
        cancelled: optional_bool(row, "cancelled")?,
        cleanup_confirmed: optional_bool(row, "cleanup_confirmed")?,
        stdout_artifact_id: optional_string(row, "stdout_artifact_id")?,
        stderr_artifact_id: optional_string(row, "stderr_artifact_id")?,
        stdout_observed_bytes: optional_u64(row, "stdout_observed_bytes")?,
        stdout_captured_bytes: optional_u64(row, "stdout_captured_bytes")?,
        stderr_observed_bytes: optional_u64(row, "stderr_observed_bytes")?,
        stderr_captured_bytes: optional_u64(row, "stderr_captured_bytes")?,
        truncated: required_bool(row, "truncated")?,
    })
}

async fn load_artifacts(
    connection: &mut SqliteConnection,
    work_id: &str,
) -> Result<Vec<ArtifactObservation>, EvidenceQueryError> {
    let rows = sqlx::query(
        "SELECT artifact_id, producing_work_id, producer_kind, producer_id, storage_key, sha256, \
         captured_byte_count, observed_byte_count, retention_class, truncated, created_at \
         FROM artifacts WHERE producing_work_id = ? ORDER BY artifact_id",
    )
    .bind(work_id)
    .fetch_all(connection)
    .await
    .map_err(map_sqlx)?;
    rows.iter()
        .map(|row| {
            Ok(ArtifactObservation {
                artifact_id: required_string(row, "artifact_id")?,
                producing_work_id: optional_string(row, "producing_work_id")?,
                producer_kind: optional_string(row, "producer_kind")?,
                producer_id: optional_string(row, "producer_id")?,
                storage_key: required_string(row, "storage_key")?,
                sha256: required_string(row, "sha256")?,
                captured_byte_count: required_u64(row, "captured_byte_count")?,
                observed_byte_count: optional_u64(row, "observed_byte_count")?,
                retention_class: required_string(row, "retention_class")?,
                truncated: required_bool(row, "truncated")?,
                created_at: required_string(row, "created_at")?,
            })
        })
        .collect()
}

async fn load_recovery(
    connection: &mut SqliteConnection,
    runtime_id: &str,
) -> Result<Vec<RecoveryObservation>, EvidenceQueryError> {
    let rows = sqlx::query(
        "SELECT journal_offset, recorded_at, \
         json_extract(payload_json, '$.stale_runtimes_observed') AS stale_runtime_count, \
         json_extract(payload_json, '$.retained_queued_work') AS queued_work_retained, \
         json_extract(payload_json, '$.interrupted_work') AS work_interrupted, \
         json_extract(payload_json, '$.model_attempts_provider_outcome_unknown') AS model_attempts_marked_unknown, \
         json_extract(payload_json, '$.tool_attempts_outcome_unknown') AS tool_attempts_marked_unknown, \
         json_extract(payload_json, '$.cleanup_checks_performed') AS cleanup_checks_performed, \
         json_extract(payload_json, '$.cleanup_unconfirmed') AS cleanup_unconfirmed, \
         json_extract(payload_json, '$.orphan_artifacts_observed') AS orphan_count \
         FROM journal_events WHERE runtime_instance_id = ? AND event_type = 'runtime.recovery_performed' \
         ORDER BY journal_offset",
    )
    .bind(runtime_id)
    .fetch_all(connection)
    .await
    .map_err(map_sqlx)?;
    rows.iter()
        .map(|row| {
            Ok(RecoveryObservation {
                journal_offset: required_u64(row, "journal_offset")?,
                recorded_at: required_string(row, "recorded_at")?,
                stale_runtime_count: optional_u64(row, "stale_runtime_count")?,
                queued_work_retained: optional_u64(row, "queued_work_retained")?,
                work_interrupted: optional_u64(row, "work_interrupted")?,
                model_attempts_marked_unknown: optional_u64(row, "model_attempts_marked_unknown")?,
                tool_attempts_marked_unknown: optional_u64(row, "tool_attempts_marked_unknown")?,
                cleanup_checks_performed: optional_u64(row, "cleanup_checks_performed")?,
                cleanup_unconfirmed: optional_u64(row, "cleanup_unconfirmed")?,
                orphan_count: optional_u64(row, "orphan_count")?,
            })
        })
        .collect()
}

fn required_string(
    row: &sqlx::sqlite::SqliteRow,
    name: &str,
) -> Result<String, EvidenceQueryError> {
    row.try_get(name).map_err(|_| integrity())
}

fn optional_string(
    row: &sqlx::sqlite::SqliteRow,
    name: &str,
) -> Result<Option<String>, EvidenceQueryError> {
    row.try_get(name).map_err(|_| integrity())
}

fn required_u64(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<u64, EvidenceQueryError> {
    let value: i64 = row.try_get(name).map_err(|_| integrity())?;
    u64::try_from(value).map_err(|_| integrity())
}

fn optional_u64(
    row: &sqlx::sqlite::SqliteRow,
    name: &str,
) -> Result<Option<u64>, EvidenceQueryError> {
    let value: Option<i64> = row.try_get(name).map_err(|_| integrity())?;
    value
        .map(u64::try_from)
        .transpose()
        .map_err(|_| integrity())
}

fn required_bool(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<bool, EvidenceQueryError> {
    match row.try_get::<i64, _>(name).map_err(|_| integrity())? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(integrity()),
    }
}

fn optional_bool(
    row: &sqlx::sqlite::SqliteRow,
    name: &str,
) -> Result<Option<bool>, EvidenceQueryError> {
    match row
        .try_get::<Option<i64>, _>(name)
        .map_err(|_| integrity())?
    {
        None => Ok(None),
        Some(0) => Ok(Some(false)),
        Some(1) => Ok(Some(true)),
        Some(_) => Err(integrity()),
    }
}

fn map_sqlx(_: sqlx::Error) -> EvidenceQueryError {
    EvidenceQueryError::new(EvidenceQueryErrorKind::Storage)
}

fn integrity() -> EvidenceQueryError {
    EvidenceQueryError::new(EvidenceQueryErrorKind::Integrity)
}

fn not_found() -> EvidenceQueryError {
    EvidenceQueryError::new(EvidenceQueryErrorKind::NotFound)
}
