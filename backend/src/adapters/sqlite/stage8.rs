use std::collections::{HashMap, HashSet};

use sqlx::Row;

use crate::domain::{
    ArtifactId, ArtifactProducer, ArtifactRecordedV1, ArtifactRetention, ArtifactStorageKey,
    AuthorityDecision, CleanupStatus, ConversationId, CorrelationId, CraxiiId, CurrentWorkAttempt,
    JournalActor, JournalCurrentAttempt, JournalEvent, JournalEventPayload, JournalStreamId,
    JournalWorkTerminalReason, ModelInvocationEventV1, ModelInvocationState, PrivilegeMode,
    RuntimeInstanceId, ToolExecutionEventV1, ToolExecutionLifecycle, ToolExecutionState,
    UtcTimestamp, WorkFailureReason, WorkId, WorkLifecycleSnapshot, WorkLifecycleSnapshotInput,
    WorkState, WorkTerminalReason, WorkTransitionV1, decide_tool_transition, is_legal_model_pair,
};
use crate::ports::state_store::{
    BeginModelInvocationRequest, CommitReceipt, CommitToolDispatchIntentRequest,
    CommittedEventRange, ContextModelRole, ContextSourceIdentity, ContextSourceKind,
    ContextSourceRecordKind, EventIntent, FinishModelInvocationRequest, FinishToolExecutionRequest,
    MarkModelStreamingRequest, ModelSelectionReason, ModelStateStore, PreparedArtifact,
    PreparedContextManifest, RequestToolExecutionRequest, StateStoreFuture, ToolStateStore,
    ToolStreamCounts, WorkExpectation,
};

use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{CommittedJournalPosition, JournalAppendIntent, append_event, prepare_event};
use super::projection::{ProjectionMutationError, WorkProjectionTimes, guarded_work_update};
use super::stage8_codec::{
    decode_model_capabilities, decode_tool_result, encode_attempt_error, encode_authority,
    encode_eligibility_cutoff, encode_model_capabilities, encode_model_usage,
    encode_normalized_output, encode_omissions, encode_output_policy, encode_provider_options,
    encode_required_capabilities, encode_tool_result, encode_transform, validate_attempt_error,
    validate_authority, validate_dispatch_evidence, validate_eligibility_cutoff,
    validate_normalized_output, validate_omissions, validate_output_policy,
    validate_provider_options, validate_required_capabilities, validate_transform,
};
use super::state_store::{SqliteStateStore, map_port_error};
use super::transaction::WriteTransaction;

fn invalid() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InternalInvariant)
}

fn conflict() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::StateConflict)
}

fn corrupt() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

struct WorkContext {
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    correlation_id: CorrelationId,
    started_at: UtcTimestamp,
}

async fn load_work_context(
    transaction: &mut WriteTransaction,
    expected: WorkExpectation,
) -> Result<WorkContext, SqliteAdapterError> {
    let row = sqlx::query(
        "SELECT craxii_id, conversation_id, correlation_id, started_at FROM work_items \
         WHERE work_id = ?",
    )
    .bind(expected.work_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    Ok(WorkContext {
        craxii_id: row
            .try_get::<String, _>("craxii_id")?
            .parse()
            .map_err(|_| corrupt())?,
        conversation_id: row
            .try_get::<String, _>("conversation_id")?
            .parse()
            .map_err(|_| corrupt())?,
        correlation_id: row
            .try_get::<String, _>("correlation_id")?
            .parse()
            .map_err(|_| corrupt())?,
        started_at: UtcTimestamp::parse_canonical(
            &row.try_get::<Option<String>, _>("started_at")?
                .ok_or_else(corrupt)?,
        )
        .map_err(|_| corrupt())?,
    })
}

fn expected_snapshot(
    expected: WorkExpectation,
) -> Result<WorkLifecycleSnapshot, SqliteAdapterError> {
    WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
        work_id: expected.work_id,
        state: expected.state,
        projection_version: expected.version,
        runtime_owner: expected.runtime_owner,
        current_attempt: expected.current_attempt,
        cancellation_reason: None,
        terminal_reason: None,
    })
    .map_err(|_| invalid())
}

fn map_projection_error<C>(error: ProjectionMutationError<C>) -> SqliteAdapterError {
    match error {
        ProjectionMutationError::Conflict(_) => conflict(),
        ProjectionMutationError::Storage(error) => error,
        ProjectionMutationError::Invariant => invalid(),
    }
}

fn journal_attempt(value: CurrentWorkAttempt) -> JournalCurrentAttempt {
    match value {
        CurrentWorkAttempt::None => JournalCurrentAttempt::None,
        CurrentWorkAttempt::Model(id) => JournalCurrentAttempt::Model(id),
        CurrentWorkAttempt::Tool(id) => JournalCurrentAttempt::Tool(id),
    }
}

fn journal_terminal_reason(
    reason: Option<&WorkTerminalReason>,
) -> Option<JournalWorkTerminalReason> {
    match reason {
        None => None,
        Some(WorkTerminalReason::Completion(value)) => Some(match value.as_str() {
            "answered" => JournalWorkTerminalReason::Answered,
            "refused" => JournalWorkTerminalReason::Refused,
            _ => unreachable!("closed completion reason"),
        }),
        Some(WorkTerminalReason::Cancellation(value)) => Some(match value.as_str() {
            "user_request" => JournalWorkTerminalReason::UserRequest,
            "graceful_shutdown" => JournalWorkTerminalReason::GracefulShutdown,
            _ => unreachable!("closed cancellation reason"),
        }),
        Some(WorkTerminalReason::Interruption(value)) => Some(match value.as_str() {
            "runtime_ownership_lost" => JournalWorkTerminalReason::RuntimeOwnershipLost,
            "provider_outcome_unknown" => JournalWorkTerminalReason::ProviderOutcomeUnknown,
            "tool_interrupted_before_dispatch" => {
                JournalWorkTerminalReason::ToolInterruptedBeforeDispatch
            }
            "tool_outcome_unknown" => JournalWorkTerminalReason::ToolOutcomeUnknown,
            "cleanup_unconfirmed" => JournalWorkTerminalReason::CleanupUnconfirmed,
            _ => unreachable!("closed interruption reason"),
        }),
        Some(WorkTerminalReason::Failure(value)) => Some(match value {
            WorkFailureReason::Definite(_) => JournalWorkTerminalReason::DefiniteNormalizedError,
            WorkFailureReason::ProviderExhausted => JournalWorkTerminalReason::ProviderExhausted,
            WorkFailureReason::InvalidModelOutput(_) => {
                JournalWorkTerminalReason::InvalidModelOutput
            }
            WorkFailureReason::Limit(_) => JournalWorkTerminalReason::LifecycleLimit,
        }),
    }
}

fn work_payload(
    expected: &WorkLifecycleSnapshot,
    next: &WorkLifecycleSnapshot,
    transitioned_at: UtcTimestamp,
) -> Result<JournalEventPayload, SqliteAdapterError> {
    let value = WorkTransitionV1 {
        work_id: expected.work_id(),
        from_state: expected.state(),
        to_state: next.state(),
        expected_state_version: expected.projection_version(),
        expected_runtime_owner: expected.runtime_owner(),
        expected_current_attempt: journal_attempt(expected.current_attempt()),
        expected_cancellation_reason: expected.cancellation_reason(),
        state_version: next.projection_version(),
        runtime_owner: next.runtime_owner(),
        current_attempt: journal_attempt(next.current_attempt()),
        cancellation_reason: next.cancellation_reason(),
        terminal_reason: journal_terminal_reason(next.terminal_reason()),
        transitioned_at,
    };
    Ok(match next.state() {
        WorkState::Running => JournalEventPayload::WorkResumed(value),
        WorkState::WaitingOnModel => JournalEventPayload::WorkWaitingOnModel(value),
        WorkState::WaitingOnTool => JournalEventPayload::WorkWaitingOnTool(value),
        WorkState::CancelRequested => JournalEventPayload::WorkCancelRequested(value),
        WorkState::Completed => JournalEventPayload::WorkCompleted(value),
        WorkState::Failed => JournalEventPayload::WorkFailed(value),
        WorkState::Cancelled => JournalEventPayload::WorkCancelled(value),
        WorkState::Interrupted => JournalEventPayload::WorkInterrupted(value),
        WorkState::Queued => return Err(invalid()),
    })
}

async fn append_work_event(
    transaction: &mut WriteTransaction,
    context: &WorkContext,
    runtime: Option<RuntimeInstanceId>,
    intent: EventIntent,
    payload: JournalEventPayload,
    recorded_at: UtcTimestamp,
) -> Result<CommittedJournalPosition, SqliteAdapterError> {
    if intent.correlation_id != context.correlation_id {
        return Err(invalid());
    }
    append_event(
        transaction,
        prepare_event(JournalAppendIntent {
            event_id: intent.event_id,
            craxii_id: context.craxii_id,
            stream_id: JournalStreamId::Work(match &payload {
                JournalEventPayload::WorkWaitingOnModel(value)
                | JournalEventPayload::WorkWaitingOnTool(value)
                | JournalEventPayload::WorkResumed(value)
                | JournalEventPayload::WorkCancelRequested(value)
                | JournalEventPayload::WorkCancelled(value)
                | JournalEventPayload::WorkCompleted(value)
                | JournalEventPayload::WorkFailed(value)
                | JournalEventPayload::WorkInterrupted(value) => value.work_id,
                JournalEventPayload::ModelInvocationStarted(value)
                | JournalEventPayload::ModelInvocationStreaming(value)
                | JournalEventPayload::ModelInvocationCompleted(value)
                | JournalEventPayload::ModelInvocationFailed(value)
                | JournalEventPayload::ModelInvocationInterrupted(value) => value.work_id,
                JournalEventPayload::ToolExecutionRequested(value)
                | JournalEventPayload::ToolExecutionDispatching(value)
                | JournalEventPayload::ToolExecutionCompleted(value)
                | JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value)
                | JournalEventPayload::ToolExecutionOutcomeUnknown(value) => value.work_id,
                JournalEventPayload::ArtifactRecorded(value) => value.work_id,
                _ => return Err(invalid()),
            }),
            conversation_id: Some(context.conversation_id),
            work_id: Some(match &payload {
                JournalEventPayload::WorkWaitingOnModel(value)
                | JournalEventPayload::WorkWaitingOnTool(value)
                | JournalEventPayload::WorkResumed(value)
                | JournalEventPayload::WorkCancelRequested(value)
                | JournalEventPayload::WorkCancelled(value)
                | JournalEventPayload::WorkCompleted(value)
                | JournalEventPayload::WorkFailed(value)
                | JournalEventPayload::WorkInterrupted(value) => value.work_id,
                JournalEventPayload::ModelInvocationStarted(value)
                | JournalEventPayload::ModelInvocationStreaming(value)
                | JournalEventPayload::ModelInvocationCompleted(value)
                | JournalEventPayload::ModelInvocationFailed(value)
                | JournalEventPayload::ModelInvocationInterrupted(value) => value.work_id,
                JournalEventPayload::ToolExecutionRequested(value)
                | JournalEventPayload::ToolExecutionDispatching(value)
                | JournalEventPayload::ToolExecutionCompleted(value)
                | JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(value)
                | JournalEventPayload::ToolExecutionOutcomeUnknown(value) => value.work_id,
                JournalEventPayload::ArtifactRecorded(value) => value.work_id,
                _ => return Err(invalid()),
            }),
            causation_event_id: intent.causation_event_id,
            correlation_id: intent.correlation_id,
            actor: JournalActor::Craxii(context.craxii_id),
            runtime_instance_id: runtime,
            payload,
            recorded_at,
            occurred_at: None,
        })?,
    )
    .await
}

fn retention_literal(value: ArtifactRetention) -> &'static str {
    match value {
        ArtifactRetention::CanonicalEvidence => "canonical_evidence",
        ArtifactRetention::Diagnostic => "diagnostic",
        ArtifactRetention::Regenerable => "regenerable",
    }
}

fn index_validated_artifacts(
    artifacts: &[PreparedArtifact],
    expected_work_id: WorkId,
    expected_producer: ArtifactProducer,
) -> Result<HashMap<ArtifactId, &PreparedArtifact>, SqliteAdapterError> {
    let mut indexed = HashMap::with_capacity(artifacts.len());
    let mut event_ids = HashSet::with_capacity(artifacts.len());
    for artifact in artifacts {
        let finalized = &artifact.finalized;
        let metadata = &artifact.metadata;
        if finalized.artifact_id() != metadata.artifact_id()
            || finalized.storage_key() != metadata.storage_key()
            || finalized.sha256() != metadata.sha256()
            || finalized.captured_byte_count() != metadata.canonical_length()
            || metadata.observed_length() != Some(finalized.observed_byte_count())
            || finalized.observed_byte_count() < finalized.captured_byte_count()
            || finalized.truncated()
                != (finalized.observed_byte_count() > finalized.captured_byte_count())
            || finalized.truncated() != metadata.truncated()
            || metadata.producing_work_id() != Some(expected_work_id)
            || metadata.producer() != expected_producer
            || !event_ids.insert(artifact.event.event_id)
            || indexed.insert(metadata.artifact_id(), artifact).is_some()
        {
            return Err(invalid());
        }
    }
    Ok(indexed)
}

fn require_exact_artifact_set(
    indexed: &HashMap<ArtifactId, &PreparedArtifact>,
    referenced: &HashSet<ArtifactId>,
    required_supplied: &HashSet<ArtifactId>,
) -> Result<(), SqliteAdapterError> {
    if indexed.keys().any(|id| !referenced.contains(id))
        || required_supplied.iter().any(|id| !indexed.contains_key(id))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_artifact_transaction_context(
    artifacts: &[PreparedArtifact],
    context: &WorkContext,
) -> Result<(), SqliteAdapterError> {
    if artifacts.iter().any(|artifact| {
        artifact.metadata.craxii_id() != context.craxii_id
            || artifact.event.correlation_id != context.correlation_id
    }) {
        Err(invalid())
    } else {
        Ok(())
    }
}

async fn require_existing_artifact(
    transaction: &mut WriteTransaction,
    artifact_id: ArtifactId,
    work_id: WorkId,
    sha256: crate::domain::Sha256Digest,
    captured_byte_count: Option<crate::domain::CanonicalByteCount>,
) -> Result<(), SqliteAdapterError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? AND producing_work_id = ? \
         AND sha256 = ? AND (? IS NULL OR captured_byte_count = ?)",
    )
    .bind(artifact_id.to_string())
    .bind(work_id.to_string())
    .bind(sha256.to_string())
    .bind(
        captured_byte_count
            .map(|value| i64::try_from(value.get()).map_err(|_| invalid()))
            .transpose()?,
    )
    .bind(
        captured_byte_count
            .map(|value| i64::try_from(value.get()).map_err(|_| invalid()))
            .transpose()?,
    )
    .fetch_one(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if count == 1 { Ok(()) } else { Err(invalid()) }
}

