use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::domain::{
    AgentStepNo, ArtifactId, ArtifactStorageKey, CanonicalByteCount, ConversationId,
    ConversationWorkOrdinal, JournalEventPayload, JournalOffset, ModelInvocationId,
    ModelInvocationState, ModelTargetId, ProviderId, ProviderModelId, ProviderModelReference,
    Sha256Digest, TargetConfigurationVersion, ToolExecutionId, ToolExecutionState, ToolName,
    ToolOrdinal, WorkId, WorkspaceId, WorkstationId,
};
use crate::ports::context_source_store::{
    ContextArtifactDescriptor, ContextAssistantMessageSource, ContextContinuationBoundary,
    ContextEligibilityRequest, ContextEligibilitySnapshot, ContextMessageSource,
    ContextModelOutputSource, ContextReconstructionRequest, ContextReconstructionSnapshot,
    ContextReloadedMessageSource, ContextReloadedSource, ContextSourceStore,
    ContextSourceStoreError, ContextSourceStoreErrorKind, ContextSourceStoreFuture,
    ContextStreamCounts, ContextToolResultSource, ContextWorkSource, ContextWorkspaceSource,
    ContextWorkstationSource,
};
use crate::ports::state_store::{
    ContextSourceIdentity, ContextSourceKind, ContextSourceRecordKind, PreparedContextSource,
};

use super::codec::{decode_message_row, decode_work_state};
use super::journal::decode_event_row;
use super::stage8_codec::{decode_model_capabilities, validate_normalized_output};
use super::state_store::SqliteStateStore;

fn store_error(kind: ContextSourceStoreErrorKind) -> ContextSourceStoreError {
    ContextSourceStoreError::new(kind)
}

fn corrupt() -> ContextSourceStoreError {
    store_error(ContextSourceStoreErrorKind::CorruptSource)
}

fn missing() -> ContextSourceStoreError {
    store_error(ContextSourceStoreErrorKind::MissingSource)
}

fn storage(_: sqlx::Error) -> ContextSourceStoreError {
    store_error(ContextSourceStoreErrorKind::Storage)
}

fn parse_id<T>(value: &str) -> Result<T, ContextSourceStoreError>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| corrupt())
}

fn positive_ordinal(value: i64) -> Result<ConversationWorkOrdinal, ContextSourceStoreError> {
    ConversationWorkOrdinal::try_new(value).map_err(|_| corrupt())
}

fn journal_offset(value: i64) -> Result<JournalOffset, ContextSourceStoreError> {
    JournalOffset::try_new(value).map_err(|_| corrupt())
}

fn byte_count(value: i64) -> Result<CanonicalByteCount, ContextSourceStoreError> {
    u64::try_from(value)
        .ok()
        .and_then(|value| CanonicalByteCount::try_new(value).ok())
        .ok_or_else(corrupt)
}

fn canonical_json_hash(value: &Value) -> Sha256Digest {
    let bytes = serde_json::to_vec(value).expect("semantic JSON must serialize");
    Sha256Digest::hash_bytes(&bytes)
}

impl SqliteStateStore {
    async fn load_context_snapshot(
        &self,
        request: ContextEligibilityRequest,
    ) -> Result<ContextEligibilitySnapshot, ContextSourceStoreError> {
        let mut transaction = self.runtime.inner.pool.begin().await.map_err(storage)?;

        // This first read establishes the SQLite snapshot used by every following eligibility read.
        let maximum_journal_offset =
            sqlx::query_scalar::<_, Option<i64>>("SELECT max(journal_offset) FROM journal_events")
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage)?
                .ok_or_else(missing)
                .and_then(journal_offset)?;

        #[cfg(test)]
        self.fire_stage16_snapshot_hook()?;

        let active_work = load_active_work(&mut transaction, request.work_id).await?;
        let conversation_id = active_work.conversation_id;
        let active_ordinal = active_work.ordinal;

        let active_trigger = load_exact_trigger(
            &mut transaction,
            request.work_id,
            conversation_id,
            active_ordinal,
        )
        .await?;
        let prior_works =
            load_prior_works(&mut transaction, conversation_id, active_ordinal).await?;
        let prior_messages =
            load_prior_messages(&mut transaction, conversation_id, active_ordinal).await?;
        let prior_final_assistant_messages =
            load_prior_assistant_messages(&mut transaction, conversation_id, active_ordinal)
                .await?;
        let completed_model_outputs =
            load_completed_model_outputs(&mut transaction, conversation_id, active_ordinal).await?;
        let observed_tool_results =
            load_tool_results(&mut transaction, conversation_id, active_ordinal).await?;
        let continuation_boundaries =
            load_continuation_boundaries(&mut transaction, conversation_id, active_ordinal).await?;
        let (workstation, workspace) =
            load_capability_and_workspace(&mut transaction, &active_work).await?;

        let highest_prior_terminal_work_ordinal = prior_works
            .iter()
            .rev()
            .find(|work| work.state.is_terminal())
            .map(|work| work.ordinal);
        let mut exact_input_event_ids = prior_messages
            .iter()
            .map(|source| source.input_event_id)
            .collect::<Vec<_>>();
        exact_input_event_ids.push(active_trigger.input_event_id);
        let active_output_record_ids = completed_model_outputs
            .iter()
            .filter(|source| source.work_id == request.work_id)
            .map(|source| source.model_invocation_id.to_string())
            .chain(
                observed_tool_results
                    .iter()
                    .filter(|source| source.work_id == request.work_id)
                    .map(|source| source.tool_execution_id.to_string()),
            )
            .collect();

        transaction.commit().await.map_err(storage)?;
        Ok(ContextEligibilitySnapshot {
            active_work,
            active_trigger,
            prior_works,
            prior_messages,
            prior_final_assistant_messages,
            completed_model_outputs,
            observed_tool_results,
            continuation_boundaries,
            workstation,
            workspace,
            highest_prior_terminal_work_ordinal,
            maximum_journal_offset,
            exact_input_event_ids,
            active_output_record_ids,
        })
    }

    async fn reload_exact_context_sources(
        &self,
        request: ContextReconstructionRequest,
    ) -> Result<ContextReconstructionSnapshot, ContextSourceStoreError> {
        if request.manifest.sources.as_slice() != request.ordered_sources.as_ref()
            || request
                .ordered_sources
                .iter()
                .enumerate()
                .any(|(index, source)| source.position != index as i64 + 1)
        {
            return Err(corrupt());
        }
        let mut transaction = self.runtime.inner.pool.begin().await.map_err(storage)?;
        let active_work = load_active_work(&mut transaction, request.manifest.work_id).await?;
        if active_work.conversation_id != request.manifest.eligibility_conversation_id
            || active_work.ordinal.get() != request.manifest.active_work_ordinal
        {
            return Err(store_error(ContextSourceStoreErrorKind::InvalidOwnership));
        }

        let mut ordered_sources = Vec::with_capacity(request.ordered_sources.len());
        for source in request.ordered_sources.iter() {
            ordered_sources
                .push(reload_exact_source(&mut transaction, &request.manifest, source).await?);
        }
        transaction.commit().await.map_err(storage)?;
        Ok(ContextReconstructionSnapshot {
            active_work,
            ordered_sources,
        })
    }

    #[cfg(test)]
    pub(super) fn set_stage16_snapshot_hook(&self, hook: Option<Stage16SnapshotTestHook>) {
        *self.stage16_snapshot_hook.lock().unwrap() = hook;
    }

    #[cfg(test)]
    fn fire_stage16_snapshot_hook(&self) -> Result<(), ContextSourceStoreError> {
        if let Some(hook) = self.stage16_snapshot_hook.lock().unwrap().take() {
            (hook.0)();
        }
        Ok(())
    }
}

impl ContextSourceStore for SqliteStateStore {
    fn load_context_eligibility_snapshot(
        &self,
        request: ContextEligibilityRequest,
    ) -> ContextSourceStoreFuture<'_, ContextEligibilitySnapshot> {
        Box::pin(async move { self.load_context_snapshot(request).await })
    }

    fn reload_context_sources(
        &self,
        request: ContextReconstructionRequest,
    ) -> ContextSourceStoreFuture<'_, ContextReconstructionSnapshot> {
        Box::pin(async move { self.reload_exact_context_sources(request).await })
    }
}

#[cfg(test)]
pub(super) struct Stage16SnapshotTestHook(Box<dyn FnOnce() + Send + 'static>);

#[cfg(test)]
impl Stage16SnapshotTestHook {
    pub(super) fn new(hook: impl FnOnce() + Send + 'static) -> Self {
        Self(Box::new(hook))
    }
}

#[cfg(test)]
impl std::fmt::Debug for Stage16SnapshotTestHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Stage16SnapshotTestHook { .. }")
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use crate::application::command_service::{AcceptMessageCommand, CommandService};
    use crate::application::context_assembler::{
        ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot,
    };
    use crate::application::device_provisioning::DeviceProvisioningService;
    use crate::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
    use crate::application::tool_registry::{ToolRegistry, ToolSemanticPolicy};
    use crate::domain::{
        AuthenticatedDevice, BearerToken, ClientMessageId, ContentBlock, ConversationId,
        CorrelationId, CraxiiId, DeviceDisplayName, IdempotencyKey, JournalEventId, MessageContent,
        ModelCapabilitySnapshot, ModelCapabilitySnapshotInput, ModelConfigReference, ModelTarget,
        ModelTargetId, ModelTargetInput, ProviderId, ProviderModelId, ProviderModelReference,
        ProviderNativeOptions, TargetConfigurationVersion, TokenCount, TokenEstimatorIdentity,
        UtcTimestamp, WorkspaceId, WorkstationGeneration, WorkstationId,
    };
    use crate::ports::clock::TestClock;
    use crate::ports::model_provider::{
        ConservativeTokenEstimate, ProviderError, TokenEstimateUnit, TokenEstimator,
    };
    use crate::ports::state_store::{
        BootstrapObservation, BootstrapStateStore, ExecutionCapabilityObservation,
        LoadOrBootstrapIdentityRequest, V0IdentityReference,
    };

    use super::*;
    use crate::adapters::sqlite::runtime::SqliteRuntimeGuard;

    const T0: &str = "2026-08-31T01:02:03.000001Z";
    const T1: &str = "2026-08-31T01:02:04.000001Z";
    const TOKEN: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "craxii-stage16-context-test-{}-{}",
                std::process::id(),
                uuid::Uuid::now_v7()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        _root: TestRoot,
        guard: SqliteRuntimeGuard,
        store: SqliteStateStore,
        identity: V0IdentityReference,
        device_id: crate::domain::DeviceId,
    }

    fn timestamp(value: &str) -> UtcTimestamp {
        value.parse().unwrap()
    }

    async fn fixture() -> Fixture {
        let root = TestRoot::new();
        let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
        let store = SqliteStateStore::new(guard.runtime().clone());
        let identity = store
            .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
                proposed: V0IdentityReference {
                    craxii_id: CraxiiId::generate(),
                    conversation_id: ConversationId::generate(),
                    workstation_id: WorkstationId::generate(),
                    workspace_id: WorkspaceId::generate(),
                },
                initialized_event_id: JournalEventId::generate(),
                conversation_created_event_id: JournalEventId::generate(),
                correlation_id: CorrelationId::generate(),
                created_at: timestamp(T0),
                observation: BootstrapObservation {
                    initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                    architecture: "aarch64".to_owned(),
                    os_release: "stage16-test".to_owned(),
                    default_shell: "/bin/zsh".to_owned(),
                    workspace_logical_name: "primary".to_owned(),
                    workspace_logical_root: "/workspace".to_owned(),
                    workspace_resolved_root: "/workspace".to_owned(),
                    execution_capabilities: ExecutionCapabilityObservation::unavailable(),
                },
            })
            .await
            .unwrap()
            .identity;
        let device_id = DeviceProvisioningService::new(&store)
            .provision_fixture_token(
                DeviceDisplayName::try_new("Stage 16 device".to_owned()).unwrap(),
                timestamp(T0),
                BearerToken::parse(TOKEN.to_owned()).unwrap(),
            )
            .await
            .unwrap()
            .summary
            .device_id;
        Fixture {
            _root: root,
            guard,
            store,
            identity,
            device_id,
        }
    }

    struct ReopenEstimator(TokenEstimatorIdentity);

    impl TokenEstimator for ReopenEstimator {
        fn identity(&self) -> &TokenEstimatorIdentity {
            &self.0
        }

        fn estimate(
            &self,
            _: &ModelTarget,
            _: &[TokenEstimateUnit],
        ) -> Result<ConservativeTokenEstimate, ProviderError> {
            ConservativeTokenEstimate::try_new(self.0.clone(), 1)
        }
    }

    fn reconstruction_components(
        store: Arc<SqliteStateStore>,
    ) -> (
        ContextAssembler,
        crate::application::model_selection::ModelSelectionResult,
    ) {
        let estimator = TokenEstimatorIdentity::try_new("stage16_reopen_v1", 1).unwrap();
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(100_000).unwrap(),
            max_output_tokens: TokenCount::try_new(100).unwrap(),
        });
        let target = ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new("stage16-reopen").unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new("fixture-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("fixture-account").unwrap(),
            requested_output_tokens: TokenCount::try_new(100).unwrap(),
            estimator: estimator.clone(),
            provider_native_options: ProviderNativeOptions::new(false),
        })
        .unwrap();
        let snapshot = Arc::new(
            ModelTargetSnapshot::try_new(
                target.reference().model_target_id().clone(),
                vec![target],
            )
            .unwrap(),
        );
        let selection = ModelSelectionPolicy::new(snapshot)
            .select(
                None,
                crate::domain::model::RequiredModelCapabilities {
                    text_input: true,
                    text_output: true,
                    custom_tool_calling: true,
                    streaming: true,
                    ordered_output_items: true,
                    structured_output: false,
                    reasoning_continuation: false,
                    required_output_tokens: TokenCount::try_new(1).unwrap(),
                },
            )
            .unwrap();
        let registry = Arc::new(
            ToolRegistry::v0(ToolSemanticPolicy {
                read_file_default_bytes: 4_096,
                read_file_max_bytes: 65_536,
                run_shell_command_max_bytes: 65_536,
                run_shell_default_timeout_ms: 60_000,
                run_shell_max_timeout_ms: 900_000,
            })
            .unwrap(),
        );
        let clock = Arc::new(TestClock::new(
            timestamp(T1).to_offset_datetime(),
            Duration::from_secs(1),
        ));
        (
            ContextAssembler::new(
                store,
                None,
                Arc::new(ReopenEstimator(estimator)),
                registry,
                VersionedInstructionSnapshot::v0(),
                clock,
            ),
            selection,
        )
    }

    fn client_message_id() -> ClientMessageId {
        ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap()
    }

    async fn accept(fixture: &Fixture, text: &str) -> crate::domain::MessageCommandReceipt {
        let client_message_id = client_message_id();
        CommandService::new(&fixture.store)
            .accept_message(
                AuthenticatedDevice::new(fixture.device_id),
                AcceptMessageCommand {
                    idempotency_key: IdempotencyKey::for_message(client_message_id),
                    client_message_id,
                    conversation_id: fixture.identity.conversation_id,
                    content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()])
                        .unwrap(),
                    accepted_at: timestamp(T1),
                },
            )
            .await
            .unwrap()
            .into_receipt()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ordinal_frontier_excludes_future_and_loads_current_trigger_once() {
        let fixture = fixture().await;
        let first = accept(&fixture, "first despite equal timestamps").await;
        let second = accept(&fixture, "second already queued").await;

        let first_snapshot = fixture
            .store
            .load_context_eligibility_snapshot(ContextEligibilityRequest {
                work_id: first.work_id,
            })
            .await
            .unwrap();
        assert_eq!(first_snapshot.active_work.ordinal, first.work_ordinal);
        assert_eq!(
            first_snapshot.active_trigger.message.message_id(),
            first.message_id
        );
        assert!(first_snapshot.prior_messages.is_empty());
        assert!(
            first_snapshot
                .prior_messages
                .iter()
                .all(|source| source.message.message_id() != second.message_id)
        );

        let second_snapshot = fixture
            .store
            .load_context_eligibility_snapshot(ContextEligibilityRequest {
                work_id: second.work_id,
            })
            .await
            .unwrap();
        assert_eq!(
            second_snapshot
                .prior_messages
                .iter()
                .map(|source| source.message.message_id())
                .collect::<Vec<_>>(),
            vec![first.message_id]
        );
        assert_eq!(
            second_snapshot.active_trigger.message.message_id(),
            second.message_id
        );
        assert_eq!(
            second_snapshot
                .prior_messages
                .iter()
                .filter(|source| source.message.message_id() == second.message_id)
                .count(),
            0
        );
        fixture.guard.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn all_context_reads_share_the_snapshot_established_before_concurrent_write() {
        let fixture = fixture().await;
        let active = accept(&fixture, "snapshot").await;
        let barriers = Arc::new(Barrier::new(2));
        let hook_barriers = barriers.clone();
        fixture
            .store
            .set_stage16_snapshot_hook(Some(Stage16SnapshotTestHook::new(move || {
                hook_barriers.wait();
                hook_barriers.wait();
            })));
        let runtime = fixture.guard.runtime().clone();
        let workstation_id = fixture.identity.workstation_id;
        let writer_barriers = barriers.clone();
        let writer = tokio::spawn(async move {
            tokio::task::spawn_blocking({
                let barriers = writer_barriers.clone();
                move || barriers.wait()
            })
            .await
            .unwrap();
            let mut connection = runtime.acquire().await.unwrap();
            sqlx::query("UPDATE workstations SET capabilities_json = ? WHERE workstation_id = ?")
                .bind("{\"mutated_after_snapshot\":true}")
                .bind(workstation_id.to_string())
                .execute(&mut *connection)
                .await
                .unwrap();
            drop(connection);
            tokio::task::spawn_blocking(move || writer_barriers.wait())
                .await
                .unwrap();
        });
        let snapshot = fixture
            .store
            .load_context_eligibility_snapshot(ContextEligibilityRequest {
                work_id: active.work_id,
            })
            .await
            .unwrap();
        writer.await.unwrap();
        assert!(
            snapshot.workstation.semantic_json["capabilities"]
                .get("mutated_after_snapshot")
                .is_none()
        );
        fixture.guard.shutdown().await;
    }

    #[tokio::test]
    async fn exact_trigger_relationship_is_required_and_corruption_fails_closed() {
        let fixture = fixture().await;
        let active = accept(&fixture, "trigger").await;
        let missing_trigger_work = WorkId::generate();
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO work_items SELECT ?, craxii_id, conversation_id, 2, kind, state, \
                    state_version, priority, workspace_id, runtime_instance_id, \
                    current_model_invocation_id, current_tool_execution_id, correlation_id, \
                    created_at, queued_at, started_at, cancel_requested_at, \
                    cancellation_reason_code, terminal_at, terminal_reason_code, \
                    terminal_detail_json FROM work_items WHERE work_id = ?",
        )
        .bind(missing_trigger_work.to_string())
        .bind(active.work_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
        drop(connection);
        let error = fixture
            .store
            .load_context_eligibility_snapshot(ContextEligibilityRequest {
                work_id: missing_trigger_work,
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ContextSourceStoreErrorKind::MissingSource);
        fixture.guard.shutdown().await;
    }

    #[tokio::test]
    async fn conversation_predicates_exclude_nearby_foreign_rows() {
        let fixture = fixture().await;
        let foreign = accept(&fixture, "foreign row").await;
        let active = accept(&fixture, "active row").await;
        let foreign_conversation = ConversationId::generate();
        let mut connection = fixture.guard.runtime().acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE work_items SET conversation_id = ? WHERE work_id = ?")
            .bind(foreign_conversation.to_string())
            .bind(foreign.work_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE messages SET conversation_id = ? WHERE message_id = ?")
            .bind(foreign_conversation.to_string())
            .bind(foreign.message_id.to_string())
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);

        let snapshot = fixture
            .store
            .load_context_eligibility_snapshot(ContextEligibilityRequest {
                work_id: active.work_id,
            })
            .await
            .unwrap();
        assert!(
            snapshot
                .prior_works
                .iter()
                .all(|source| source.work_id != foreign.work_id)
        );
        assert!(
            snapshot
                .prior_messages
                .iter()
                .all(|source| source.message.message_id() != foreign.message_id)
        );
        fixture.guard.shutdown().await;
    }

    #[tokio::test]
    async fn exact_manifest_sources_reconstruct_identical_request_after_database_reopen() {
        let fixture = fixture().await;
        let active = accept(&fixture, "reopen reconstruction source").await;
        let (assembler, selection) = reconstruction_components(Arc::new(fixture.store.clone()));
        let prepared = assembler
            .assemble(active.work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let old_request_bytes = prepared.request().canonical_bytes();
        let old_request_hash = prepared.prepared_manifest().rendered_request_sha256;
        let old_manifest_hash = prepared.prepared_manifest().manifest_sha256;
        drop(assembler);

        let Fixture {
            _root: root,
            guard,
            store,
            identity: _,
            device_id: _,
        } = fixture;
        drop(store);
        guard.shutdown().await;

        let reopened_guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
        let reopened_store = Arc::new(SqliteStateStore::new(reopened_guard.runtime().clone()));
        let (reopened_assembler, reopened_selection) =
            reconstruction_components(reopened_store.clone());
        assert_eq!(reopened_selection, selection);
        reopened_assembler
            .verify_reconstruction(&prepared)
            .await
            .unwrap();
        assert_eq!(prepared.request().canonical_bytes(), old_request_bytes);
        assert_eq!(
            prepared.prepared_manifest().rendered_request_sha256,
            old_request_hash
        );
        assert_eq!(
            prepared.prepared_manifest().manifest_sha256,
            old_manifest_hash
        );
        drop(reopened_assembler);
        drop(reopened_store);
        reopened_guard.shutdown().await;
    }
}

async fn reload_exact_source(
    transaction: &mut Transaction<'_, Sqlite>,
    manifest: &crate::ports::state_store::PreparedContextManifest,
    source: &PreparedContextSource,
) -> Result<ContextReloadedSource, ContextSourceStoreError> {
    match (&source.identity, source.kind) {
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::InstructionVersion,
                ..
            },
            ContextSourceKind::SystemInstruction | ContextSourceKind::DeveloperInstruction,
        ) => Ok(ContextReloadedSource::InstructionVersion),
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::ToolDefinition,
                ..
            },
            ContextSourceKind::ToolDefinition,
        ) => Ok(ContextReloadedSource::ToolDefinition),
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::Workstation,
                id,
            },
            ContextSourceKind::WorkstationCapabilitySummary,
        ) => {
            let workstation_id = parse_id(id)?;
            Ok(ContextReloadedSource::Workstation(
                load_exact_workstation(transaction, workstation_id).await?,
            ))
        }
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::Workspace,
                id,
            },
            ContextSourceKind::WorkspaceIdentity,
        ) => {
            let workspace_id = parse_id(id)?;
            Ok(ContextReloadedSource::Workspace(
                load_exact_workspace(transaction, workspace_id).await?,
            ))
        }
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::Message,
                id,
            },
            ContextSourceKind::UserMessage
            | ContextSourceKind::ActiveTrigger
            | ContextSourceKind::AssistantMessage,
        ) => {
            let message_id = parse_id(id)?;
            let message = load_exact_message(transaction, source.kind, message_id).await?;
            validate_reloaded_message_ownership(manifest, source.kind, &message)?;
            Ok(ContextReloadedSource::Message(message))
        }
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::ModelInvocation,
                id,
            },
            ContextSourceKind::CompletedModelOutput | ContextSourceKind::ProviderNativeContinuation,
        ) => {
            let invocation_id = parse_model_source_id(id, source.kind)?;
            let output = load_exact_model_output(transaction, invocation_id).await?;
            validate_reloaded_output_ownership(manifest, &output)?;
            Ok(ContextReloadedSource::ModelOutput(output))
        }
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::ToolExecution,
                id,
            },
            ContextSourceKind::ObservedToolResult | ContextSourceKind::SyntheticOutcomeUnknown,
        ) => {
            let tool_id = parse_id(id)?;
            let tool = load_exact_tool_result(transaction, tool_id).await?;
            validate_reloaded_tool_ownership(manifest, &tool)?;
            Ok(ContextReloadedSource::ToolResult(tool))
        }
        (
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::Work,
                id,
            },
            ContextSourceKind::SyntheticFailure | ContextSourceKind::SyntheticInterruption,
        ) => {
            let work = load_active_work(transaction, parse_id(id)?).await?;
            if work.conversation_id != manifest.eligibility_conversation_id
                || work.ordinal.get() >= manifest.active_work_ordinal
            {
                return Err(store_error(ContextSourceStoreErrorKind::InvalidOwnership));
            }
            Ok(ContextReloadedSource::Work(work))
        }
        (ContextSourceIdentity::Artifact(id), ContextSourceKind::ArtifactContent) => Ok(
            ContextReloadedSource::Artifact(load_artifact_descriptor(transaction, *id).await?),
        ),
        _ => Err(corrupt()),
    }
}