async fn insert_artifact_metadata(
    transaction: &mut WriteTransaction,
    artifact: &PreparedArtifact,
    expected_work_id: WorkId,
) -> Result<(), SqliteAdapterError> {
    let finalized = &artifact.finalized;
    let metadata = &artifact.metadata;
    let (producer_kind, producer_id) = match metadata.producer() {
        ArtifactProducer::None => return Err(invalid()),
        ArtifactProducer::Model(id) => ("model_invocation", id.to_string()),
        ArtifactProducer::Tool(id) => ("tool_execution", id.to_string()),
    };
    sqlx::query(
        "INSERT INTO artifacts (artifact_id, craxii_id, producing_work_id, producer_kind, \
         producer_id, backend, storage_key, sha256, captured_byte_count, observed_byte_count, \
         mime_type, encoding, logical_name, retention_class, truncated, compression, created_at) \
         VALUES (?, ?, ?, ?, ?, 'local', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(metadata.artifact_id().to_string())
    .bind(metadata.craxii_id().to_string())
    .bind(expected_work_id.to_string())
    .bind(producer_kind)
    .bind(producer_id)
    .bind(metadata.storage_key().as_str())
    .bind(metadata.sha256().to_string())
    .bind(i64::try_from(metadata.canonical_length().get()).map_err(|_| invalid())?)
    .bind(i64::try_from(finalized.observed_byte_count().get()).map_err(|_| invalid())?)
    .bind(metadata.mime_type().as_str())
    .bind(metadata.encoding().map(|value| value.as_str()))
    .bind(metadata.logical_name().map(|value| value.as_str()))
    .bind(retention_literal(metadata.retention()))
    .bind(i64::from(metadata.truncated()))
    .bind(metadata.compression().map(|value| value.as_str()))
    .bind(metadata.created_at().to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    Ok(())
}

async fn append_artifact_events(
    transaction: &mut WriteTransaction,
    context: &WorkContext,
    runtime: RuntimeInstanceId,
    artifacts: &[PreparedArtifact],
) -> Result<Option<(CommittedJournalPosition, CommittedJournalPosition)>, SqliteAdapterError> {
    let mut range = None;
    for artifact in artifacts {
        let position = append_work_event(
            transaction,
            context,
            Some(runtime),
            artifact.event,
            JournalEventPayload::ArtifactRecorded(ArtifactRecordedV1 {
                work_id: artifact.metadata.producing_work_id().ok_or_else(invalid)?,
                artifact_id: artifact.metadata.artifact_id(),
                sha256: artifact.metadata.sha256(),
                canonical_length: artifact.metadata.canonical_length().get(),
                retention: artifact.metadata.retention(),
                recorded_at: artifact.metadata.created_at(),
            }),
            artifact.metadata.created_at(),
        )
        .await?;
        range = Some(match range {
            None => (position, position),
            Some((first, _)) => (first, position),
        });
    }
    Ok(range)
}

fn source_kind(value: ContextSourceKind) -> &'static str {
    match value {
        ContextSourceKind::SystemInstruction => "system_instruction",
        ContextSourceKind::DeveloperInstruction => "developer_instruction",
        ContextSourceKind::WorkstationCapabilitySummary => "workstation_capability_summary",
        ContextSourceKind::WorkspaceIdentity => "workspace_identity",
        ContextSourceKind::ToolDefinition => "tool_definition",
        ContextSourceKind::UserMessage => "user_message",
        ContextSourceKind::ActiveTrigger => "active_trigger",
        ContextSourceKind::AssistantMessage => "assistant_message",
        ContextSourceKind::CompletedModelOutput => "completed_model_output",
        ContextSourceKind::ObservedToolResult => "observed_tool_result",
        ContextSourceKind::ArtifactContent => "artifact_content",
        ContextSourceKind::SyntheticFailure => "synthetic_failure",
        ContextSourceKind::SyntheticInterruption => "synthetic_interruption",
        ContextSourceKind::SyntheticOutcomeUnknown => "synthetic_outcome_unknown",
        ContextSourceKind::SyntheticDraftStatus => "synthetic_draft_status",
        ContextSourceKind::ProviderNativeContinuation => "provider_native_continuation",
    }
}

fn source_record_kind(value: ContextSourceRecordKind) -> &'static str {
    match value {
        ContextSourceRecordKind::InstructionVersion => "instruction_version",
        ContextSourceRecordKind::Workstation => "workstation",
        ContextSourceRecordKind::Workspace => "workspace",
        ContextSourceRecordKind::ToolDefinition => "tool_definition",
        ContextSourceRecordKind::Message => "message",
        ContextSourceRecordKind::ModelInvocation => "model_invocation",
        ContextSourceRecordKind::ToolExecution => "tool_execution",
        ContextSourceRecordKind::Work => "work",
    }
}

fn valid_bounded_record_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_lowercase() || byte.is_ascii_digit()
            } else {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-' | b':' | b'@')
            }
        })
}

fn valid_source_record_identifier(kind: &str, value: &str) -> bool {
    match kind {
        "instruction_version" | "tool_definition" => valid_bounded_record_literal(value),
        "workstation" => value.parse::<crate::domain::WorkstationId>().is_ok(),
        "workspace" => value.parse::<crate::domain::WorkspaceId>().is_ok(),
        "message" => value.parse::<crate::domain::MessageId>().is_ok(),
        "model_invocation" => value.parse::<crate::domain::ModelInvocationId>().is_ok(),
        "tool_execution" => value.parse::<crate::domain::ToolExecutionId>().is_ok(),
        "work" => value.parse::<crate::domain::WorkId>().is_ok(),
        _ => false,
    }
}

fn model_role(value: ContextModelRole) -> &'static str {
    match value {
        ContextModelRole::System => "system",
        ContextModelRole::Developer => "developer",
        ContextModelRole::User => "user",
        ContextModelRole::Assistant => "assistant",
        ContextModelRole::Tool => "tool",
    }
}

async fn insert_context_manifest(
    transaction: &mut WriteTransaction,
    manifest: &PreparedContextManifest,
) -> Result<(), SqliteAdapterError> {
    if manifest.sources.len() > i64::MAX as usize
        || manifest.assembler_version.is_empty()
        || manifest.assembler_version.len() > 64
        || manifest.context_policy_version.is_empty()
        || manifest.context_policy_version.len() > 64
        || manifest.token_estimator_id.is_empty()
        || manifest.token_estimator_id.len() > 64
        || manifest.estimated_input_tokens > 2_147_483_647
        || manifest.context_window_tokens == 0
        || manifest.context_window_tokens > 2_147_483_647
        || manifest.reserved_output_tokens > 2_147_483_647
        || manifest
            .estimated_input_tokens
            .saturating_add(manifest.reserved_output_tokens)
            > manifest.context_window_tokens
    {
        return Err(invalid());
    }
    let computed_basis_points = manifest
        .estimated_input_tokens
        .saturating_add(manifest.reserved_output_tokens)
        .saturating_mul(10_000)
        .saturating_add(manifest.context_window_tokens - 1)
        / manifest.context_window_tokens;
    if u64::from(manifest.utilization_basis_points) != computed_basis_points {
        return Err(invalid());
    }
    let mut event_ids = HashSet::new();
    if manifest
        .input_event_ids
        .iter()
        .any(|value| !event_ids.insert(*value))
    {
        return Err(invalid());
    }
    let capabilities = encode_model_capabilities(&manifest.provider_model)?;
    let cutoff = encode_eligibility_cutoff(manifest)?;
    let omissions = encode_omissions(manifest)?;
    sqlx::query(
        "INSERT INTO context_manifests (context_manifest_id, work_id, logical_invocation_id, \
         model_target_id, provider_id, provider_model_id, target_configuration_version, \
         model_capabilities_json, assembler_version, context_policy_version, \
         system_prompt_fingerprint, toolset_fingerprint, eligibility_cutoff_json, source_count, \
         canonical_byte_count, rendered_request_byte_count, estimated_input_tokens, \
         token_estimator_id, context_window_tokens, reserved_output_tokens, \
         utilization_basis_points, manifest_sha256, rendered_request_sha256, \
         rendered_request_artifact_id, omissions_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(manifest.context_manifest_id.to_string())
    .bind(manifest.work_id.to_string())
    .bind(manifest.logical_invocation_id.to_string())
    .bind(manifest.provider_model.model_target_id().as_str())
    .bind(manifest.provider_model.provider_id().as_str())
    .bind(manifest.provider_model.provider_model_id().as_str())
    .bind(manifest.provider_model.target_configuration_version().get())
    .bind(capabilities)
    .bind(&manifest.assembler_version)
    .bind(&manifest.context_policy_version)
    .bind(manifest.system_prompt_fingerprint.to_string())
    .bind(manifest.toolset_fingerprint.to_string())
    .bind(cutoff)
    .bind(i64::try_from(manifest.sources.len()).map_err(|_| invalid())?)
    .bind(i64::try_from(manifest.canonical_byte_count.get()).map_err(|_| invalid())?)
    .bind(i64::try_from(manifest.rendered_request_byte_count.get()).map_err(|_| invalid())?)
    .bind(i64::try_from(manifest.estimated_input_tokens).map_err(|_| invalid())?)
    .bind(&manifest.token_estimator_id)
    .bind(i64::try_from(manifest.context_window_tokens).map_err(|_| invalid())?)
    .bind(i64::try_from(manifest.reserved_output_tokens).map_err(|_| invalid())?)
    .bind(i64::from(manifest.utilization_basis_points))
    .bind(manifest.manifest_sha256.to_string())
    .bind(manifest.rendered_request_sha256.to_string())
    .bind(
        manifest
            .rendered_request_artifact_id
            .map(|value| value.to_string()),
    )
    .bind(omissions)
    .bind(manifest.created_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;

    for (index, source) in manifest.sources.iter().enumerate() {
        if source.position != i64::try_from(index + 1).map_err(|_| invalid())?
            || source.item_class.as_ref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > 64
                    || !value.bytes().enumerate().all(|(index, byte)| {
                        if index == 0 {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit()
                        } else {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'.' | b'_' | b'-')
                        }
                    })
            })
        {
            return Err(invalid());
        }
        let (event_id, artifact_id, record_kind, record_id) = match &source.identity {
            ContextSourceIdentity::Event(id) => (Some(id.to_string()), None, None, None),
            ContextSourceIdentity::Artifact(id) => (None, Some(id.to_string()), None, None),
            ContextSourceIdentity::Record { kind, id }
                if valid_source_record_identifier(source_record_kind(*kind), id) =>
            {
                (
                    None,
                    None,
                    Some(source_record_kind(*kind)),
                    Some(id.clone()),
                )
            }
            ContextSourceIdentity::Record { .. } => return Err(invalid()),
        };
        if (source.kind == ContextSourceKind::ArtifactContent && artifact_id.is_none())
            || (source.kind == ContextSourceKind::ProviderNativeContinuation
                && record_kind != Some("model_invocation"))
        {
            return Err(invalid());
        }
        sqlx::query(
            "INSERT INTO context_manifest_sources (context_manifest_id, position, source_kind, \
             event_id, artifact_id, source_record_kind, source_record_id, model_role, item_class, \
             source_content_sha256, rendered_byte_contribution, transform_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(manifest.context_manifest_id.to_string())
        .bind(source.position)
        .bind(source_kind(source.kind))
        .bind(event_id)
        .bind(artifact_id)
        .bind(record_kind)
        .bind(record_id)
        .bind(source.model_role.map(model_role))
        .bind(source.item_class.as_deref())
        .bind(source.source_content_sha256.to_string())
        .bind(i64::try_from(source.rendered_byte_contribution.get()).map_err(|_| invalid())?)
        .bind(encode_transform(source.transform, source.transformed)?)
        .execute(transaction.connection())
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    }
    Ok(())
}