fn parse_model_source_id(
    value: &str,
    kind: ContextSourceKind,
) -> Result<ModelInvocationId, ContextSourceStoreError> {
    let id = match kind {
        ContextSourceKind::CompletedModelOutput => value
            .split_once(":item:")
            .filter(|(_, index)| index.parse::<usize>().is_ok_and(|index| index > 0))
            .map(|(id, _)| id),
        ContextSourceKind::ProviderNativeContinuation => value.strip_suffix(":continuation"),
        _ => None,
    }
    .ok_or_else(corrupt)?;
    parse_id(id)
}

fn validate_reloaded_message_ownership(
    manifest: &crate::ports::state_store::PreparedContextManifest,
    kind: ContextSourceKind,
    source: &ContextReloadedMessageSource,
) -> Result<(), ContextSourceStoreError> {
    let valid_position = match kind {
        ContextSourceKind::ActiveTrigger => {
            source.work_id == manifest.work_id
                && source.work_ordinal.get() == manifest.active_work_ordinal
        }
        ContextSourceKind::UserMessage | ContextSourceKind::AssistantMessage => {
            source.work_ordinal.get() < manifest.active_work_ordinal
        }
        _ => false,
    };
    if !valid_position
        || source.message.conversation_id() != manifest.eligibility_conversation_id
        || matches!(
            kind,
            ContextSourceKind::UserMessage | ContextSourceKind::ActiveTrigger
        ) && !manifest.input_event_ids.contains(&source.journal_event_id)
    {
        return Err(store_error(ContextSourceStoreErrorKind::InvalidOwnership));
    }
    Ok(())
}