async fn validate_retry_or_insert_manifest(
    transaction: &mut WriteTransaction,
    manifest: &PreparedContextManifest,
    attempt: &crate::domain::ModelAttemptReference,
) -> Result<(), SqliteAdapterError> {
    if manifest.work_id != attempt.work_id()
        || manifest.logical_invocation_id != attempt.logical_invocation_id()
        || manifest.context_manifest_id != attempt.context_manifest_id()
        || manifest.provider_model != *attempt.provider_model()
    {
        return Err(invalid());
    }
    if attempt.attempt_no().get() == 1 {
        if attempt.retry_of().is_some() {
            return Err(invalid());
        }
        crate::domain::ModelInvocationLifecycle::start(attempt.clone()).map_err(|_| invalid())?;
        return insert_context_manifest(transaction, manifest).await;
    }
    let predecessor = attempt.retry_of().ok_or_else(invalid)?;
    let row = sqlx::query(
        "SELECT logical_invocation_id, work_id, context_manifest_id, agent_step_no, attempt_no, \
         state FROM model_invocations WHERE model_invocation_id = ?",
    )
    .bind(predecessor.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    let prior_attempt: i64 = row.try_get("attempt_no")?;
    let prior_state: String = row.try_get("state")?;
    if row.try_get::<String, _>("logical_invocation_id")?
        != attempt.logical_invocation_id().to_string()
        || row.try_get::<String, _>("work_id")? != attempt.work_id().to_string()
        || row.try_get::<String, _>("context_manifest_id")?
            != attempt.context_manifest_id().to_string()
        || row.try_get::<i64, _>("agent_step_no")? != attempt.agent_step_no().get()
        || prior_attempt.checked_add(1) != Some(attempt.attempt_no().get())
        || !matches!(
            prior_state.as_str(),
            "completed" | "failed" | "cancelled_locally" | "provider_outcome_unknown"
        )
    {
        return Err(conflict());
    }
    let manifest_row = sqlx::query(
        "SELECT work_id, logical_invocation_id, model_target_id, provider_id, \
         provider_model_id, target_configuration_version, model_capabilities_json, \
         manifest_sha256, rendered_request_sha256, rendered_request_artifact_id, source_count \
         FROM context_manifests WHERE context_manifest_id = ?",
    )
    .bind(manifest.context_manifest_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(corrupt)?;
    if manifest_row.try_get::<String, _>("work_id")? != manifest.work_id.to_string()
        || manifest_row.try_get::<String, _>("logical_invocation_id")?
            != manifest.logical_invocation_id.to_string()
        || manifest_row.try_get::<String, _>("manifest_sha256")?
            != manifest.manifest_sha256.to_string()
        || manifest_row.try_get::<String, _>("model_target_id")?
            != manifest.provider_model.model_target_id().as_str()
        || manifest_row.try_get::<String, _>("provider_id")?
            != manifest.provider_model.provider_id().as_str()
        || manifest_row.try_get::<String, _>("provider_model_id")?
            != manifest.provider_model.provider_model_id().as_str()
        || manifest_row.try_get::<i64, _>("target_configuration_version")?
            != manifest.provider_model.target_configuration_version().get()
        || manifest_row.try_get::<String, _>("model_capabilities_json")?
            != encode_model_capabilities(&manifest.provider_model)?
        || manifest_row.try_get::<String, _>("rendered_request_sha256")?
            != manifest.rendered_request_sha256.to_string()
        || manifest_row.try_get::<Option<String>, _>("rendered_request_artifact_id")?
            != manifest
                .rendered_request_artifact_id
                .map(|value| value.to_string())
        || manifest_row.try_get::<i64, _>("source_count")?
            != i64::try_from(manifest.sources.len()).map_err(|_| invalid())?
    {
        return Err(corrupt());
    }
    Ok(())
}

fn require_supplied_artifact_facts(
    indexed: &HashMap<ArtifactId, &PreparedArtifact>,
    artifact_id: ArtifactId,
    sha256: crate::domain::Sha256Digest,
    captured: Option<crate::domain::CanonicalByteCount>,
    observed: Option<crate::domain::CanonicalByteCount>,
    truncated: Option<bool>,
) -> Result<(), SqliteAdapterError> {
    let artifact = indexed.get(&artifact_id).ok_or_else(invalid)?;
    if artifact.metadata.sha256() != sha256
        || captured.is_some_and(|value| artifact.finalized.captured_byte_count() != value)
        || observed.is_some_and(|value| artifact.finalized.observed_byte_count() != value)
        || truncated.is_some_and(|value| artifact.finalized.truncated() != value)
    {
        return Err(invalid());
    }
    Ok(())
}

fn privilege(value: PrivilegeMode) -> &'static str {
    match value {
        PrivilegeMode::User => "user",
        PrivilegeMode::Administrative => "administrative",
    }
}

async fn begin_model(
    store: &SqliteStateStore,
    request: BeginModelInvocationRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let attempt = &request.invocation.attempt;
    if request.expected_work.state != WorkState::Running
        || request.expected_work.current_attempt != CurrentWorkAttempt::None
        || request.work_next.state() != WorkState::WaitingOnModel
        || request.work_next.current_attempt()
            != CurrentWorkAttempt::Model(attempt.model_invocation_id())
        || request.work_next.runtime_owner() != Some(attempt.runtime_instance_id())
        || request.expected_work.runtime_owner != Some(attempt.runtime_instance_id())
        || request.expected_work.work_id != attempt.work_id()
        || request.work_event.causation_event_id != Some(request.invocation_event.event_id)
    {
        return Err(invalid());
    }
    if request.manifest.rendered_request_sha256 != request.invocation.request_sha256
        || (attempt.attempt_no().get() == 1
            && request.manifest.rendered_request_artifact_id
                != request.invocation.request_artifact_id)
    {
        return Err(invalid());
    }
    let indexed = index_validated_artifacts(
        &request.artifacts,
        attempt.work_id(),
        ArtifactProducer::Model(attempt.model_invocation_id()),
    )?;
    let mut referenced = HashSet::new();
    let mut required_supplied = HashSet::new();
    if let Some(id) = request.invocation.request_artifact_id {
        referenced.insert(id);
        required_supplied.insert(id);
        require_supplied_artifact_facts(
            &indexed,
            id,
            request.invocation.request_sha256,
            None,
            None,
            None,
        )?;
    }
    if let Some(id) = request.manifest.rendered_request_artifact_id {
        referenced.insert(id);
        if attempt.attempt_no().get() == 1 {
            required_supplied.insert(id);
        }
        if indexed.contains_key(&id) {
            require_supplied_artifact_facts(
                &indexed,
                id,
                request.manifest.rendered_request_sha256,
                Some(request.manifest.rendered_request_byte_count),
                None,
                None,
            )?;
        }
    }
    let primary_ids = referenced.clone();
    let mut source_ids = HashSet::new();
    for source in &request.manifest.sources {
        if let ContextSourceIdentity::Artifact(id) = &source.identity {
            let id = *id;
            if primary_ids.contains(&id) || !source_ids.insert(id) {
                return Err(invalid());
            }
            referenced.insert(id);
            if indexed.contains_key(&id) {
                require_supplied_artifact_facts(
                    &indexed,
                    id,
                    source.source_content_sha256,
                    None,
                    None,
                    None,
                )?;
            }
        }
    }
    require_exact_artifact_set(&indexed, &referenced, &required_supplied)?;
    let mut transaction = WriteTransaction::begin(&store.runtime, "begin_model_invocation").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    validate_artifact_transaction_context(&request.artifacts, &context)?;
    if let Some(id) = request.manifest.rendered_request_artifact_id
        && !indexed.contains_key(&id)
    {
        require_existing_artifact(
            &mut transaction,
            id,
            attempt.work_id(),
            request.manifest.rendered_request_sha256,
            Some(request.manifest.rendered_request_byte_count),
        )
        .await?;
    }
    for source in &request.manifest.sources {
        if let ContextSourceIdentity::Artifact(id) = &source.identity
            && !indexed.contains_key(id)
        {
            require_existing_artifact(
                &mut transaction,
                *id,
                attempt.work_id(),
                source.source_content_sha256,
                None,
            )
            .await?;
        }
    }
    for artifact in &request.artifacts {
        insert_artifact_metadata(&mut transaction, artifact, attempt.work_id()).await?;
    }
    validate_retry_or_insert_manifest(&mut transaction, &request.manifest, attempt).await?;
    let capabilities = encode_model_capabilities(attempt.provider_model())?;
    let selection_reason = match request.invocation.selection_reason {
        ModelSelectionReason::Explicit => "explicit",
        ModelSelectionReason::ConfiguredDefault => "configured_default",
    };
    sqlx::query(
        "INSERT INTO model_invocations (model_invocation_id, logical_invocation_id, work_id, \
         runtime_instance_id, context_manifest_id, agent_step_no, attempt_no, \
         retry_of_invocation_id, model_target_id, provider_id, provider_model_id, \
         target_configuration_version, model_capabilities_json, selection_reason, \
         required_capabilities_json, provider_options_json, state, request_sha256, \
         request_artifact_id, started_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'requesting', ?, ?, ?)",
    )
    .bind(attempt.model_invocation_id().to_string())
    .bind(attempt.logical_invocation_id().to_string())
    .bind(attempt.work_id().to_string())
    .bind(attempt.runtime_instance_id().to_string())
    .bind(attempt.context_manifest_id().to_string())
    .bind(attempt.agent_step_no().get())
    .bind(attempt.attempt_no().get())
    .bind(attempt.retry_of().map(|value| value.to_string()))
    .bind(attempt.provider_model().model_target_id().as_str())
    .bind(attempt.provider_model().provider_id().as_str())
    .bind(attempt.provider_model().provider_model_id().as_str())
    .bind(
        attempt
            .provider_model()
            .target_configuration_version()
            .get(),
    )
    .bind(capabilities)
    .bind(selection_reason)
    .bind(encode_required_capabilities(
        request.invocation.required_capabilities,
    )?)
    .bind(encode_provider_options(
        &request.invocation.provider_options,
    )?)
    .bind(request.invocation.request_sha256.to_string())
    .bind(
        request
            .invocation
            .request_artifact_id
            .map(|value| value.to_string()),
    )
    .bind(request.invocation.started_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;

    let expected = expected_snapshot(request.expected_work)?;
    guarded_work_update(
        &mut transaction,
        &expected,
        &request.work_next,
        WorkProjectionTimes {
            started_at: Some(context.started_at),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .map_err(map_projection_error)?;

    let artifact_range = append_artifact_events(
        &mut transaction,
        &context,
        attempt.runtime_instance_id(),
        &request.artifacts,
    )
    .await?;
    let model_position = append_work_event(
        &mut transaction,
        &context,
        Some(attempt.runtime_instance_id()),
        request.invocation_event,
        JournalEventPayload::ModelInvocationStarted(ModelInvocationEventV1 {
            work_id: attempt.work_id(),
            model_invocation_id: attempt.model_invocation_id(),
            logical_invocation_id: attempt.logical_invocation_id(),
            state: ModelInvocationState::Requesting,
            observed_at: request.invocation.started_at,
        }),
        request.invocation.started_at,
    )
    .await?;
    let work_position = append_work_event(
        &mut transaction,
        &context,
        Some(attempt.runtime_instance_id()),
        request.work_event,
        work_payload(&expected, &request.work_next, request.invocation.started_at)?,
        request.invocation.started_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.work_next.projection_version()),
        events: Some(CommittedEventRange {
            first: artifact_range.map_or(model_position.offset, |(first, _)| first.offset),
            last: work_position.offset,
        }),
    })
}

async fn mark_model_streaming(
    store: &SqliteStateStore,
    request: MarkModelStreamingRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    if request.expected_model.state != ModelInvocationState::Requesting
        || request.expected_work.state != WorkState::WaitingOnModel
        || request.expected_work.current_attempt
            != CurrentWorkAttempt::Model(request.expected_model.model_invocation_id)
        || request
            .observation
            .first_output_at
            .is_some_and(|value| value.to_string() < request.observation.first_byte_at.to_string())
    {
        return Err(invalid());
    }
    for value in [
        request.observation.provider_request_id.as_deref(),
        request.observation.provider_response_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.is_empty() || value.len() > 255 || value.trim() != value {
            return Err(invalid());
        }
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "mark_model_streaming").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    let row = sqlx::query(
        "SELECT logical_invocation_id, work_id, runtime_instance_id, started_at \
         FROM model_invocations WHERE model_invocation_id = ? AND state = 'requesting'",
    )
    .bind(request.expected_model.model_invocation_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    if row.try_get::<String, _>("work_id")? != request.expected_work.work_id.to_string()
        || row.try_get::<String, _>("runtime_instance_id")?
            != request
                .expected_work
                .runtime_owner
                .ok_or_else(invalid)?
                .to_string()
        || row.try_get::<String, _>("started_at")? > request.observation.first_byte_at.to_string()
    {
        return Err(conflict());
    }
    let logical_id: crate::domain::LogicalInvocationId = row
        .try_get::<String, _>("logical_invocation_id")?
        .parse()
        .map_err(|_| corrupt())?;
    let result = sqlx::query(
        "UPDATE model_invocations SET state = 'streaming', first_byte_at = ?, first_output_at = ?, \
         provider_request_id = ?, provider_response_id = ?, draft_exposed = ? \
         WHERE model_invocation_id = ? AND state = 'requesting' AND work_id = ? \
         AND runtime_instance_id = ?",
    )
    .bind(request.observation.first_byte_at.to_string())
    .bind(
        request
            .observation
            .first_output_at
            .map(|value| value.to_string()),
    )
    .bind(request.observation.provider_request_id.as_deref())
    .bind(request.observation.provider_response_id.as_deref())
    .bind(i64::from(request.observation.draft_exposed))
    .bind(request.expected_model.model_invocation_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(
        request
            .expected_work
            .runtime_owner
            .ok_or_else(invalid)?
            .to_string(),
    )
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    let position = append_work_event(
        &mut transaction,
        &context,
        request.expected_work.runtime_owner,
        request.event,
        JournalEventPayload::ModelInvocationStreaming(ModelInvocationEventV1 {
            work_id: request.expected_work.work_id,
            model_invocation_id: request.expected_model.model_invocation_id,
            logical_invocation_id: logical_id,
            state: ModelInvocationState::Streaming,
            observed_at: request.observation.first_byte_at,
        }),
        request.observation.first_byte_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.expected_work.version),
        events: Some(CommittedEventRange {
            first: position.offset,
            last: position.offset,
        }),
    })
}

fn validate_model_terminal(
    request: &FinishModelInvocationRequest,
) -> Result<(Option<String>, Option<[i64; 5]>), SqliteAdapterError> {
    let outcome = &request.outcome;
    if !outcome.state.is_terminal()
        || !is_legal_model_pair(request.expected_model.state, outcome.state)
        || request.expected_work.state != WorkState::WaitingOnModel
        || request.expected_work.current_attempt
            != CurrentWorkAttempt::Model(request.expected_model.model_invocation_id)
        || request.work_next.current_attempt() != CurrentWorkAttempt::None
        || request.work_event.causation_event_id != Some(request.model_event.event_id)
    {
        return Err(invalid());
    }
    for value in [
        outcome.provider_request_id.as_deref(),
        outcome.provider_response_id.as_deref(),
        outcome.stop_reason.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.is_empty() || value.len() > 255 || value.trim() != value {
            return Err(invalid());
        }
    }
    if outcome
        .stop_reason
        .as_ref()
        .is_some_and(|value| value.len() > 64)
        || outcome
            .tool_call_count
            .is_some_and(|value| value > i64::MAX as u64)
    {
        return Err(invalid());
    }
    let (output_json, usage) = match outcome.state {
        ModelInvocationState::Completed => {
            if outcome.response_sha256.is_none()
                || outcome.normalized_output.is_none()
                || outcome.first_byte_at.is_none()
                || outcome.first_output_at.is_none()
                || outcome.stop_reason.is_none()
                || outcome.tool_call_count.is_none()
                || outcome.normalized_error.is_some()
            {
                return Err(invalid());
            }
            let output = outcome.normalized_output.as_ref().ok_or_else(invalid)?;
            let tool_calls = u64::try_from(
                output
                    .items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item,
                            crate::ports::state_store::NormalizedModelOutputItem::ToolCall { .. }
                        )
                    })
                    .count(),
            )
            .map_err(|_| invalid())?;
            if outcome.tool_call_count != Some(tool_calls) {
                return Err(invalid());
            }
            (
                Some(encode_normalized_output(output)?),
                outcome.usage.map(encode_model_usage).transpose()?,
            )
        }
        ModelInvocationState::Failed
        | ModelInvocationState::CancelledLocally
        | ModelInvocationState::ProviderOutcomeUnknown => {
            if outcome.response_sha256.is_some()
                || outcome.response_artifact_id.is_some()
                || outcome.normalized_output.is_some()
                || outcome.usage.is_some()
                || outcome.stop_reason.is_some()
                || outcome.tool_call_count.is_some()
                || outcome.normalized_error.is_none()
            {
                return Err(invalid());
            }
            (None, None)
        }
        ModelInvocationState::Requesting | ModelInvocationState::Streaming => {
            return Err(invalid());
        }
    };
    let error = outcome
        .normalized_error
        .as_ref()
        .map(|value| {
            let unknown = outcome.state == ModelInvocationState::ProviderOutcomeUnknown;
            if (value.certainty().as_str() == "outcome_unknown") != unknown {
                return Err(invalid());
            }
            encode_attempt_error(value, unknown)
        })
        .transpose()?;
    Ok((output_json.or(error), usage))
}