fn validate_reloaded_output_ownership(
    manifest: &crate::ports::state_store::PreparedContextManifest,
    source: &ContextModelOutputSource,
) -> Result<(), ContextSourceStoreError> {
    if source.conversation_id != manifest.eligibility_conversation_id
        || source.work_ordinal.get() > manifest.active_work_ordinal
        || source.work_ordinal.get() == manifest.active_work_ordinal
            && source.work_id != manifest.work_id
    {
        return Err(store_error(ContextSourceStoreErrorKind::InvalidOwnership));
    }
    Ok(())
}

fn validate_reloaded_tool_ownership(
    manifest: &crate::ports::state_store::PreparedContextManifest,
    source: &ContextToolResultSource,
) -> Result<(), ContextSourceStoreError> {
    if source.conversation_id != manifest.eligibility_conversation_id
        || source.work_ordinal.get() > manifest.active_work_ordinal
        || source.work_ordinal.get() == manifest.active_work_ordinal
            && source.work_id != manifest.work_id
    {
        return Err(store_error(ContextSourceStoreErrorKind::InvalidOwnership));
    }
    Ok(())
}

async fn load_exact_message(
    transaction: &mut Transaction<'_, Sqlite>,
    kind: ContextSourceKind,
    message_id: crate::domain::MessageId,
) -> Result<ContextReloadedMessageSource, ContextSourceStoreError> {
    let query = match kind {
        ContextSourceKind::UserMessage | ContextSourceKind::ActiveTrigger => {
            "SELECT m.*, w.work_id AS owning_work_id, \
                    w.conversation_work_ordinal AS owning_work_ordinal, \
                    je.event_id AS source_event_id, je.journal_offset AS source_journal_offset \
             FROM messages m \
             JOIN journal_events je ON je.event_type = 'message.accepted' \
               AND json_extract(je.payload_json, '$.message_id') = m.message_id \
             JOIN work_item_inputs wi ON wi.input_event_id = je.event_id \
               AND wi.relationship = 'trigger' AND wi.ordinal_within_work = 1 \
             JOIN work_items w ON w.work_id = wi.work_id \
             WHERE m.message_id = ? AND m.role = 'user' \
               AND m.conversation_id = w.conversation_id \
               AND je.conversation_id = w.conversation_id"
        }
        ContextSourceKind::AssistantMessage => {
            "SELECT m.*, w.work_id AS owning_work_id, \
                    w.conversation_work_ordinal AS owning_work_ordinal, \
                    je.event_id AS source_event_id, je.journal_offset AS source_journal_offset \
             FROM messages m \
             JOIN work_items w ON w.work_id = m.produced_by_work_id \
             JOIN journal_events je ON je.event_type = 'assistant.message_committed' \
               AND je.work_id = w.work_id AND je.conversation_id = w.conversation_id \
               AND json_extract(je.payload_json, '$.message_id') = m.message_id \
             WHERE m.message_id = ? AND m.role = 'assistant' \
               AND m.conversation_id = w.conversation_id"
        }
        _ => return Err(corrupt()),
    };
    let rows = sqlx::query(query)
        .bind(message_id.to_string())
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage)?;
    if rows.len() != 1 {
        return Err(if rows.is_empty() {
            missing()
        } else {
            corrupt()
        });
    }
    let row = &rows[0];
    Ok(ContextReloadedMessageSource {
        work_id: parse_id(
            &row.try_get::<String, _>("owning_work_id")
                .map_err(storage)?,
        )?,
        work_ordinal: positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?,
        journal_event_id: parse_id(
            &row.try_get::<String, _>("source_event_id")
                .map_err(storage)?,
        )?,
        journal_offset: journal_offset(row.try_get("source_journal_offset").map_err(storage)?)?,
        message: decode_message_row(row).map_err(|_| corrupt())?,
    })
}