async fn finish_model(
    store: &SqliteStateStore,
    request: FinishModelInvocationRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let (output_or_error, usage) = validate_model_terminal(&request)?;
    let (output_json, error_json) = if request.outcome.state == ModelInvocationState::Completed {
        (output_or_error, None)
    } else {
        (None, output_or_error)
    };
    let indexed = index_validated_artifacts(
        &request.artifacts,
        request.expected_work.work_id,
        ArtifactProducer::Model(request.expected_model.model_invocation_id),
    )?;
    let referenced = request
        .outcome
        .response_artifact_id
        .into_iter()
        .collect::<HashSet<_>>();
    require_exact_artifact_set(&indexed, &referenced, &referenced)?;
    if let Some(id) = request.outcome.response_artifact_id {
        require_supplied_artifact_facts(
            &indexed,
            id,
            request.outcome.response_sha256.ok_or_else(invalid)?,
            None,
            None,
            None,
        )?;
    }
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "finish_model_invocation").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    validate_artifact_transaction_context(&request.artifacts, &context)?;
    let identity = sqlx::query(
        "SELECT logical_invocation_id, runtime_instance_id, started_at, first_byte_at, \
         first_output_at FROM model_invocations WHERE model_invocation_id = ? AND work_id = ? \
         AND state = ?",
    )
    .bind(request.expected_model.model_invocation_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(request.expected_model.state.as_str())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    let runtime_id: RuntimeInstanceId = identity
        .try_get::<String, _>("runtime_instance_id")?
        .parse()
        .map_err(|_| corrupt())?;
    if request.expected_work.runtime_owner != Some(runtime_id) {
        return Err(conflict());
    }
    for artifact in &request.artifacts {
        insert_artifact_metadata(&mut transaction, artifact, request.expected_work.work_id).await?;
    }
    let first_byte = request
        .outcome
        .first_byte_at
        .map(|value| value.to_string())
        .or(identity.try_get::<Option<String>, _>("first_byte_at")?);
    let first_output = request
        .outcome
        .first_output_at
        .map(|value| value.to_string())
        .or(identity.try_get::<Option<String>, _>("first_output_at")?);
    let usage = usage.unwrap_or([0; 5]);
    let usage_present = request.outcome.usage.is_some();
    let result = sqlx::query(
        "UPDATE model_invocations SET state = ?, response_sha256 = ?, response_artifact_id = ?, \
         normalized_output_json = ?, provider_request_id = coalesce(?, provider_request_id), \
         provider_response_id = coalesce(?, provider_response_id), first_byte_at = ?, \
         first_output_at = ?, completed_at = ?, input_tokens = ?, cached_input_tokens = ?, \
         output_tokens = ?, reasoning_tokens = ?, total_tokens = ?, stop_reason = ?, \
         tool_call_count = ?, draft_exposed = ?, normalized_error_json = ? \
         WHERE model_invocation_id = ? AND work_id = ? AND runtime_instance_id = ? AND state = ?",
    )
    .bind(request.outcome.state.as_str())
    .bind(
        request
            .outcome
            .response_sha256
            .map(|value| value.to_string()),
    )
    .bind(
        request
            .outcome
            .response_artifact_id
            .map(|value| value.to_string()),
    )
    .bind(output_json)
    .bind(request.outcome.provider_request_id.as_deref())
    .bind(request.outcome.provider_response_id.as_deref())
    .bind(first_byte)
    .bind(first_output)
    .bind(request.outcome.completed_at.to_string())
    .bind(usage_present.then_some(usage[0]))
    .bind(usage_present.then_some(usage[1]))
    .bind(usage_present.then_some(usage[2]))
    .bind(usage_present.then_some(usage[3]))
    .bind(usage_present.then_some(usage[4]))
    .bind(request.outcome.stop_reason.as_deref())
    .bind(
        request
            .outcome
            .tool_call_count
            .map(|value| i64::try_from(value).expect("validated")),
    )
    .bind(i64::from(request.outcome.draft_exposed))
    .bind(error_json)
    .bind(request.expected_model.model_invocation_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(runtime_id.to_string())
    .bind(request.expected_model.state.as_str())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    let expected = expected_snapshot(request.expected_work)?;
    guarded_work_update(
        &mut transaction,
        &expected,
        &request.work_next,
        WorkProjectionTimes {
            started_at: Some(context.started_at),
            cancel_requested_at: None,
            terminal_at: request
                .work_next
                .state()
                .is_terminal()
                .then_some(request.outcome.completed_at),
        },
    )
    .await
    .map_err(map_projection_error)?;
    let artifact_range =
        append_artifact_events(&mut transaction, &context, runtime_id, &request.artifacts).await?;
    let logical_id: crate::domain::LogicalInvocationId = identity
        .try_get::<String, _>("logical_invocation_id")?
        .parse()
        .map_err(|_| corrupt())?;
    let model_payload = ModelInvocationEventV1 {
        work_id: request.expected_work.work_id,
        model_invocation_id: request.expected_model.model_invocation_id,
        logical_invocation_id: logical_id,
        state: request.outcome.state,
        observed_at: request.outcome.completed_at,
    };
    let payload = match request.outcome.state {
        ModelInvocationState::Completed => {
            JournalEventPayload::ModelInvocationCompleted(model_payload)
        }
        ModelInvocationState::Failed => JournalEventPayload::ModelInvocationFailed(model_payload),
        ModelInvocationState::CancelledLocally | ModelInvocationState::ProviderOutcomeUnknown => {
            JournalEventPayload::ModelInvocationInterrupted(model_payload)
        }
        _ => return Err(invalid()),
    };
    let model_position = append_work_event(
        &mut transaction,
        &context,
        Some(runtime_id),
        request.model_event,
        payload,
        request.outcome.completed_at,
    )
    .await?;
    let work_position = append_work_event(
        &mut transaction,
        &context,
        Some(runtime_id),
        request.work_event,
        work_payload(&expected, &request.work_next, request.outcome.completed_at)?,
        request.outcome.completed_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.work_next.projection_version()),
        events: Some(CommittedEventRange {
            first: artifact_range.map_or(model_position.offset, |(first, _)| first.offset),
            last: work_position.offset,
        }),
    })
}

fn canonical_arguments(json: &str) -> Result<(), SqliteAdapterError> {
    if json.len() > 65_536 {
        return Err(invalid());
    }
    let parsed: serde_json::Value = serde_json::from_str(json).map_err(|_| invalid())?;
    if !parsed.is_object() || serde_json::to_string(&parsed).map_err(|_| invalid())? != json {
        return Err(invalid());
    }
    Ok(())
}

async fn request_tool(
    store: &SqliteStateStore,
    request: RequestToolExecutionRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let tool = &request.tool;
    let lifecycle = tool.lifecycle;
    canonical_arguments(&tool.arguments_json)?;
    if crate::domain::Sha256Digest::hash_bytes(tool.arguments_json.as_bytes())
        != tool.arguments_sha256
        || request.expected_work.state != WorkState::Running
        || request.expected_work.current_attempt != CurrentWorkAttempt::None
        || request.expected_work.work_id != lifecycle.work_id()
        || request.expected_work.runtime_owner != Some(lifecycle.runtime_instance_id())
        || request.work_next.state() != WorkState::WaitingOnTool
        || request.work_next.current_attempt()
            != CurrentWorkAttempt::Tool(lifecycle.tool_execution_id())
        || request.work_event.causation_event_id != Some(request.tool_event.event_id)
        || tool.timeout_ms == 0
        || tool.timeout_ms > 900_000
        || tool.tool_schema_version <= 0
        || tool
            .provider_tool_call_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 255 || value.trim() != value)
    {
        return Err(invalid());
    }
    let mut transaction = WriteTransaction::begin(&store.runtime, "request_tool_execution").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    let source_state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM model_invocations WHERE model_invocation_id = ? AND work_id = ? \
         AND runtime_instance_id = ? AND agent_step_no = ?",
    )
    .bind(lifecycle.source_model_invocation_id().to_string())
    .bind(lifecycle.work_id().to_string())
    .bind(lifecycle.runtime_instance_id().to_string())
    .bind(lifecycle.agent_step_no().get())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if source_state.as_deref() != Some("completed") {
        return Err(conflict());
    }
    sqlx::query(
        "INSERT INTO tool_executions (tool_execution_id, execution_id, work_id, \
         source_model_invocation_id, runtime_instance_id, agent_step_no, tool_ordinal, \
         provider_tool_call_id, tool_name, tool_version, tool_schema_version, arguments_json, \
         arguments_sha256, workstation_id, workstation_generation, workspace_id, requested_cwd, \
         requested_privilege, timeout_ms, output_policy_json, state, requested_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'requested', ?)",
    )
    .bind(lifecycle.tool_execution_id().to_string())
    .bind(lifecycle.execution_id().to_string())
    .bind(lifecycle.work_id().to_string())
    .bind(lifecycle.source_model_invocation_id().to_string())
    .bind(lifecycle.runtime_instance_id().to_string())
    .bind(lifecycle.agent_step_no().get())
    .bind(lifecycle.tool_ordinal().get())
    .bind(tool.provider_tool_call_id.as_deref())
    .bind(tool.tool_name.as_str())
    .bind(tool.tool_version.as_str())
    .bind(tool.tool_schema_version)
    .bind(&tool.arguments_json)
    .bind(tool.arguments_sha256.to_string())
    .bind(tool.workstation_id.to_string())
    .bind(tool.workstation_generation.get())
    .bind(tool.workspace_id.to_string())
    .bind(tool.requested_cwd.canonical())
    .bind(privilege(tool.requested_privilege))
    .bind(i64::try_from(tool.timeout_ms).map_err(|_| invalid())?)
    .bind(encode_output_policy(tool.output_policy)?)
    .bind(tool.requested_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let expected = expected_snapshot(request.expected_work)?;
    guarded_work_update(
        &mut transaction,
        &expected,
        &request.work_next,
        WorkProjectionTimes {
            started_at: Some(context.started_at),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .map_err(map_projection_error)?;
    let tool_position = append_work_event(
        &mut transaction,
        &context,
        Some(lifecycle.runtime_instance_id()),
        request.tool_event,
        JournalEventPayload::ToolExecutionRequested(ToolExecutionEventV1 {
            work_id: lifecycle.work_id(),
            tool_execution_id: lifecycle.tool_execution_id(),
            state: ToolExecutionState::Requested,
            outcome_classification: None,
            observed_at: tool.requested_at,
        }),
        tool.requested_at,
    )
    .await?;
    let work_position = append_work_event(
        &mut transaction,
        &context,
        Some(lifecycle.runtime_instance_id()),
        request.work_event,
        work_payload(&expected, &request.work_next, tool.requested_at)?,
        tool.requested_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.work_next.projection_version()),
        events: Some(CommittedEventRange {
            first: tool_position.offset,
            last: work_position.offset,
        }),
    })
}