async fn load_exact_model_output(
    transaction: &mut Transaction<'_, Sqlite>,
    invocation_id: ModelInvocationId,
) -> Result<ContextModelOutputSource, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT mi.*, w.conversation_id AS owning_conversation_id, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                EXISTS(SELECT 1 FROM messages am WHERE am.produced_by_work_id = w.work_id \
                       AND am.conversation_id = w.conversation_id AND am.role = 'assistant') \
                  AS has_final_assistant, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = mi.work_id AND je.conversation_id = w.conversation_id \
                   AND je.event_type = 'model.invocation_completed' \
                   AND json_extract(je.payload_json, '$.model_invocation_id') = \
                       mi.model_invocation_id) AS completion_journal_offset \
         FROM model_invocations mi \
         JOIN work_items w ON w.work_id = mi.work_id \
         WHERE mi.model_invocation_id = ? AND mi.state = 'completed' \
           AND mi.normalized_output_json IS NOT NULL",
    )
    .bind(invocation_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    let mut source = decode_model_output(&row)?;
    for item in &source.normalized_output.items {
        if let crate::ports::state_store::NormalizedModelOutputItem::ProviderOpaque {
            sha256,
            artifact_id,
            ..
        } = item
        {
            let artifact = load_artifact_descriptor(transaction, *artifact_id).await?;
            if artifact.sha256 != *sha256 {
                return Err(corrupt());
            }
            source.provider_opaque_artifacts.push(artifact);
        }
    }
    Ok(source)
}

async fn load_exact_tool_result(
    transaction: &mut Transaction<'_, Sqlite>,
    tool_id: ToolExecutionId,
) -> Result<ContextToolResultSource, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT te.*, w.conversation_id AS owning_conversation_id, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = te.work_id AND je.conversation_id = w.conversation_id \
                   AND je.event_type IN ('tool.execution_completed', \
                       'tool.execution_interrupted_before_dispatch', \
                       'tool.execution_outcome_unknown') \
                   AND json_extract(je.payload_json, '$.tool_execution_id') = \
                       te.tool_execution_id) AS terminal_journal_offset, \
                oa.artifact_id AS stdout_meta_id, oa.storage_key AS stdout_storage_key, \
                oa.sha256 AS stdout_sha256, \
                oa.captured_byte_count AS stdout_artifact_bytes, \
                ea.artifact_id AS stderr_meta_id, ea.storage_key AS stderr_storage_key, \
                ea.sha256 AS stderr_sha256, \
                ea.captured_byte_count AS stderr_artifact_bytes \
         FROM tool_executions te \
         JOIN work_items w ON w.work_id = te.work_id \
         LEFT JOIN artifacts oa ON oa.artifact_id = te.stdout_artifact_id \
         LEFT JOIN artifacts ea ON ea.artifact_id = te.stderr_artifact_id \
         WHERE te.tool_execution_id = ? \
           AND te.state IN ('completed', 'interrupted_before_dispatch', 'outcome_unknown')",
    )
    .bind(tool_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    decode_tool_result(&row)
}

async fn load_exact_workstation(
    transaction: &mut Transaction<'_, Sqlite>,
    workstation_id: WorkstationId,
) -> Result<ContextWorkstationSource, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT workstation_id, generation, architecture, os_release, capabilities_json \
         FROM workstations WHERE workstation_id = ?",
    )
    .bind(workstation_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    let capabilities: Value = serde_json::from_str(
        &row.try_get::<String, _>("capabilities_json")
            .map_err(storage)?,
    )
    .map_err(|_| corrupt())?;
    let semantic_json = json!({
        "architecture": row.try_get::<String, _>("architecture").map_err(storage)?,
        "capabilities": capabilities,
        "generation": row.try_get::<i64, _>("generation").map_err(storage)?,
        "os_release": row.try_get::<String, _>("os_release").map_err(storage)?,
        "workstation_id": workstation_id.to_string(),
    });
    Ok(ContextWorkstationSource {
        workstation_id,
        source_sha256: canonical_json_hash(&semantic_json),
        semantic_json,
    })
}

async fn load_exact_workspace(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: WorkspaceId,
) -> Result<ContextWorkspaceSource, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT workspace_id, workstation_id, logical_name, logical_root \
         FROM workspaces WHERE workspace_id = ?",
    )
    .bind(workspace_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    let semantic_json = json!({
        "logical_name": row.try_get::<String, _>("logical_name").map_err(storage)?,
        "logical_root": row.try_get::<String, _>("logical_root").map_err(storage)?,
        "workspace_id": workspace_id.to_string(),
        "workstation_id": row.try_get::<String, _>("workstation_id").map_err(storage)?,
    });
    Ok(ContextWorkspaceSource {
        workspace_id,
        source_sha256: canonical_json_hash(&semantic_json),
        semantic_json,
    })
}

async fn load_active_work(
    transaction: &mut Transaction<'_, Sqlite>,
    work_id: WorkId,
) -> Result<ContextWorkSource, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT work_id, conversation_id, conversation_work_ordinal, workspace_id, state, \
                terminal_reason_code, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = work_items.work_id \
                   AND je.event_type IN ('work.completed', 'work.failed', 'work.cancelled', 'work.interrupted')) \
                 AS terminal_journal_offset \
         FROM work_items WHERE work_id = ?",
    )
    .bind(work_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    decode_work_source(&row)
}