async fn dispatch_tool(
    store: &SqliteStateStore,
    request: CommitToolDispatchIntentRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    if request.expected_tool.state != ToolExecutionState::Requested
        || request.expected_work.state != WorkState::WaitingOnTool
        || request.expected_work.current_attempt
            != CurrentWorkAttempt::Tool(request.expected_tool.tool_execution_id)
        || request.dispatch.authority.decision() != AuthorityDecision::Allow
        || request.dispatch.authority.effective_privilege() != request.dispatch.effective_privilege
        || validate_dispatch_evidence(
            &request.dispatch.dispatch_evidence_json,
            &request.dispatch.authority,
            &request.dispatch.prepared_cwd,
        )
        .is_err()
        || request.dispatch.timeout_ms == 0
        || request.dispatch.timeout_ms > 900_000
    {
        return Err(invalid());
    }
    decide_tool_transition(
        ToolExecutionLifecycle::new(
            crate::domain::ToolLifecycleReference::new(
                request.expected_tool.tool_execution_id,
                crate::domain::ExecutionId::generate(),
                request.expected_work.work_id,
                request.expected_work.runtime_owner.ok_or_else(invalid)?,
                crate::domain::ModelInvocationId::generate(),
                crate::domain::AgentStepNo::try_new(1).map_err(|_| invalid())?,
                crate::domain::ToolOrdinal::try_new(1).map_err(|_| invalid())?,
            ),
            ToolExecutionState::Requested,
        ),
        crate::domain::ToolTransitionRequest::BeginDispatch,
    )
    .map_err(|_| invalid())?;
    let mut transaction =
        WriteTransaction::begin(&store.runtime, "commit_tool_dispatch_intent").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    let row = sqlx::query(
        "SELECT runtime_instance_id, workstation_id, workstation_generation, workspace_id, \
         requested_cwd, requested_at FROM tool_executions WHERE tool_execution_id = ? \
         AND work_id = ? AND state = 'requested'",
    )
    .bind(request.expected_tool.tool_execution_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    let runtime_id: RuntimeInstanceId = row
        .try_get::<String, _>("runtime_instance_id")?
        .parse()
        .map_err(|_| corrupt())?;
    if Some(runtime_id) != request.expected_work.runtime_owner
        || row.try_get::<String, _>("workstation_id")?
            != request
                .dispatch
                .prepared_cwd
                .resolved_cwd()
                .workstation_id()
                .to_string()
        || row.try_get::<i64, _>("workstation_generation")?
            != request
                .dispatch
                .prepared_cwd
                .resolved_cwd()
                .workstation_generation()
                .get()
        || row.try_get::<String, _>("workspace_id")?
            != request
                .dispatch
                .prepared_cwd
                .resolved_cwd()
                .workspace_id()
                .to_string()
        || row.try_get::<String, _>("requested_cwd")?
            != request
                .dispatch
                .prepared_cwd
                .resolved_cwd()
                .requested_path()
                .canonical()
        || row.try_get::<String, _>("requested_at")?
            > request.dispatch.dispatch_intent_at.to_string()
    {
        return Err(conflict());
    }
    let result = sqlx::query(
        "UPDATE tool_executions SET state = 'dispatching', resolved_cwd = ?, \
         effective_privilege = ?, authority_decision_json = ?, timeout_ms = ?, \
         output_policy_json = ?, dispatch_intent_at = ? WHERE tool_execution_id = ? \
         AND work_id = ? AND runtime_instance_id = ? AND state = 'requested'",
    )
    .bind(
        request
            .dispatch
            .prepared_cwd
            .resolved_cwd()
            .resolved_absolute_path(),
    )
    .bind(privilege(request.dispatch.effective_privilege))
    .bind(&request.dispatch.dispatch_evidence_json)
    .bind(i64::try_from(request.dispatch.timeout_ms).map_err(|_| invalid())?)
    .bind(encode_output_policy(request.dispatch.output_policy)?)
    .bind(request.dispatch.dispatch_intent_at.to_string())
    .bind(request.expected_tool.tool_execution_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(runtime_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    let position = append_work_event(
        &mut transaction,
        &context,
        Some(runtime_id),
        request.event,
        JournalEventPayload::ToolExecutionDispatching(ToolExecutionEventV1 {
            work_id: request.expected_work.work_id,
            tool_execution_id: request.expected_tool.tool_execution_id,
            state: ToolExecutionState::Dispatching,
            outcome_classification: None,
            observed_at: request.dispatch.dispatch_intent_at,
        }),
        request.dispatch.dispatch_intent_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.expected_work.version),
        events: Some(CommittedEventRange {
            first: position.offset,
            last: position.offset,
        }),
    })
}

fn validate_stream_counts(value: ToolStreamCounts) -> Result<[i64; 4], SqliteAdapterError> {
    if value.observed < value.captured
        || value.captured < value.returned_inline
        || value.omitted.get() != value.observed.get() - value.returned_inline.get()
    {
        return Err(invalid());
    }
    [
        value.observed,
        value.captured,
        value.returned_inline,
        value.omitted,
    ]
    .map(|value| i64::try_from(value.get()).map_err(|_| invalid()))
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?
    .try_into()
    .map_err(|_| invalid())
}

fn valid_tool_result_observation(
    result: crate::domain::ToolResultClass,
    dispatched: bool,
    started: bool,
    exit_code: Option<i64>,
    signal: Option<i64>,
    timed_out: Option<bool>,
    cancelled: Option<bool>,
) -> bool {
    if !dispatched {
        return !started
            && exit_code.is_none()
            && signal.is_none()
            && timed_out.is_none()
            && cancelled.is_none()
            && matches!(
                result,
                crate::domain::ToolResultClass::ValidationRejection
                    | crate::domain::ToolResultClass::UnknownTool
                    | crate::domain::ToolResultClass::AuthorityDenial
                    | crate::domain::ToolResultClass::FileError
                    | crate::domain::ToolResultClass::Cancellation
            );
    }
    match result {
        crate::domain::ToolResultClass::Success => {
            exit_code.is_none_or(|value| value == 0)
                && signal.is_none()
                && timed_out != Some(true)
                && cancelled != Some(true)
        }
        crate::domain::ToolResultClass::ProcessExit => {
            exit_code.is_some_and(|value| value != 0)
                && signal.is_none()
                && timed_out != Some(true)
                && cancelled != Some(true)
        }
        crate::domain::ToolResultClass::SignalTermination => {
            signal.is_some()
                && exit_code.is_none()
                && timed_out != Some(true)
                && cancelled != Some(true)
        }
        crate::domain::ToolResultClass::Timeout => {
            timed_out == Some(true) && cancelled != Some(true)
        }
        crate::domain::ToolResultClass::Cancellation => cancelled == Some(true),
        crate::domain::ToolResultClass::SpawnFailure => {
            !started && exit_code.is_none() && signal.is_none()
        }
        crate::domain::ToolResultClass::ValidationRejection
        | crate::domain::ToolResultClass::UnknownTool
        | crate::domain::ToolResultClass::AuthorityDenial
        | crate::domain::ToolResultClass::FileError
        | crate::domain::ToolResultClass::CleanupFailure => true,
    }
}

async fn finish_tool(
    store: &SqliteStateStore,
    request: FinishToolExecutionRequest,
) -> Result<CommitReceipt, SqliteAdapterError> {
    let outcome = &request.outcome;
    if !outcome.state.is_terminal()
        || !matches!(
            request.expected_work.state,
            WorkState::WaitingOnTool | WorkState::CancelRequested
        )
        || request.expected_work.current_attempt
            != CurrentWorkAttempt::Tool(request.expected_tool.tool_execution_id)
        || request.work_next.current_attempt() != CurrentWorkAttempt::None
        || request.work_event.causation_event_id != Some(request.tool_event.event_id)
        || (request.expected_work.state == WorkState::WaitingOnTool
            && !matches!(
                request.work_next.state(),
                WorkState::Running | WorkState::Interrupted
            ))
        || (request.expected_work.state == WorkState::CancelRequested
            && !matches!(
                request.work_next.state(),
                WorkState::Cancelled | WorkState::Interrupted
            ))
        || (matches!(
            outcome.state,
            ToolExecutionState::InterruptedBeforeDispatch | ToolExecutionState::OutcomeUnknown
        ) && request.work_next.state() != WorkState::Interrupted)
    {
        return Err(invalid());
    }
    let (result_json, error_json, classification) = match outcome.state {
        ToolExecutionState::Completed => {
            let evidence = outcome.result.as_ref().ok_or_else(invalid)?;
            if request.expected_tool.state == ToolExecutionState::Requested {
                if outcome.started_at.is_some()
                    || outcome.exit_code.is_some()
                    || outcome.signal.is_some()
                    || outcome.timed_out.is_some()
                    || outcome.cancelled.is_some()
                    || outcome.cleanup_confirmed.is_some()
                    || outcome.stdout_counts.is_some()
                    || outcome.stderr_counts.is_some()
                    || !outcome.evidence_artifact_ids.is_empty()
                    || outcome.stdout_artifact_id.is_some()
                    || outcome.stderr_artifact_id.is_some()
                    || outcome.truncated
                    || outcome
                        .predispatch_authority
                        .as_ref()
                        .is_some_and(|authority| {
                            authority.decision() != AuthorityDecision::Deny
                                || evidence.result_kind
                                    != crate::domain::ToolResultClass::AuthorityDenial
                        })
                {
                    return Err(invalid());
                }
            } else if outcome.predispatch_authority.is_some() {
                return Err(invalid());
            }
            if !valid_tool_result_observation(
                evidence.result_kind,
                request.expected_tool.state == ToolExecutionState::Dispatching,
                outcome.started_at.is_some(),
                outcome.exit_code,
                outcome.signal,
                outcome.timed_out,
                outcome.cancelled,
            ) {
                return Err(invalid());
            }
            let cleanup = match outcome.cleanup_confirmed {
                Some(true) => CleanupStatus::Confirmed,
                Some(false) => CleanupStatus::Unconfirmed,
                None => CleanupStatus::NotRequired,
            };
            decide_tool_transition(
                ToolExecutionLifecycle::new(
                    crate::domain::ToolLifecycleReference::new(
                        request.expected_tool.tool_execution_id,
                        crate::domain::ExecutionId::generate(),
                        request.expected_work.work_id,
                        request.expected_work.runtime_owner.ok_or_else(invalid)?,
                        crate::domain::ModelInvocationId::generate(),
                        crate::domain::AgentStepNo::try_new(1).map_err(|_| invalid())?,
                        crate::domain::ToolOrdinal::try_new(1).map_err(|_| invalid())?,
                    ),
                    request.expected_tool.state,
                ),
                crate::domain::ToolTransitionRequest::Complete {
                    result: evidence.result_kind,
                    cleanup_status: cleanup,
                },
            )
            .map_err(|_| invalid())?;
            let error = outcome
                .normalized_error
                .as_ref()
                .map(|value| encode_attempt_error(value, false))
                .transpose()?;
            (
                Some(encode_tool_result(evidence)?),
                error,
                Some(evidence.result_kind),
            )
        }
        ToolExecutionState::InterruptedBeforeDispatch => {
            if request.expected_tool.state != ToolExecutionState::Requested
                || outcome.predispatch_authority.is_some()
                || outcome.result.is_some()
                || outcome.stdout_counts.is_some()
                || outcome.stderr_counts.is_some()
                || !outcome.evidence_artifact_ids.is_empty()
                || outcome.stdout_artifact_id.is_some()
                || outcome.stderr_artifact_id.is_some()
                || outcome.normalized_error.is_none()
            {
                return Err(invalid());
            }
            decide_tool_transition(
                ToolExecutionLifecycle::new(
                    crate::domain::ToolLifecycleReference::new(
                        request.expected_tool.tool_execution_id,
                        crate::domain::ExecutionId::generate(),
                        request.expected_work.work_id,
                        request.expected_work.runtime_owner.ok_or_else(invalid)?,
                        crate::domain::ModelInvocationId::generate(),
                        crate::domain::AgentStepNo::try_new(1).map_err(|_| invalid())?,
                        crate::domain::ToolOrdinal::try_new(1).map_err(|_| invalid())?,
                    ),
                    request.expected_tool.state,
                ),
                crate::domain::ToolTransitionRequest::InterruptBeforeDispatch,
            )
            .map_err(|_| invalid())?;
            (
                None,
                Some(encode_attempt_error(
                    outcome.normalized_error.as_ref().ok_or_else(invalid)?,
                    false,
                )?),
                None,
            )
        }
        ToolExecutionState::OutcomeUnknown => {
            if request.expected_tool.state != ToolExecutionState::Dispatching
                || outcome.predispatch_authority.is_some()
                || outcome.cleanup_confirmed != Some(false)
                || outcome.result.is_some()
                || outcome.stdout_counts.is_some()
                || outcome.stderr_counts.is_some()
                || !outcome.evidence_artifact_ids.is_empty()
                || outcome.stdout_artifact_id.is_some()
                || outcome.stderr_artifact_id.is_some()
                || outcome
                    .normalized_error
                    .as_ref()
                    .is_none_or(|value| value.certainty().as_str() != "outcome_unknown")
                || request.work_next.state() != WorkState::Interrupted
            {
                return Err(invalid());
            }
            decide_tool_transition(
                ToolExecutionLifecycle::new(
                    crate::domain::ToolLifecycleReference::new(
                        request.expected_tool.tool_execution_id,
                        crate::domain::ExecutionId::generate(),
                        request.expected_work.work_id,
                        request.expected_work.runtime_owner.ok_or_else(invalid)?,
                        crate::domain::ModelInvocationId::generate(),
                        crate::domain::AgentStepNo::try_new(1).map_err(|_| invalid())?,
                        crate::domain::ToolOrdinal::try_new(1).map_err(|_| invalid())?,
                    ),
                    request.expected_tool.state,
                ),
                crate::domain::ToolTransitionRequest::MarkOutcomeUnknown,
            )
            .map_err(|_| invalid())?;
            (
                None,
                Some(encode_attempt_error(
                    outcome.normalized_error.as_ref().ok_or_else(invalid)?,
                    true,
                )?),
                None,
            )
        }
        ToolExecutionState::Requested | ToolExecutionState::Dispatching => return Err(invalid()),
    };
    let stdout = outcome
        .stdout_counts
        .map(validate_stream_counts)
        .transpose()?;
    let stderr = outcome
        .stderr_counts
        .map(validate_stream_counts)
        .transpose()?;
    let computed_truncated = stdout.is_some_and(|value| value[0] > value[1])
        || stderr.is_some_and(|value| value[0] > value[1]);
    if computed_truncated != outcome.truncated
        || outcome.stdout_artifact_id.is_some() != stdout.is_some_and(|value| value[1] > 0)
        || outcome.stderr_artifact_id.is_some() != stderr.is_some_and(|value| value[1] > 0)
    {
        return Err(invalid());
    }
    if outcome.stdout_artifact_id.is_some()
        && outcome.stdout_artifact_id == outcome.stderr_artifact_id
    {
        return Err(invalid());
    }
    let indexed = index_validated_artifacts(
        &request.artifacts,
        request.expected_work.work_id,
        ArtifactProducer::Tool(request.expected_tool.tool_execution_id),
    )?;
    let referenced = [outcome.stdout_artifact_id, outcome.stderr_artifact_id]
        .into_iter()
        .flatten()
        .chain(outcome.evidence_artifact_ids.iter().copied())
        .collect::<HashSet<_>>();
    if referenced.len()
        != outcome.evidence_artifact_ids.len()
            + usize::from(outcome.stdout_artifact_id.is_some())
            + usize::from(outcome.stderr_artifact_id.is_some())
    {
        return Err(invalid());
    }
    require_exact_artifact_set(&indexed, &referenced, &referenced)?;
    for (artifact_id, counts) in [
        (outcome.stdout_artifact_id, outcome.stdout_counts),
        (outcome.stderr_artifact_id, outcome.stderr_counts),
    ] {
        if let Some(id) = artifact_id {
            let counts = counts.ok_or_else(invalid)?;
            let artifact = indexed.get(&id).ok_or_else(invalid)?;
            require_supplied_artifact_facts(
                &indexed,
                id,
                artifact.metadata.sha256(),
                Some(counts.captured),
                Some(counts.observed),
                Some(counts.observed > counts.captured),
            )?;
        }
    }
    let predispatch_authority_json = outcome
        .predispatch_authority
        .as_ref()
        .map(encode_authority)
        .transpose()?;
    let mut transaction = WriteTransaction::begin(&store.runtime, "finish_tool_execution").await?;
    let context = load_work_context(&mut transaction, request.expected_work).await?;
    validate_artifact_transaction_context(&request.artifacts, &context)?;
    let row = sqlx::query(
        "SELECT runtime_instance_id, dispatch_intent_at FROM tool_executions \
         WHERE tool_execution_id = ? AND work_id = ? AND state = ?",
    )
    .bind(request.expected_tool.tool_execution_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(request.expected_tool.state.as_str())
    .fetch_optional(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?
    .ok_or_else(conflict)?;
    let runtime_id: RuntimeInstanceId = row
        .try_get::<String, _>("runtime_instance_id")?
        .parse()
        .map_err(|_| corrupt())?;
    if request.expected_work.runtime_owner != Some(runtime_id) {
        return Err(conflict());
    }
    for artifact in &request.artifacts {
        insert_artifact_metadata(&mut transaction, artifact, request.expected_work.work_id).await?;
    }
    let stdout = stdout.unwrap_or([0; 4]);
    let stderr = stderr.unwrap_or([0; 4]);
    let result = sqlx::query(
        "UPDATE tool_executions SET state = ?, authority_decision_json = \
         coalesce(?, authority_decision_json), started_at = ?, completed_at = ?, exit_code = ?, \
         signal = ?, timed_out = ?, cancelled = ?, cleanup_confirmed = ?, result_json = ?, \
         stdout_artifact_id = ?, stderr_artifact_id = ?, stdout_observed_bytes = ?, \
         stdout_captured_bytes = ?, stdout_returned_inline_bytes = ?, stdout_omitted_bytes = ?, \
         stderr_observed_bytes = ?, stderr_captured_bytes = ?, stderr_returned_inline_bytes = ?, \
         stderr_omitted_bytes = ?, truncated = ?, normalized_error_json = ? \
         WHERE tool_execution_id = ? AND work_id = ? AND runtime_instance_id = ? AND state = ?",
    )
    .bind(outcome.state.as_str())
    .bind(predispatch_authority_json)
    .bind(outcome.started_at.map(|value| value.to_string()))
    .bind(outcome.completed_at.to_string())
    .bind(outcome.exit_code)
    .bind(outcome.signal)
    .bind(outcome.timed_out.map(i64::from))
    .bind(outcome.cancelled.map(i64::from))
    .bind(outcome.cleanup_confirmed.map(i64::from))
    .bind(result_json)
    .bind(outcome.stdout_artifact_id.map(|value| value.to_string()))
    .bind(outcome.stderr_artifact_id.map(|value| value.to_string()))
    .bind(outcome.stdout_counts.is_some().then_some(stdout[0]))
    .bind(outcome.stdout_counts.is_some().then_some(stdout[1]))
    .bind(outcome.stdout_counts.is_some().then_some(stdout[2]))
    .bind(outcome.stdout_counts.is_some().then_some(stdout[3]))
    .bind(outcome.stderr_counts.is_some().then_some(stderr[0]))
    .bind(outcome.stderr_counts.is_some().then_some(stderr[1]))
    .bind(outcome.stderr_counts.is_some().then_some(stderr[2]))
    .bind(outcome.stderr_counts.is_some().then_some(stderr[3]))
    .bind(i64::from(outcome.truncated))
    .bind(error_json)
    .bind(request.expected_tool.tool_execution_id.to_string())
    .bind(request.expected_work.work_id.to_string())
    .bind(runtime_id.to_string())
    .bind(request.expected_tool.state.as_str())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if result.rows_affected() != 1 {
        return Err(conflict());
    }
    let expected = expected_snapshot(request.expected_work)?;
    guarded_work_update(
        &mut transaction,
        &expected,
        &request.work_next,
        WorkProjectionTimes {
            started_at: Some(context.started_at),
            cancel_requested_at: None,
            terminal_at: request
                .work_next
                .state()
                .is_terminal()
                .then_some(outcome.completed_at),
        },
    )
    .await
    .map_err(map_projection_error)?;
    let artifact_range =
        append_artifact_events(&mut transaction, &context, runtime_id, &request.artifacts).await?;
    let tool_fact = ToolExecutionEventV1 {
        work_id: request.expected_work.work_id,
        tool_execution_id: request.expected_tool.tool_execution_id,
        state: outcome.state,
        outcome_classification: classification,
        observed_at: outcome.completed_at,
    };
    let payload = match outcome.state {
        ToolExecutionState::Completed => JournalEventPayload::ToolExecutionCompleted(tool_fact),
        ToolExecutionState::InterruptedBeforeDispatch => {
            JournalEventPayload::ToolExecutionInterruptedBeforeDispatch(tool_fact)
        }
        ToolExecutionState::OutcomeUnknown => {
            JournalEventPayload::ToolExecutionOutcomeUnknown(tool_fact)
        }
        _ => return Err(invalid()),
    };
    let tool_position = append_work_event(
        &mut transaction,
        &context,
        Some(runtime_id),
        request.tool_event,
        payload,
        outcome.completed_at,
    )
    .await?;
    let work_position = append_work_event(
        &mut transaction,
        &context,
        Some(runtime_id),
        request.work_event,
        work_payload(&expected, &request.work_next, outcome.completed_at)?,
        outcome.completed_at,
    )
    .await?;
    transaction.commit().await?;
    Ok(CommitReceipt {
        committed_version: Some(request.work_next.projection_version()),
        events: Some(CommittedEventRange {
            first: artifact_range.map_or(tool_position.offset, |(first, _)| first.offset),
            last: work_position.offset,
        }),
    })
}

fn decode_model_state(value: &str) -> Result<ModelInvocationState, SqliteAdapterError> {
    match value {
        "requesting" => Ok(ModelInvocationState::Requesting),
        "streaming" => Ok(ModelInvocationState::Streaming),
        "completed" => Ok(ModelInvocationState::Completed),
        "failed" => Ok(ModelInvocationState::Failed),
        "cancelled_locally" => Ok(ModelInvocationState::CancelledLocally),
        "provider_outcome_unknown" => Ok(ModelInvocationState::ProviderOutcomeUnknown),
        _ => Err(corrupt()),
    }
}

fn decode_tool_state(value: &str) -> Result<ToolExecutionState, SqliteAdapterError> {
    match value {
        "requested" => Ok(ToolExecutionState::Requested),
        "dispatching" => Ok(ToolExecutionState::Dispatching),
        "completed" => Ok(ToolExecutionState::Completed),
        "interrupted_before_dispatch" => Ok(ToolExecutionState::InterruptedBeforeDispatch),
        "outcome_unknown" => Ok(ToolExecutionState::OutcomeUnknown),
        _ => Err(corrupt()),
    }
}

fn parse_uuid<T: std::str::FromStr>(value: &str) -> Result<T, SqliteAdapterError> {
    value.parse().map_err(|_| corrupt())
}

pub(super) async fn verify_stage8_consistency(
    connection: &mut sqlx::SqliteConnection,
    projected: &crate::application::projector::ProjectedState,
    events: &[JournalEvent],
) -> Result<u64, SqliteAdapterError> {
    let manifest_rows = sqlx::query("SELECT * FROM context_manifests")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    for row in &manifest_rows {
        let manifest_id: crate::domain::ContextManifestId =
            parse_uuid(&row.try_get::<String, _>("context_manifest_id")?)?;
        let manifest_work_id: WorkId = parse_uuid(&row.try_get::<String, _>("work_id")?)?;
        let _: crate::domain::LogicalInvocationId =
            parse_uuid(&row.try_get::<String, _>("logical_invocation_id")?)?;
        crate::domain::ModelTargetId::try_new(row.try_get::<String, _>("model_target_id")?)
            .map_err(|_| corrupt())?;
        crate::domain::ProviderId::try_new(row.try_get::<String, _>("provider_id")?)
            .map_err(|_| corrupt())?;
        crate::domain::ProviderModelId::try_new(row.try_get::<String, _>("provider_model_id")?)
            .map_err(|_| corrupt())?;
        crate::domain::TargetConfigurationVersion::try_new(
            row.try_get("target_configuration_version")?,
        )
        .map_err(|_| corrupt())?;
        decode_model_capabilities(&row.try_get::<String, _>("model_capabilities_json")?)?;
        let cutoff =
            validate_eligibility_cutoff(&row.try_get::<String, _>("eligibility_cutoff_json")?)?;
        validate_omissions(&row.try_get::<String, _>("omissions_json")?)?;
        crate::domain::Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("system_prompt_fingerprint")?,
        )
        .map_err(|_| corrupt())?;
        crate::domain::Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("toolset_fingerprint")?,
        )
        .map_err(|_| corrupt())?;
        crate::domain::Sha256Digest::parse_canonical(&row.try_get::<String, _>("manifest_sha256")?)
            .map_err(|_| corrupt())?;
        crate::domain::Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("rendered_request_sha256")?,
        )
        .map_err(|_| corrupt())?;
        UtcTimestamp::parse_canonical(&row.try_get::<String, _>("created_at")?)
            .map_err(|_| corrupt())?;
        let cutoff_work_coherent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_items WHERE work_id = ? AND conversation_id = ? \
             AND conversation_work_ordinal = ?",
        )
        .bind(manifest_work_id.to_string())
        .bind(cutoff.conversation_id.to_string())
        .bind(cutoff.active_work_ordinal)
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let cutoff_offset_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM journal_events WHERE journal_offset = ?")
                .bind(cutoff.maximum_journal_offset.get())
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
        if cutoff_work_coherent != 1 || cutoff_offset_exists != 1 {
            return Err(corrupt());
        }
        if let Some(prior) = cutoff.highest_prior_terminal_work_ordinal {
            let prior_exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM work_items WHERE conversation_id = ? \
                 AND conversation_work_ordinal = ? \
                 AND state IN ('completed', 'failed', 'cancelled', 'interrupted')",
            )
            .bind(cutoff.conversation_id.to_string())
            .bind(prior)
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if prior_exists != 1 {
                return Err(corrupt());
            }
        }
        for event_id in cutoff.input_event_ids {
            let input_coherent: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM work_item_inputs i \
                 JOIN work_items w ON w.work_id = i.work_id AND w.conversation_id = ? \
                 JOIN journal_events e ON e.event_id = i.input_event_id \
                    AND e.conversation_id = ? AND e.journal_offset <= ? \
                 WHERE i.input_event_id = ?",
            )
            .bind(cutoff.conversation_id.to_string())
            .bind(cutoff.conversation_id.to_string())
            .bind(cutoff.maximum_journal_offset.get())
            .bind(event_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if input_coherent != 1 {
                return Err(corrupt());
            }
        }
        if let Some(artifact_id) =
            row.try_get::<Option<String>, _>("rendered_request_artifact_id")?
        {
            let coherent: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM artifacts a JOIN model_invocations m \
                 ON m.context_manifest_id = ? AND m.attempt_no = 1 AND m.work_id = ? \
                 WHERE a.artifact_id = ? AND a.producing_work_id = ? AND a.sha256 = ? \
                 AND a.captured_byte_count = ? AND a.producer_kind = 'model_invocation' \
                 AND a.producer_id = m.model_invocation_id",
            )
            .bind(manifest_id.to_string())
            .bind(manifest_work_id.to_string())
            .bind(artifact_id)
            .bind(manifest_work_id.to_string())
            .bind(row.try_get::<String, _>("rendered_request_sha256")?)
            .bind(row.try_get::<i64, _>("rendered_request_byte_count")?)
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if coherent != 1 {
                return Err(corrupt());
            }
        }
        let sources = sqlx::query(
            "SELECT * FROM context_manifest_sources WHERE context_manifest_id = ? \
             ORDER BY position",
        )
        .bind(manifest_id.to_string())
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if i64::try_from(sources.len()).map_err(|_| corrupt())?
            != row.try_get::<i64, _>("source_count")?
        {
            return Err(corrupt());
        }
        for (index, source) in sources.iter().enumerate() {
            if source.try_get::<i64, _>("position")?
                != i64::try_from(index + 1).map_err(|_| corrupt())?
            {
                return Err(corrupt());
            }
            let source_kind: String = source.try_get("source_kind")?;
            if !matches!(
                source_kind.as_str(),
                "system_instruction"
                    | "developer_instruction"
                    | "workstation_capability_summary"
                    | "workspace_identity"
                    | "tool_definition"
                    | "user_message"
                    | "active_trigger"
                    | "assistant_message"
                    | "completed_model_output"
                    | "observed_tool_result"
                    | "artifact_content"
                    | "synthetic_failure"
                    | "synthetic_interruption"
                    | "synthetic_outcome_unknown"
                    | "synthetic_draft_status"
                    | "provider_native_continuation"
            ) {
                return Err(corrupt());
            }
            let event: Option<String> = source.try_get("event_id")?;
            let artifact: Option<String> = source.try_get("artifact_id")?;
            let record_kind: Option<String> = source.try_get("source_record_kind")?;
            let record_id: Option<String> = source.try_get("source_record_id")?;
            if usize::from(event.is_some())
                + usize::from(artifact.is_some())
                + usize::from(record_kind.is_some())
                != 1
                || record_kind.is_some() != record_id.is_some()
                || (source_kind == "artifact_content" && artifact.is_none())
                || (source_kind == "provider_native_continuation"
                    && record_kind.as_deref() != Some("model_invocation"))
            {
                return Err(corrupt());
            }
            if let (Some(kind), Some(id)) = (record_kind.as_deref(), record_id.as_deref())
                && !valid_source_record_identifier(kind, id)
            {
                return Err(corrupt());
            }
            if let Some(value) = event {
                let _: crate::domain::JournalEventId = parse_uuid(&value)?;
            }
            if let Some(value) = artifact {
                let _: ArtifactId = parse_uuid(&value)?;
                let coherent: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? AND sha256 = ? \
                     AND producing_work_id = ?",
                )
                .bind(value)
                .bind(source.try_get::<String, _>("source_content_sha256")?)
                .bind(manifest_work_id.to_string())
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
                if coherent != 1 {
                    return Err(corrupt());
                }
            }
            crate::domain::Sha256Digest::parse_canonical(
                &source.try_get::<String, _>("source_content_sha256")?,
            )
            .map_err(|_| corrupt())?;
            validate_transform(&source.try_get::<String, _>("transform_json")?)?;
        }
    }

    let artifact_rows = sqlx::query("SELECT * FROM artifacts")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if artifact_rows.len() != projected.artifacts.len() {
        return Err(corrupt());
    }
    let mut event_artifacts = std::collections::HashMap::new();
    for event in events {
        if let JournalEventPayload::ArtifactRecorded(payload) = &event.payload
            && event_artifacts
                .insert(payload.artifact_id, payload)
                .is_some()
        {
            return Err(corrupt());
        }
    }
    for row in &artifact_rows {
        let artifact_id: ArtifactId = parse_uuid(&row.try_get::<String, _>("artifact_id")?)?;
        let _: CraxiiId = parse_uuid(&row.try_get::<String, _>("craxii_id")?)?;
        let work_id: WorkId = parse_uuid(
            &row.try_get::<Option<String>, _>("producing_work_id")?
                .ok_or_else(corrupt)?,
        )?;
        let sha256 =
            crate::domain::Sha256Digest::parse_canonical(&row.try_get::<String, _>("sha256")?)
                .map_err(|_| corrupt())?;
        let stored_key: String = row.try_get("storage_key")?;
        let storage_key =
            ArtifactStorageKey::parse_canonical(&stored_key).map_err(|_| corrupt())?;
        if storage_key != ArtifactStorageKey::from_digest(sha256) {
            return Err(corrupt());
        }
        if row.try_get::<String, _>("backend")? != "local" {
            return Err(corrupt());
        }
        let captured = crate::domain::CanonicalByteCount::try_new(
            u64::try_from(row.try_get::<i64, _>("captured_byte_count")?).map_err(|_| corrupt())?,
        )
        .map_err(|_| corrupt())?;
        let observed = crate::domain::CanonicalByteCount::try_new(
            u64::try_from(
                row.try_get::<Option<i64>, _>("observed_byte_count")?
                    .ok_or_else(corrupt)?,
            )
            .map_err(|_| corrupt())?,
        )
        .map_err(|_| corrupt())?;
        let truncated = row.try_get::<i64, _>("truncated")? == 1;
        if observed < captured || truncated != (observed > captured) {
            return Err(corrupt());
        }
        crate::domain::ArtifactMimeType::try_new(row.try_get::<String, _>("mime_type")?)
            .map_err(|_| corrupt())?;
        if let Some(value) = row.try_get::<Option<String>, _>("encoding")? {
            crate::domain::ArtifactEncoding::try_new(value).map_err(|_| corrupt())?;
        }
        if let Some(value) = row.try_get::<Option<String>, _>("logical_name")? {
            crate::domain::ArtifactLogicalName::try_new(value).map_err(|_| corrupt())?;
        }
        if let Some(value) = row.try_get::<Option<String>, _>("compression")? {
            crate::domain::ArtifactCompression::try_new(value).map_err(|_| corrupt())?;
        }
        let payload = event_artifacts.get(&artifact_id).ok_or_else(corrupt)?;
        let retention = row.try_get::<String, _>("retention_class")?;
        if payload.work_id != work_id
            || payload.sha256 != sha256
            || payload.canonical_length
                != u64::try_from(row.try_get::<i64, _>("captured_byte_count")?)
                    .map_err(|_| corrupt())?
            || retention_literal(payload.retention) != retention
            || !projected.artifacts.contains(&artifact_id)
        {
            return Err(corrupt());
        }
        UtcTimestamp::parse_canonical(&row.try_get::<String, _>("created_at")?)
            .map_err(|_| corrupt())?;
        let producer_kind: String = row.try_get("producer_kind")?;
        let producer_id: String = row.try_get("producer_id")?;
        let producer_exists: i64 = match producer_kind.as_str() {
            "model_invocation" => {
                let _: crate::domain::ModelInvocationId = parse_uuid(&producer_id)?;
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM model_invocations WHERE model_invocation_id = ? \
                     AND work_id = ? AND (request_artifact_id = ? OR response_artifact_id = ? \
                     OR EXISTS (SELECT 1 FROM context_manifests WHERE work_id = ? \
                     AND rendered_request_artifact_id = ?))",
                )
                .bind(&producer_id)
                .bind(work_id.to_string())
                .bind(artifact_id.to_string())
                .bind(artifact_id.to_string())
                .bind(work_id.to_string())
                .bind(artifact_id.to_string())
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?
            }
            "tool_execution" => {
                let _: crate::domain::ToolExecutionId = parse_uuid(&producer_id)?;
                let producer = sqlx::query(
                    "SELECT stdout_artifact_id, stderr_artifact_id, result_json \
                     FROM tool_executions WHERE tool_execution_id = ? AND work_id = ?",
                )
                .bind(&producer_id)
                .bind(work_id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
                producer.map_or(Ok(0), |producer| {
                    let stdout: Option<String> = producer.try_get("stdout_artifact_id")?;
                    let stderr: Option<String> = producer.try_get("stderr_artifact_id")?;
                    let generic = producer
                        .try_get::<Option<String>, _>("result_json")?
                        .map(|json| decode_tool_result(&json))
                        .transpose()?
                        .is_some_and(|result| {
                            result.fields.iter().any(|(key, value)| {
                                key == "artifact_id" && value == &artifact_id.to_string()
                            })
                        });
                    Ok::<i64, SqliteAdapterError>(i64::from(
                        stdout.as_deref() == Some(artifact_id.to_string().as_str())
                            || stderr.as_deref() == Some(artifact_id.to_string().as_str())
                            || generic,
                    ))
                })?
            }
            _ => return Err(corrupt()),
        };
        if producer_exists != 1 {
            return Err(corrupt());
        }
    }

    let model_rows = sqlx::query("SELECT * FROM model_invocations")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if model_rows.len() != projected.models.len() {
        return Err(corrupt());
    }
    for row in &model_rows {
        let id: crate::domain::ModelInvocationId =
            parse_uuid(&row.try_get::<String, _>("model_invocation_id")?)?;
        let logical_id: crate::domain::LogicalInvocationId =
            parse_uuid(&row.try_get::<String, _>("logical_invocation_id")?)?;
        let work_id: WorkId = parse_uuid(&row.try_get::<String, _>("work_id")?)?;
        let _: RuntimeInstanceId = parse_uuid(&row.try_get::<String, _>("runtime_instance_id")?)?;
        let manifest_id: crate::domain::ContextManifestId =
            parse_uuid(&row.try_get::<String, _>("context_manifest_id")?)?;
        crate::domain::AgentStepNo::try_new(row.try_get("agent_step_no")?)
            .map_err(|_| corrupt())?;
        let attempt =
            crate::domain::AttemptNo::try_new(row.try_get("attempt_no")?).map_err(|_| corrupt())?;
        let retry: Option<String> = row.try_get("retry_of_invocation_id")?;
        if (attempt.get() == 1) != retry.is_none() {
            return Err(corrupt());
        }
        decode_model_capabilities(&row.try_get::<String, _>("model_capabilities_json")?)?;
        validate_required_capabilities(&row.try_get::<String, _>("required_capabilities_json")?)?;
        validate_provider_options(&row.try_get::<String, _>("provider_options_json")?)?;
        let state = decode_model_state(&row.try_get::<String, _>("state")?)?;
        let projected_model = projected.models.get(&id).ok_or_else(corrupt)?;
        if projected_model.fact.work_id != work_id
            || projected_model.fact.logical_invocation_id != logical_id
            || projected_model.fact.state != state
        {
            return Err(corrupt());
        }
        let manifest_coherent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM context_manifests WHERE context_manifest_id = ? \
             AND work_id = ? AND logical_invocation_id = ? AND model_target_id = ? \
             AND provider_id = ? AND provider_model_id = ? \
             AND target_configuration_version = ? AND model_capabilities_json = ?",
        )
        .bind(manifest_id.to_string())
        .bind(work_id.to_string())
        .bind(logical_id.to_string())
        .bind(row.try_get::<String, _>("model_target_id")?)
        .bind(row.try_get::<String, _>("provider_id")?)
        .bind(row.try_get::<String, _>("provider_model_id")?)
        .bind(row.try_get::<i64, _>("target_configuration_version")?)
        .bind(row.try_get::<String, _>("model_capabilities_json")?)
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if manifest_coherent != 1 {
            return Err(corrupt());
        }
        if let Some(json) = row.try_get::<Option<String>, _>("normalized_output_json")? {
            let output = validate_normalized_output(&json)?;
            let tool_calls = i64::try_from(
                output
                    .items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item,
                            crate::ports::state_store::NormalizedModelOutputItem::ToolCall { .. }
                        )
                    })
                    .count(),
            )
            .map_err(|_| corrupt())?;
            if row.try_get::<Option<i64>, _>("tool_call_count")? != Some(tool_calls) {
                return Err(corrupt());
            }
        }
        if let Some(json) = row.try_get::<Option<String>, _>("normalized_error_json")? {
            let decoded = validate_attempt_error(
                &json,
                state == ModelInvocationState::ProviderOutcomeUnknown,
            )?;
            if (decoded.certainty == "outcome_unknown")
                != (state == ModelInvocationState::ProviderOutcomeUnknown)
            {
                return Err(corrupt());
            }
        }
        for column in ["request_sha256", "response_sha256"] {
            if let Some(value) = row.try_get::<Option<String>, _>(column)? {
                crate::domain::Sha256Digest::parse_canonical(&value).map_err(|_| corrupt())?;
            }
        }
        for column in [
            "started_at",
            "first_byte_at",
            "first_output_at",
            "completed_at",
        ] {
            if let Some(value) = row.try_get::<Option<String>, _>(column)? {
                UtcTimestamp::parse_canonical(&value).map_err(|_| corrupt())?;
            }
        }
        let usage = [
            row.try_get::<Option<i64>, _>("input_tokens")?,
            row.try_get::<Option<i64>, _>("cached_input_tokens")?,
            row.try_get::<Option<i64>, _>("output_tokens")?,
            row.try_get::<Option<i64>, _>("reasoning_tokens")?,
            row.try_get::<Option<i64>, _>("total_tokens")?,
        ];
        if usage.iter().any(Option::is_some) {
            let [
                Some(input),
                Some(cached),
                Some(output),
                Some(reasoning),
                Some(total),
            ] = usage
            else {
                return Err(corrupt());
            };
            if cached > input
                || reasoning > output
                || total != input.checked_add(output).ok_or_else(corrupt)?
            {
                return Err(corrupt());
            }
        }
        if let Some(artifact_id) = row.try_get::<Option<String>, _>("request_artifact_id")? {
            let coherent: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? AND sha256 = ? \
                 AND producing_work_id = ? AND producer_kind = 'model_invocation' \
                 AND producer_id = ?",
            )
            .bind(artifact_id)
            .bind(row.try_get::<String, _>("request_sha256")?)
            .bind(work_id.to_string())
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if coherent != 1 {
                return Err(corrupt());
            }
        }
        if let Some(predecessor) = retry {
            let chain_ok: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM model_invocations prior WHERE prior.model_invocation_id = ? \
                 AND prior.logical_invocation_id = ? AND prior.work_id = ? \
                 AND prior.context_manifest_id = ? AND prior.agent_step_no = ? \
                 AND prior.attempt_no = ? AND prior.state IN \
                 ('completed', 'failed', 'cancelled_locally', 'provider_outcome_unknown')",
            )
            .bind(predecessor)
            .bind(logical_id.to_string())
            .bind(work_id.to_string())
            .bind(row.try_get::<String, _>("context_manifest_id")?)
            .bind(row.try_get::<i64, _>("agent_step_no")?)
            .bind(attempt.get() - 1)
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if chain_ok != 1 {
                return Err(corrupt());
            }
        }
        if let (Some(artifact_id), Some(response_sha)) = (
            row.try_get::<Option<String>, _>("response_artifact_id")?,
            row.try_get::<Option<String>, _>("response_sha256")?,
        ) {
            let coherent: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? AND sha256 = ? \
                 AND producing_work_id = ? AND producer_kind = 'model_invocation' \
                 AND producer_id = ?",
            )
            .bind(artifact_id)
            .bind(response_sha)
            .bind(work_id.to_string())
            .bind(id.to_string())
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
            if coherent != 1 {
                return Err(corrupt());
            }
        }
    }

    let tool_rows = sqlx::query("SELECT * FROM tool_executions")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if tool_rows.len() != projected.tools.len() {
        return Err(corrupt());
    }
    for row in &tool_rows {
        let id: crate::domain::ToolExecutionId =
            parse_uuid(&row.try_get::<String, _>("tool_execution_id")?)?;
        let work_id: WorkId = parse_uuid(&row.try_get::<String, _>("work_id")?)?;
        let execution_id: crate::domain::ExecutionId =
            parse_uuid(&row.try_get::<String, _>("execution_id")?)?;
        let source_model_id: crate::domain::ModelInvocationId =
            parse_uuid(&row.try_get::<String, _>("source_model_invocation_id")?)?;
        let runtime_id: RuntimeInstanceId =
            parse_uuid(&row.try_get::<String, _>("runtime_instance_id")?)?;
        let agent_step = crate::domain::AgentStepNo::try_new(row.try_get("agent_step_no")?)
            .map_err(|_| corrupt())?;
        let tool_ordinal = crate::domain::ToolOrdinal::try_new(row.try_get("tool_ordinal")?)
            .map_err(|_| corrupt())?;
        let tool_name = crate::domain::ToolName::try_new(row.try_get::<String, _>("tool_name")?)
            .map_err(|_| corrupt())?;
        crate::domain::ToolVersion::try_new(row.try_get::<String, _>("tool_version")?)
            .map_err(|_| corrupt())?;
        if row.try_get::<i64, _>("tool_schema_version")? <= 0 {
            return Err(corrupt());
        }
        let arguments: String = row.try_get("arguments_json")?;
        canonical_arguments(&arguments).map_err(|_| corrupt())?;
        if crate::domain::Sha256Digest::hash_bytes(arguments.as_bytes())
            != crate::domain::Sha256Digest::parse_canonical(
                &row.try_get::<String, _>("arguments_sha256")?,
            )
            .map_err(|_| corrupt())?
        {
            return Err(corrupt());
        }
        let requested_cwd: String = row.try_get("requested_cwd")?;
        let parsed_requested_cwd = if requested_cwd.starts_with('/') {
            crate::domain::LogicalPathReference::absolute(requested_cwd.clone())
        } else {
            crate::domain::LogicalPathReference::workspace_relative(requested_cwd.clone())
        }
        .map_err(|_| corrupt())?;
        if parsed_requested_cwd.canonical() != requested_cwd {
            return Err(corrupt());
        }
        if let Some(resolved_cwd) = row.try_get::<Option<String>, _>("resolved_cwd")? {
            let parsed = crate::domain::LogicalPathReference::absolute(resolved_cwd.clone())
                .map_err(|_| corrupt())?;
            if parsed.canonical() != resolved_cwd {
                return Err(corrupt());
            }
        }
        validate_output_policy(&row.try_get::<String, _>("output_policy_json")?)?;
        let state = decode_tool_state(&row.try_get::<String, _>("state")?)?;
        let projected_tool = projected.tools.get(&id).ok_or_else(corrupt)?;
        if projected_tool.fact.work_id != work_id || projected_tool.fact.state != state {
            return Err(corrupt());
        }
        let source_coherent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM model_invocations WHERE model_invocation_id = ? \
             AND work_id = ? AND runtime_instance_id = ? AND agent_step_no = ? \
             AND state = 'completed'",
        )
        .bind(source_model_id.to_string())
        .bind(work_id.to_string())
        .bind(runtime_id.to_string())
        .bind(agent_step.get())
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let topology_coherent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_items w \
             JOIN runtime_instances r ON r.runtime_instance_id = ? AND r.craxii_id = w.craxii_id \
             JOIN workstations s ON s.workstation_id = ? AND s.craxii_id = w.craxii_id \
                AND s.generation = ? AND r.workstation_id = s.workstation_id \
                AND r.workstation_generation = s.generation \
             JOIN workspaces p ON p.workspace_id = ? AND p.craxii_id = w.craxii_id \
                AND p.workstation_id = s.workstation_id \
             WHERE w.work_id = ?",
        )
        .bind(runtime_id.to_string())
        .bind(row.try_get::<String, _>("workstation_id")?)
        .bind(row.try_get::<i64, _>("workstation_generation")?)
        .bind(row.try_get::<String, _>("workspace_id")?)
        .bind(work_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if source_coherent != 1 || topology_coherent != 1 {
            return Err(corrupt());
        }
        if let Some(json) = row.try_get::<Option<String>, _>("authority_decision_json")? {
            let was_dispatched = row
                .try_get::<Option<String>, _>("dispatch_intent_at")?
                .is_some();
            validate_authority(
                &json,
                matches!(
                    state,
                    ToolExecutionState::Dispatching | ToolExecutionState::OutcomeUnknown
                ) || (state == ToolExecutionState::Completed && was_dispatched),
            )?;
        }
        let decoded_result = row
            .try_get::<Option<String>, _>("result_json")?
            .map(|json| decode_tool_result(&json))
            .transpose()?;
        if (state == ToolExecutionState::Completed) != decoded_result.is_some() {
            return Err(corrupt());
        }
        if let Some(result_class) = decoded_result.as_ref().map(|value| value.result_class) {
            let dispatched = row
                .try_get::<Option<String>, _>("dispatch_intent_at")?
                .is_some();
            let cleanup = match row.try_get::<Option<i64>, _>("cleanup_confirmed")? {
                Some(1) => CleanupStatus::Confirmed,
                Some(0) => CleanupStatus::Unconfirmed,
                None => CleanupStatus::NotRequired,
                _ => return Err(corrupt()),
            };
            let current_state = if dispatched {
                ToolExecutionState::Dispatching
            } else {
                ToolExecutionState::Requested
            };
            let decision = decide_tool_transition(
                ToolExecutionLifecycle::new(
                    crate::domain::ToolLifecycleReference::new(
                        id,
                        execution_id,
                        work_id,
                        runtime_id,
                        source_model_id,
                        agent_step,
                        tool_ordinal,
                    ),
                    current_state,
                ),
                crate::domain::ToolTransitionRequest::Complete {
                    result: result_class,
                    cleanup_status: cleanup,
                },
            )
            .map_err(|_| corrupt())?;
            let timed_out = row
                .try_get::<Option<i64>, _>("timed_out")?
                .map(|value| value != 0);
            let cancelled = row
                .try_get::<Option<i64>, _>("cancelled")?
                .map(|value| value != 0);
            if decision.next().state() != ToolExecutionState::Completed
                || !valid_tool_result_observation(
                    result_class,
                    dispatched,
                    row.try_get::<Option<String>, _>("started_at")?.is_some(),
                    row.try_get("exit_code")?,
                    row.try_get("signal")?,
                    timed_out,
                    cancelled,
                )
            {
                return Err(corrupt());
            }
        }
        if let Some(json) = row.try_get::<Option<String>, _>("normalized_error_json")? {
            let decoded =
                validate_attempt_error(&json, state == ToolExecutionState::OutcomeUnknown)?;
            if (decoded.certainty == "outcome_unknown")
                != (state == ToolExecutionState::OutcomeUnknown)
            {
                return Err(corrupt());
            }
        }
        if row
            .try_get::<Option<String>, _>("stdout_artifact_id")?
            .is_some()
            && row.try_get::<Option<String>, _>("stdout_artifact_id")?
                == row.try_get::<Option<String>, _>("stderr_artifact_id")?
        {
            return Err(corrupt());
        }
        for (column, captured_column, observed_column) in [
            (
                "stdout_artifact_id",
                "stdout_captured_bytes",
                "stdout_observed_bytes",
            ),
            (
                "stderr_artifact_id",
                "stderr_captured_bytes",
                "stderr_observed_bytes",
            ),
        ] {
            if let Some(artifact_id) = row.try_get::<Option<String>, _>(column)? {
                let coherent: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? \
                     AND producing_work_id = ? AND producer_kind = 'tool_execution' \
                     AND producer_id = ? AND captured_byte_count = ? \
                     AND observed_byte_count = ? \
                     AND truncated = (observed_byte_count > captured_byte_count) \
                     AND mime_type = 'application/octet-stream' AND encoding IS NULL \
                     AND logical_name = ?",
                )
                .bind(artifact_id)
                .bind(work_id.to_string())
                .bind(id.to_string())
                .bind(
                    row.try_get::<Option<i64>, _>(captured_column)?
                        .ok_or_else(corrupt)?,
                )
                .bind(
                    row.try_get::<Option<i64>, _>(observed_column)?
                        .ok_or_else(corrupt)?,
                )
                .bind(if column == "stdout_artifact_id" {
                    "stdout.bin"
                } else {
                    "stderr.bin"
                })
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
                if coherent != 1 {
                    return Err(corrupt());
                }
            }
        }

        let stdout_artifact = row.try_get::<Option<String>, _>("stdout_artifact_id")?;
        let stderr_artifact = row.try_get::<Option<String>, _>("stderr_artifact_id")?;
        let mut referenced_artifacts = HashSet::new();
        if let Some(id) = stdout_artifact.as_ref() {
            referenced_artifacts.insert(id.clone());
        }
        if let Some(id) = stderr_artifact.as_ref() {
            referenced_artifacts.insert(id.clone());
        }
        if let Some(result) = &decoded_result {
            let fields = result.fields.iter().cloned().collect::<HashMap<_, _>>();
            if let Some(generic_id) = fields.get("artifact_id") {
                if tool_name.as_str() != "read_file" {
                    return Err(corrupt());
                }
                let _: ArtifactId = parse_uuid(generic_id)?;
                if !referenced_artifacts.insert(generic_id.clone()) {
                    return Err(corrupt());
                }
                let expected_sha = fields.get("sha256").ok_or_else(corrupt)?;
                let expected_length = fields
                    .get("byte_length")
                    .ok_or_else(corrupt)?
                    .parse::<i64>()
                    .map_err(|_| corrupt())?;
                let coherent: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? \
                     AND producing_work_id = ? AND producer_kind = 'tool_execution' \
                     AND producer_id = ? AND sha256 = ? AND captured_byte_count = ? \
                     AND observed_byte_count = ? AND truncated = 0 \
                     AND mime_type = 'text/plain' AND encoding = 'utf-8' \
                     AND logical_name = 'read-file.txt'",
                )
                .bind(generic_id)
                .bind(work_id.to_string())
                .bind(id.to_string())
                .bind(expected_sha)
                .bind(expected_length)
                .bind(expected_length)
                .fetch_one(&mut *connection)
                .await
                .map_err(SqliteAdapterError::from_sqlx)?;
                if coherent != 1 {
                    return Err(corrupt());
                }
            }
        }
        let producer_artifact_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM artifacts WHERE producing_work_id = ? \
             AND producer_kind = 'tool_execution' AND producer_id = ?",
        )
        .bind(work_id.to_string())
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if producer_artifact_count != referenced_artifacts.len() as i64 {
            return Err(corrupt());
        }
    }

    let bad_work_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_items w \
         LEFT JOIN model_invocations m ON m.model_invocation_id = w.current_model_invocation_id \
         LEFT JOIN tool_executions t ON t.tool_execution_id = w.current_tool_execution_id \
         WHERE (w.state = 'waiting_on_model' AND (w.current_model_invocation_id IS NULL \
             OR w.current_tool_execution_id IS NOT NULL OR m.work_id <> w.work_id \
             OR m.runtime_instance_id <> w.runtime_instance_id OR m.state NOT IN ('requesting','streaming'))) \
            OR (w.state = 'waiting_on_tool' AND (w.current_tool_execution_id IS NULL \
             OR w.current_model_invocation_id IS NOT NULL OR t.work_id <> w.work_id \
             OR t.runtime_instance_id <> w.runtime_instance_id OR t.state NOT IN ('requested','dispatching'))) \
            OR (w.state = 'cancel_requested' AND w.current_model_invocation_id IS NOT NULL \
             AND (w.current_tool_execution_id IS NOT NULL OR m.work_id <> w.work_id \
             OR m.runtime_instance_id <> w.runtime_instance_id)) \
            OR (w.state = 'cancel_requested' AND w.current_tool_execution_id IS NOT NULL \
             AND (w.current_model_invocation_id IS NOT NULL OR t.work_id <> w.work_id \
             OR t.runtime_instance_id <> w.runtime_instance_id)) \
            OR (w.state NOT IN ('waiting_on_model','waiting_on_tool','cancel_requested') \
             AND (w.current_model_invocation_id IS NOT NULL OR w.current_tool_execution_id IS NOT NULL))",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let detached_model: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_invocations m LEFT JOIN work_items w \
         ON w.current_model_invocation_id = m.model_invocation_id \
         WHERE m.state IN ('requesting','streaming') AND w.work_id IS NULL",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    let detached_tool: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tool_executions t LEFT JOIN work_items w \
         ON w.current_tool_execution_id = t.tool_execution_id \
         WHERE t.state IN ('requested','dispatching') AND w.work_id IS NULL",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    if bad_work_links != 0 || detached_model != 0 || detached_tool != 0 {
        return Err(corrupt());
    }
    Ok(12)
}

impl SqliteStateStore {
    /// Loads logical descriptors only; no physical path escapes the artifact adapter.
    pub(crate) async fn load_referenced_artifacts(
        &self,
    ) -> Result<Vec<crate::ports::artifact_store::ArtifactObjectReference>, SqliteAdapterError>
    {
        let mut connection = self.runtime.acquire().await?;
        let inconsistent_groups: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM (SELECT storage_key FROM artifacts GROUP BY storage_key \
             HAVING COUNT(DISTINCT sha256) <> 1 OR COUNT(DISTINCT captured_byte_count) <> 1)",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        if inconsistent_groups != 0 {
            return Err(corrupt());
        }
        let rows = sqlx::query(
            "SELECT MIN(artifact_id) AS artifact_id, storage_key, sha256, captured_byte_count \
             FROM artifacts GROUP BY storage_key, sha256, captured_byte_count ORDER BY storage_key",
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
        let mut artifacts = Vec::with_capacity(rows.len());
        for row in rows {
            let _: ArtifactId = parse_uuid(&row.try_get::<String, _>("artifact_id")?)?;
            let sha256 =
                crate::domain::Sha256Digest::parse_canonical(&row.try_get::<String, _>("sha256")?)
                    .map_err(|_| corrupt())?;
            let key_text: String = row.try_get("storage_key")?;
            let storage_key =
                ArtifactStorageKey::parse_canonical(&key_text).map_err(|_| corrupt())?;
            let captured = crate::domain::CanonicalByteCount::try_new(
                u64::try_from(row.try_get::<i64, _>("captured_byte_count")?)
                    .map_err(|_| corrupt())?,
            )
            .map_err(|_| corrupt())?;
            artifacts.push(
                crate::ports::artifact_store::ArtifactObjectReference::from_persisted_metadata(
                    storage_key,
                    sha256,
                    captured,
                ),
            );
        }
        Ok(artifacts)
    }
}

impl ModelStateStore for SqliteStateStore {
    fn begin_model_invocation(
        &self,
        request: BeginModelInvocationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move { begin_model(self, request).await.map_err(map_port_error) })
    }

    fn mark_model_streaming(
        &self,
        request: MarkModelStreamingRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move {
            mark_model_streaming(self, request)
                .await
                .map_err(map_port_error)
        })
    }

    fn finish_model_invocation(
        &self,
        request: FinishModelInvocationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move { finish_model(self, request).await.map_err(map_port_error) })
    }
}

impl ToolStateStore for SqliteStateStore {
    fn request_tool_execution(
        &self,
        request: RequestToolExecutionRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move { request_tool(self, request).await.map_err(map_port_error) })
    }

    fn commit_tool_dispatch_intent(
        &self,
        request: CommitToolDispatchIntentRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move { dispatch_tool(self, request).await.map_err(map_port_error) })
    }

    fn finish_tool_execution(
        &self,
        request: FinishToolExecutionRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        Box::pin(async move { finish_tool(self, request).await.map_err(map_port_error) })
    }
}