fn decode_work_source(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ContextWorkSource, ContextSourceStoreError> {
    Ok(ContextWorkSource {
        work_id: parse_id(&row.try_get::<String, _>("work_id").map_err(storage)?)?,
        conversation_id: parse_id(
            &row.try_get::<String, _>("conversation_id")
                .map_err(storage)?,
        )?,
        ordinal: positive_ordinal(row.try_get("conversation_work_ordinal").map_err(storage)?)?,
        workspace_id: parse_id(&row.try_get::<String, _>("workspace_id").map_err(storage)?)?,
        state: decode_work_state(&row.try_get::<String, _>("state").map_err(storage)?)
            .map_err(|_| corrupt())?,
        terminal_reason: row.try_get("terminal_reason_code").map_err(storage)?,
        terminal_journal_offset: row
            .try_get::<Option<i64>, _>("terminal_journal_offset")
            .map_err(storage)?
            .map(journal_offset)
            .transpose()?,
    })
}

async fn load_prior_works(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextWorkSource>, ContextSourceStoreError> {
    let rows = sqlx::query(
        "SELECT w.work_id, w.conversation_id, w.conversation_work_ordinal, w.workspace_id, \
                w.state, w.terminal_reason_code, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = w.work_id \
                   AND je.event_type IN ('work.completed', 'work.failed', 'work.cancelled', 'work.interrupted')) \
                 AS terminal_journal_offset \
         FROM work_items w \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal < ? \
         ORDER BY w.conversation_work_ordinal ASC, w.work_id ASC",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.iter().map(decode_work_source).collect()
}

async fn load_exact_trigger(
    transaction: &mut Transaction<'_, Sqlite>,
    work_id: WorkId,
    conversation_id: ConversationId,
    work_ordinal: ConversationWorkOrdinal,
) -> Result<ContextMessageSource, ContextSourceStoreError> {
    let event_row = sqlx::query(
        "SELECT je.* FROM work_item_inputs wi \
         JOIN journal_events je ON je.event_id = wi.input_event_id \
         JOIN work_items w ON w.work_id = wi.work_id \
         WHERE wi.work_id = ? AND w.conversation_id = ? \
           AND wi.relationship = 'trigger' AND wi.ordinal_within_work = 1 \
           AND je.conversation_id = w.conversation_id AND je.event_type = 'message.accepted'",
    )
    .bind(work_id.to_string())
    .bind(conversation_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    message_source_from_event(transaction, work_id, work_ordinal, &event_row).await
}

async fn load_prior_messages(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextMessageSource>, ContextSourceStoreError> {
    let rows = sqlx::query(
        "SELECT je.*, w.work_id AS owning_work_id, \
                w.conversation_work_ordinal AS owning_work_ordinal \
         FROM work_items w \
         JOIN work_item_inputs wi ON wi.work_id = w.work_id \
         JOIN journal_events je ON je.event_id = wi.input_event_id \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal < ? \
           AND wi.relationship = 'trigger' AND wi.ordinal_within_work = 1 \
           AND je.conversation_id = w.conversation_id AND je.event_type = 'message.accepted' \
         ORDER BY w.conversation_work_ordinal ASC, wi.ordinal_within_work ASC, \
                  je.journal_offset ASC, je.event_id ASC",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let mut sources = Vec::with_capacity(rows.len());
    for row in rows {
        let work_id = parse_id(
            &row.try_get::<String, _>("owning_work_id")
                .map_err(storage)?,
        )?;
        let ordinal = positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?;
        sources.push(message_source_from_event(transaction, work_id, ordinal, &row).await?);
    }
    Ok(sources)
}

async fn message_source_from_event(
    transaction: &mut Transaction<'_, Sqlite>,
    work_id: WorkId,
    work_ordinal: ConversationWorkOrdinal,
    event_row: &sqlx::sqlite::SqliteRow,
) -> Result<ContextMessageSource, ContextSourceStoreError> {
    let event = decode_event_row(event_row).map_err(|_| corrupt())?;
    let accepted = match event.payload {
        JournalEventPayload::MessageAccepted(value) => value,
        _ => return Err(corrupt()),
    };
    let message_row =
        sqlx::query("SELECT * FROM messages WHERE message_id = ? AND conversation_id = ?")
            .bind(accepted.message_id.to_string())
            .bind(accepted.conversation_id.to_string())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(storage)?
            .ok_or_else(missing)?;
    let message = decode_message_row(&message_row).map_err(|_| corrupt())?;
    if message.content_sha256() != accepted.content_sha256 || message.role() != accepted.role {
        return Err(corrupt());
    }
    Ok(ContextMessageSource {
        work_id,
        work_ordinal,
        input_event_id: event.event_id,
        journal_offset: event.journal_offset,
        message,
    })
}

async fn load_prior_assistant_messages(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextAssistantMessageSource>, ContextSourceStoreError> {
    let rows = sqlx::query(
        "SELECT m.*, w.work_id AS owning_work_id, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                je.event_id AS assistant_event_id, je.journal_offset AS assistant_journal_offset \
         FROM work_items w \
         JOIN messages m ON m.produced_by_work_id = w.work_id \
         JOIN journal_events je ON je.work_id = w.work_id \
           AND je.conversation_id = w.conversation_id \
           AND je.event_type = 'assistant.message_committed' \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal < ? \
           AND m.conversation_id = w.conversation_id AND m.role = 'assistant' \
         ORDER BY w.conversation_work_ordinal ASC, je.journal_offset ASC, m.message_id ASC",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.iter()
        .map(|row| {
            let message = decode_message_row(row).map_err(|_| corrupt())?;
            let work_id = parse_id(
                &row.try_get::<String, _>("owning_work_id")
                    .map_err(storage)?,
            )?;
            if message.produced_by_work_id() != Some(work_id) {
                return Err(corrupt());
            }
            Ok(ContextAssistantMessageSource {
                work_id,
                work_ordinal: positive_ordinal(
                    row.try_get("owning_work_ordinal").map_err(storage)?,
                )?,
                journal_event_id: parse_id(
                    &row.try_get::<String, _>("assistant_event_id")
                        .map_err(storage)?,
                )?,
                journal_offset: journal_offset(
                    row.try_get("assistant_journal_offset").map_err(storage)?,
                )?,
                message,
            })
        })
        .collect()
}

async fn load_continuation_boundaries(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextContinuationBoundary>, ContextSourceStoreError> {
    let model_rows = sqlx::query(
        "SELECT mi.model_invocation_id, mi.logical_invocation_id, mi.work_id, \
                mi.agent_step_no, mi.attempt_no, mi.state, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = mi.work_id AND je.conversation_id = w.conversation_id \
                   AND json_extract(je.payload_json, '$.model_invocation_id') = \
                       mi.model_invocation_id \
                   AND je.event_type = CASE mi.state \
                       WHEN 'completed' THEN 'model.invocation_completed' \
                       WHEN 'failed' THEN 'model.invocation_failed' \
                       ELSE 'model.invocation_interrupted' END) AS terminal_journal_offset \
         FROM model_invocations mi \
         JOIN work_items w ON w.work_id = mi.work_id \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal <= ? \
           AND mi.state IN ('completed', 'failed', 'cancelled_locally', \
                            'provider_outcome_unknown')",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let tool_rows = sqlx::query(
        "SELECT te.tool_execution_id, te.source_model_invocation_id, te.work_id, \
                te.agent_step_no, te.tool_ordinal, te.state, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = te.work_id AND je.conversation_id = w.conversation_id \
                   AND json_extract(je.payload_json, '$.tool_execution_id') = \
                       te.tool_execution_id \
                   AND je.event_type = CASE te.state \
                       WHEN 'completed' THEN 'tool.execution_completed' \
                       WHEN 'interrupted_before_dispatch' \
                           THEN 'tool.execution_interrupted_before_dispatch' \
                       ELSE 'tool.execution_outcome_unknown' END) AS terminal_journal_offset \
         FROM tool_executions te \
         JOIN work_items w ON w.work_id = te.work_id \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal <= ? \
           AND te.state IN ('completed', 'interrupted_before_dispatch', 'outcome_unknown')",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;

    let mut boundaries = Vec::with_capacity(model_rows.len() + tool_rows.len());
    for row in model_rows {
        let state = match row.try_get::<String, _>("state").map_err(storage)?.as_str() {
            "completed" => ModelInvocationState::Completed,
            "failed" => ModelInvocationState::Failed,
            "cancelled_locally" => ModelInvocationState::CancelledLocally,
            "provider_outcome_unknown" => ModelInvocationState::ProviderOutcomeUnknown,
            _ => return Err(corrupt()),
        };
        boundaries.push(ContextContinuationBoundary::Model {
            model_invocation_id: parse_id(
                &row.try_get::<String, _>("model_invocation_id")
                    .map_err(storage)?,
            )?,
            logical_invocation_id: parse_id(
                &row.try_get::<String, _>("logical_invocation_id")
                    .map_err(storage)?,
            )?,
            work_id: parse_id(&row.try_get::<String, _>("work_id").map_err(storage)?)?,
            work_ordinal: positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?,
            agent_step_no: AgentStepNo::try_new(row.try_get("agent_step_no").map_err(storage)?)
                .map_err(|_| corrupt())?,
            attempt_no: row.try_get("attempt_no").map_err(storage)?,
            state,
            journal_offset: row
                .try_get::<Option<i64>, _>("terminal_journal_offset")
                .map_err(storage)?
                .ok_or_else(missing)
                .and_then(journal_offset)?,
        });
    }
    for row in tool_rows {
        let state = match row.try_get::<String, _>("state").map_err(storage)?.as_str() {
            "completed" => ToolExecutionState::Completed,
            "interrupted_before_dispatch" => ToolExecutionState::InterruptedBeforeDispatch,
            "outcome_unknown" => ToolExecutionState::OutcomeUnknown,
            _ => return Err(corrupt()),
        };
        boundaries.push(ContextContinuationBoundary::Tool {
            tool_execution_id: parse_id(
                &row.try_get::<String, _>("tool_execution_id")
                    .map_err(storage)?,
            )?,
            source_model_invocation_id: parse_id(
                &row.try_get::<String, _>("source_model_invocation_id")
                    .map_err(storage)?,
            )?,
            work_id: parse_id(&row.try_get::<String, _>("work_id").map_err(storage)?)?,
            work_ordinal: positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?,
            agent_step_no: AgentStepNo::try_new(row.try_get("agent_step_no").map_err(storage)?)
                .map_err(|_| corrupt())?,
            tool_ordinal: ToolOrdinal::try_new(row.try_get("tool_ordinal").map_err(storage)?)
                .map_err(|_| corrupt())?,
            state,
            journal_offset: row
                .try_get::<Option<i64>, _>("terminal_journal_offset")
                .map_err(storage)?
                .ok_or_else(missing)
                .and_then(journal_offset)?,
        });
    }
    boundaries.sort_by(|left, right| {
        continuation_boundary_key(left).cmp(&continuation_boundary_key(right))
    });
    Ok(boundaries)
}

fn continuation_boundary_key(
    boundary: &ContextContinuationBoundary,
) -> (i64, i64, i64, i64, i64, String) {
    match boundary {
        ContextContinuationBoundary::Model {
            model_invocation_id,
            work_ordinal,
            agent_step_no,
            attempt_no,
            journal_offset,
            ..
        } => (
            work_ordinal.get(),
            agent_step_no.get(),
            0,
            *attempt_no,
            journal_offset.get(),
            model_invocation_id.to_string(),
        ),
        ContextContinuationBoundary::Tool {
            tool_execution_id,
            work_ordinal,
            agent_step_no,
            tool_ordinal,
            journal_offset,
            ..
        } => (
            work_ordinal.get(),
            agent_step_no.get(),
            1,
            tool_ordinal.get(),
            journal_offset.get(),
            tool_execution_id.to_string(),
        ),
    }
}

async fn load_completed_model_outputs(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextModelOutputSource>, ContextSourceStoreError> {
    let rows = sqlx::query(
        "SELECT mi.*, w.conversation_id AS owning_conversation_id, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                EXISTS(SELECT 1 FROM messages am WHERE am.produced_by_work_id = w.work_id \
                       AND am.conversation_id = w.conversation_id AND am.role = 'assistant') \
                  AS has_final_assistant, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = mi.work_id AND je.conversation_id = w.conversation_id \
                   AND je.event_type = 'model.invocation_completed' \
                   AND json_extract(je.payload_json, '$.model_invocation_id') = \
                       mi.model_invocation_id) AS completion_journal_offset \
         FROM model_invocations mi \
         JOIN work_items w ON w.work_id = mi.work_id \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal <= ? \
           AND mi.state = 'completed' AND mi.normalized_output_json IS NOT NULL \
         ORDER BY w.conversation_work_ordinal ASC, mi.agent_step_no ASC, mi.attempt_no ASC, \
                  completion_journal_offset ASC, mi.model_invocation_id ASC",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    let mut sources = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut source = decode_model_output(row)?;
        for item in &source.normalized_output.items {
            if let crate::ports::state_store::NormalizedModelOutputItem::ProviderOpaque {
                sha256,
                artifact_id,
                ..
            } = item
            {
                let artifact = load_artifact_descriptor(transaction, *artifact_id).await?;
                if artifact.sha256 != *sha256 {
                    return Err(corrupt());
                }
                source.provider_opaque_artifacts.push(artifact);
            }
        }
        sources.push(source);
    }
    Ok(sources)
}

fn decode_model_output(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ContextModelOutputSource, ContextSourceStoreError> {
    let capabilities = decode_model_capabilities(
        &row.try_get::<String, _>("model_capabilities_json")
            .map_err(storage)?,
    )
    .map_err(|_| corrupt())?;
    let provider_model = ProviderModelReference::new(
        ModelTargetId::try_new(
            row.try_get::<String, _>("model_target_id")
                .map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        ProviderId::try_new(row.try_get::<String, _>("provider_id").map_err(storage)?)
            .map_err(|_| corrupt())?,
        ProviderModelId::try_new(
            row.try_get::<String, _>("provider_model_id")
                .map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        TargetConfigurationVersion::try_new(
            row.try_get("target_configuration_version")
                .map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        capabilities,
    );
    Ok(ContextModelOutputSource {
        model_invocation_id: parse_id(
            &row.try_get::<String, _>("model_invocation_id")
                .map_err(storage)?,
        )?,
        logical_invocation_id: parse_id(
            &row.try_get::<String, _>("logical_invocation_id")
                .map_err(storage)?,
        )?,
        work_id: parse_id(&row.try_get::<String, _>("work_id").map_err(storage)?)?,
        conversation_id: parse_id(
            &row.try_get::<String, _>("owning_conversation_id")
                .map_err(storage)?,
        )?,
        work_ordinal: positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?,
        agent_step_no: AgentStepNo::try_new(row.try_get("agent_step_no").map_err(storage)?)
            .map_err(|_| corrupt())?,
        attempt_no: row.try_get("attempt_no").map_err(storage)?,
        provider_model,
        normalized_output: validate_normalized_output(
            &row.try_get::<String, _>("normalized_output_json")
                .map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        provider_opaque_artifacts: Vec::new(),
        stop_reason: row.try_get("stop_reason").map_err(storage)?,
        journal_offset: row
            .try_get::<Option<i64>, _>("completion_journal_offset")
            .map_err(storage)?
            .ok_or_else(missing)
            .and_then(journal_offset)?,
        has_committed_final_assistant: row
            .try_get::<i64, _>("has_final_assistant")
            .map_err(storage)?
            == 1,
    })
}

async fn load_artifact_descriptor(
    transaction: &mut Transaction<'_, Sqlite>,
    artifact_id: ArtifactId,
) -> Result<ContextArtifactDescriptor, ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT artifact_id, storage_key, sha256, captured_byte_count \
         FROM artifacts WHERE artifact_id = ?",
    )
    .bind(artifact_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    Ok(ContextArtifactDescriptor {
        artifact_id,
        storage_key: ArtifactStorageKey::parse_canonical(
            &row.try_get::<String, _>("storage_key").map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        sha256: Sha256Digest::parse_canonical(
            &row.try_get::<String, _>("sha256").map_err(storage)?,
        )
        .map_err(|_| corrupt())?,
        captured_byte_count: byte_count(row.try_get("captured_byte_count").map_err(storage)?)?,
    })
}

async fn load_tool_results(
    transaction: &mut Transaction<'_, Sqlite>,
    conversation_id: ConversationId,
    active_ordinal: ConversationWorkOrdinal,
) -> Result<Vec<ContextToolResultSource>, ContextSourceStoreError> {
    let rows = sqlx::query(
        "SELECT te.*, w.conversation_id AS owning_conversation_id, \
                w.conversation_work_ordinal AS owning_work_ordinal, \
                (SELECT max(je.journal_offset) FROM journal_events je \
                 WHERE je.work_id = te.work_id AND je.conversation_id = w.conversation_id \
                   AND je.event_type IN ('tool.execution_completed', \
                       'tool.execution_interrupted_before_dispatch', 'tool.execution_outcome_unknown') \
                   AND json_extract(je.payload_json, '$.tool_execution_id') = \
                       te.tool_execution_id) \
                  AS terminal_journal_offset, \
                oa.artifact_id AS stdout_meta_id, oa.storage_key AS stdout_storage_key, \
                oa.sha256 AS stdout_sha256, oa.captured_byte_count AS stdout_artifact_bytes, \
                ea.artifact_id AS stderr_meta_id, ea.storage_key AS stderr_storage_key, \
                ea.sha256 AS stderr_sha256, ea.captured_byte_count AS stderr_artifact_bytes \
         FROM tool_executions te \
         JOIN work_items w ON w.work_id = te.work_id \
         JOIN model_invocations mi ON mi.model_invocation_id = te.source_model_invocation_id \
           AND mi.work_id = te.work_id \
         LEFT JOIN artifacts oa ON oa.artifact_id = te.stdout_artifact_id \
         LEFT JOIN artifacts ea ON ea.artifact_id = te.stderr_artifact_id \
         WHERE w.conversation_id = ? AND w.conversation_work_ordinal <= ? \
           AND te.state IN ('completed', 'interrupted_before_dispatch', 'outcome_unknown') \
         ORDER BY w.conversation_work_ordinal ASC, te.agent_step_no ASC, te.tool_ordinal ASC, \
                  terminal_journal_offset ASC, te.tool_execution_id ASC",
    )
    .bind(conversation_id.to_string())
    .bind(active_ordinal.get())
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage)?;
    rows.iter().map(decode_tool_result).collect()
}

fn decode_tool_result(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ContextToolResultSource, ContextSourceStoreError> {
    let state = match row.try_get::<String, _>("state").map_err(storage)?.as_str() {
        "completed" => ToolExecutionState::Completed,
        "interrupted_before_dispatch" => ToolExecutionState::InterruptedBeforeDispatch,
        "outcome_unknown" => ToolExecutionState::OutcomeUnknown,
        _ => return Err(corrupt()),
    };
    let result = row
        .try_get::<Option<String>, _>("result_json")
        .map_err(storage)?
        .map(|value| serde_json::from_str(&value).map_err(|_| corrupt()))
        .transpose()?;
    let stdout_counts = decode_counts(row, "stdout")?;
    let stderr_counts = decode_counts(row, "stderr")?;
    Ok(ContextToolResultSource {
        tool_execution_id: parse_id(
            &row.try_get::<String, _>("tool_execution_id")
                .map_err(storage)?,
        )?,
        work_id: parse_id(&row.try_get::<String, _>("work_id").map_err(storage)?)?,
        conversation_id: parse_id(
            &row.try_get::<String, _>("owning_conversation_id")
                .map_err(storage)?,
        )?,
        work_ordinal: positive_ordinal(row.try_get("owning_work_ordinal").map_err(storage)?)?,
        source_model_invocation_id: parse_id(
            &row.try_get::<String, _>("source_model_invocation_id")
                .map_err(storage)?,
        )?,
        agent_step_no: AgentStepNo::try_new(row.try_get("agent_step_no").map_err(storage)?)
            .map_err(|_| corrupt())?,
        tool_ordinal: ToolOrdinal::try_new(row.try_get("tool_ordinal").map_err(storage)?)
            .map_err(|_| corrupt())?,
        provider_tool_call_id: row
            .try_get::<Option<String>, _>("provider_tool_call_id")
            .map_err(storage)?
            .ok_or_else(missing)?,
        tool_name: ToolName::try_new(row.try_get::<String, _>("tool_name").map_err(storage)?)
            .map_err(|_| corrupt())?,
        state,
        result,
        stdout_counts,
        stderr_counts,
        stdout_artifact: decode_artifact(row, "stdout")?,
        stderr_artifact: decode_artifact(row, "stderr")?,
        truncated: row.try_get::<i64, _>("truncated").map_err(storage)? == 1,
        journal_offset: row
            .try_get::<Option<i64>, _>("terminal_journal_offset")
            .map_err(storage)?
            .ok_or_else(missing)
            .and_then(journal_offset)?,
    })
}

fn decode_counts(
    row: &sqlx::sqlite::SqliteRow,
    prefix: &str,
) -> Result<Option<ContextStreamCounts>, ContextSourceStoreError> {
    let values = ["observed", "captured", "returned_inline", "omitted"]
        .map(|suffix| {
            row.try_get::<Option<i64>, _>(format!("{prefix}_{suffix}_bytes").as_str())
                .map_err(storage)
        })
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    match values.as_slice() {
        [None, None, None, None] => Ok(None),
        [
            Some(observed),
            Some(captured),
            Some(returned_inline),
            Some(omitted),
        ] => Ok(Some(ContextStreamCounts {
            observed: byte_count(*observed)?,
            captured: byte_count(*captured)?,
            returned_inline: byte_count(*returned_inline)?,
            omitted: byte_count(*omitted)?,
        })),
        _ => Err(corrupt()),
    }
}

fn decode_artifact(
    row: &sqlx::sqlite::SqliteRow,
    prefix: &str,
) -> Result<Option<ContextArtifactDescriptor>, ContextSourceStoreError> {
    let id = row
        .try_get::<Option<String>, _>(format!("{prefix}_meta_id").as_str())
        .map_err(storage)?;
    let key = row
        .try_get::<Option<String>, _>(format!("{prefix}_storage_key").as_str())
        .map_err(storage)?;
    let digest = row
        .try_get::<Option<String>, _>(format!("{prefix}_sha256").as_str())
        .map_err(storage)?;
    let bytes = row
        .try_get::<Option<i64>, _>(format!("{prefix}_artifact_bytes").as_str())
        .map_err(storage)?;
    match (id, key, digest, bytes) {
        (None, None, None, None) => Ok(None),
        (Some(id), Some(key), Some(digest), Some(bytes)) => Ok(Some(ContextArtifactDescriptor {
            artifact_id: ArtifactId::parse_canonical(&id).map_err(|_| corrupt())?,
            storage_key: ArtifactStorageKey::parse_canonical(&key).map_err(|_| corrupt())?,
            sha256: Sha256Digest::parse_canonical(&digest).map_err(|_| corrupt())?,
            captured_byte_count: byte_count(bytes)?,
        })),
        _ => Err(corrupt()),
    }
}

async fn load_capability_and_workspace(
    transaction: &mut Transaction<'_, Sqlite>,
    active_work: &ContextWorkSource,
) -> Result<(ContextWorkstationSource, ContextWorkspaceSource), ContextSourceStoreError> {
    let row = sqlx::query(
        "SELECT ws.workspace_id, ws.craxii_id, ws.workstation_id, ws.logical_name, ws.logical_root, \
                w.generation, w.architecture, w.os_release, w.capabilities_json \
         FROM workspaces ws JOIN workstations w ON w.workstation_id = ws.workstation_id \
         WHERE ws.workspace_id = ? AND ws.craxii_id = w.craxii_id",
    )
    .bind(active_work.workspace_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage)?
    .ok_or_else(missing)?;
    let workspace_id: WorkspaceId =
        parse_id(&row.try_get::<String, _>("workspace_id").map_err(storage)?)?;
    let workstation_id: WorkstationId = parse_id(
        &row.try_get::<String, _>("workstation_id")
            .map_err(storage)?,
    )?;
    let capabilities: Value = serde_json::from_str(
        &row.try_get::<String, _>("capabilities_json")
            .map_err(storage)?,
    )
    .map_err(|_| corrupt())?;
    let workstation_semantic = json!({
        "architecture": row.try_get::<String, _>("architecture").map_err(storage)?,
        "capabilities": capabilities,
        "generation": row.try_get::<i64, _>("generation").map_err(storage)?,
        "os_release": row.try_get::<String, _>("os_release").map_err(storage)?,
        "workstation_id": workstation_id.to_string(),
    });
    let workspace_semantic = json!({
        "logical_name": row.try_get::<String, _>("logical_name").map_err(storage)?,
        "logical_root": row.try_get::<String, _>("logical_root").map_err(storage)?,
        "workspace_id": workspace_id.to_string(),
        "workstation_id": workstation_id.to_string(),
    });
    Ok((
        ContextWorkstationSource {
            workstation_id,
            source_sha256: canonical_json_hash(&workstation_semantic),
            semantic_json: workstation_semantic,
        },
        ContextWorkspaceSource {
            workspace_id,
            source_sha256: canonical_json_hash(&workspace_semantic),
            semantic_json: workspace_semantic,
        },
    ))
}
