use sqlx::Row;

use crate::application::projector::project;
use crate::domain::{
    Conversation, ConversationKind, ConversationLifecycle, ConversationWorkOrdinal, CraxiiId,
    CraxiiInitializedV1, CraxiiPrincipal, CraxiiPrincipalInput, JournalActor,
    JournalCurrentAttempt, JournalEvent, JournalEventId, JournalEventPayload, JournalStreamId,
    LogicalPathReference, ModelInvocationId, ProjectionVersion, RuntimeInstanceId, SchemaVersion,
    Sha256Digest, ToolExecutionId, UtcTimestamp, WorkId, WorkspaceCapabilityRef, WorkspaceIdentity,
    WorkspaceIdentityInput, WorkstationCapabilities, WorkstationCapabilitiesInput,
    WorkstationCapabilityFlags, WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits,
    WorkstationIdentity,
};
use crate::ports::state_store::{
    ApplicationConsistencyReceipt, BootstrapObservation, BootstrapSnapshot, BootstrapStateStore,
    CommitReceipt, CommittedEventRange, LoadOrBootstrapIdentityReceipt,
    LoadOrBootstrapIdentityRequest, StateStoreError, StateStoreErrorKind, StateStoreFuture,
    V0IdentityReference,
};

use super::codec::{
    decode_message_row, decode_optional_id, decode_optional_timestamp, decode_timestamp,
    decode_work_state, decode_workstation_row, encode_workstation_capabilities,
};
use super::error::{SqliteAdapterError, SqliteFailureKind};
use super::journal::{JournalAppendIntent, append_event, decode_event_row, prepare_event};
use super::runtime::SqliteRuntime;
use super::transaction::WriteTransaction;

const DISPLAY_NAME: &str = "Craxii";
const OWNER_LABEL: &str = "local-owner";
const ARCHITECTURE_REVISION: &str = "V0.0.01";
const SCHEMA_REVISION: i64 = 3;

#[derive(Clone, Debug)]
pub struct SqliteStateStore {
    pub(super) runtime: SqliteRuntime,
    #[cfg(test)]
    bootstrap_hook: std::sync::Arc<std::sync::Mutex<Option<BootstrapTestHook>>>,
    #[cfg(test)]
    pub(super) stage9_hook: std::sync::Arc<std::sync::Mutex<Option<super::stage9::Stage9TestHook>>>,
    #[cfg(test)]
    pub(super) stage11_snapshot_hook:
        std::sync::Arc<std::sync::Mutex<Option<super::stage11::Stage11SnapshotTestHook>>>,
}

impl SqliteStateStore {
    #[must_use]
    pub fn new(runtime: SqliteRuntime) -> Self {
        Self {
            runtime,
            #[cfg(test)]
            bootstrap_hook: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            stage9_hook: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            stage11_snapshot_hook: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    async fn load_or_bootstrap(
        &self,
        request: LoadOrBootstrapIdentityRequest,
    ) -> Result<LoadOrBootstrapIdentityReceipt, SqliteAdapterError> {
        let mut transaction =
            WriteTransaction::begin(&self.runtime, "bootstrap_v0_identity").await?;
        let product_rows = product_row_count(transaction.connection()).await?;
        if product_rows == 0 {
            self.fire_test_hook(BootstrapTestHook::BeforeFirstInsert)?;
            let prepared = PreparedBootstrap::new(&request)?;
            insert_root_rows(&mut transaction, &prepared, &request).await?;
            self.fire_test_hook(BootstrapTestHook::AfterRootRows)?;

            let root_position = append_event(
                &mut transaction,
                prepare_event(JournalAppendIntent {
                    event_id: request.initialized_event_id,
                    craxii_id: request.proposed.craxii_id,
                    stream_id: JournalStreamId::Craxii(request.proposed.craxii_id),
                    conversation_id: Some(request.proposed.conversation_id),
                    work_id: None,
                    causation_event_id: None,
                    correlation_id: request.correlation_id,
                    actor: JournalActor::Craxii(request.proposed.craxii_id),
                    runtime_instance_id: None,
                    payload: JournalEventPayload::CraxiiInitialized(prepared.initialized),
                    recorded_at: request.created_at,
                    occurred_at: None,
                })?,
            )
            .await?;
            self.fire_test_hook(BootstrapTestHook::AfterFirstEvent)?;
            let conversation_position = append_event(
                &mut transaction,
                prepare_event(JournalAppendIntent {
                    event_id: request.conversation_created_event_id,
                    craxii_id: request.proposed.craxii_id,
                    stream_id: JournalStreamId::Conversation(request.proposed.conversation_id),
                    conversation_id: Some(request.proposed.conversation_id),
                    work_id: None,
                    causation_event_id: Some(request.initialized_event_id),
                    correlation_id: request.correlation_id,
                    actor: JournalActor::Craxii(request.proposed.craxii_id),
                    runtime_instance_id: None,
                    payload: JournalEventPayload::ConversationCreated(prepared.conversation),
                    recorded_at: request.created_at,
                    occurred_at: None,
                })?,
            )
            .await?;
            self.fire_test_hook(BootstrapTestHook::AfterSecondEvent)?;
            transaction.commit().await?;
            Ok(LoadOrBootstrapIdentityReceipt {
                identity: request.proposed,
                created: true,
                commit: CommitReceipt {
                    committed_version: None,
                    events: Some(CommittedEventRange {
                        first: root_position.offset,
                        last: conversation_position.offset,
                    }),
                },
            })
        } else {
            let identity = validate_existing_bootstrap_in_write(&mut transaction, &request).await?;
            transaction.commit().await?;
            Ok(LoadOrBootstrapIdentityReceipt {
                identity,
                created: false,
                commit: CommitReceipt {
                    committed_version: None,
                    events: None,
                },
            })
        }
    }

    async fn verify_consistency(
        &self,
    ) -> Result<ApplicationConsistencyReceipt, SqliteAdapterError> {
        let mut transaction = self
            .runtime
            .inner
            .pool
            .begin()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;

        let event_rows = sqlx::query("SELECT * FROM journal_events ORDER BY journal_offset ASC")
            .fetch_all(&mut *transaction)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let events = event_rows
            .iter()
            .map(decode_event_row)
            .collect::<Result<Vec<_>, _>>()?;
        let projected = project(&events).map_err(|_| inconsistent())?;

        let head_rows = sqlx::query("SELECT stream_id, last_stream_seq FROM stream_heads")
            .fetch_all(&mut *transaction)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let mut heads = std::collections::HashMap::new();
        for row in head_rows {
            let stream: JournalStreamId = row
                .try_get::<String, _>("stream_id")?
                .parse()
                .map_err(|_| inconsistent())?;
            let sequence = crate::domain::StreamSeq::try_new(row.try_get("last_stream_seq")?)
                .map_err(|_| inconsistent())?;
            if heads.insert(stream, sequence).is_some() {
                return Err(inconsistent());
            }
        }
        let mut replay_heads = std::collections::HashMap::new();
        for event in &events {
            replay_heads.insert(event.stream_id, event.stream_seq);
        }
        if heads != replay_heads {
            return Err(inconsistent());
        }

        validate_exact_root_counts(&mut transaction).await?;
        let root = load_root_snapshot(&mut transaction).await?;
        compare_root_projection(&root, &projected, &events)?;
        compare_message_projection(&mut transaction, &projected).await?;
        compare_work_projection(&mut transaction, &projected).await?;
        compare_work_inputs(&mut transaction, &projected).await?;
        let stage8_invariants =
            super::stage8::verify_stage8_consistency(&mut transaction, &projected, &events).await?;
        let stage9_invariants =
            super::stage9::verify_stage9_consistency(&mut transaction, &events).await?;
        let stage10_invariants =
            super::stage10::verify_stage10_consistency(&mut transaction, &projected, &events)
                .await?;

        let journal_head = events.last().map(|event| event.journal_offset);
        transaction
            .commit()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        Ok(ApplicationConsistencyReceipt {
            checked_invariants: 18 + stage8_invariants + stage9_invariants + stage10_invariants,
            journal_head,
        })
    }

    async fn load_snapshot(&self) -> Result<BootstrapSnapshot, SqliteAdapterError> {
        let consistency = self.verify_consistency().await?;
        let mut transaction = self
            .runtime
            .inner
            .pool
            .begin()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let root = load_root_snapshot(&mut transaction).await?;
        transaction
            .commit()
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        let journal_head = consistency.journal_head.ok_or_else(inconsistent)?;
        Ok(BootstrapSnapshot {
            identity: V0IdentityReference {
                craxii_id: root.principal.craxii_id(),
                conversation_id: root.primary_conversation.conversation_id(),
                workstation_id: root.workstation.workstation_id(),
                workspace_id: root.workspace.workspace_id(),
            },
            principal: root.principal,
            workstation: root.workstation,
            workstation_capabilities: root.capabilities,
            workspace: root.workspace,
            primary_conversation: root.primary_conversation,
            journal_head,
            consistency,
        })
    }

    #[cfg(test)]
    pub(super) fn set_bootstrap_test_hook(&self, hook: Option<BootstrapTestHook>) {
        *self.bootstrap_hook.lock().unwrap() = hook;
    }

    #[cfg(test)]
    fn fire_test_hook(&self, hook: BootstrapTestHook) -> Result<(), SqliteAdapterError> {
        if self.bootstrap_hook.lock().unwrap().as_ref() == Some(&hook) {
            Err(SqliteAdapterError::new(
                SqliteFailureKind::InternalInvariant,
            ))
        } else {
            Ok(())
        }
    }

    #[cfg(not(test))]
    fn fire_test_hook(&self, _hook: BootstrapTestHook) -> Result<(), SqliteAdapterError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BootstrapTestHook {
    BeforeFirstInsert,
    AfterRootRows,
    AfterFirstEvent,
    AfterSecondEvent,
}

impl BootstrapStateStore for SqliteStateStore {
    fn load_or_bootstrap_v0_identity(
        &self,
        request: LoadOrBootstrapIdentityRequest,
    ) -> StateStoreFuture<'_, LoadOrBootstrapIdentityReceipt> {
        Box::pin(async move {
            self.load_or_bootstrap(request)
                .await
                .map_err(map_port_error)
        })
    }

    fn load_bootstrap_snapshot(&self) -> StateStoreFuture<'_, BootstrapSnapshot> {
        Box::pin(async move { self.load_snapshot().await.map_err(map_port_error) })
    }

    fn verify_application_consistency(
        &self,
    ) -> StateStoreFuture<'_, ApplicationConsistencyReceipt> {
        Box::pin(async move { self.verify_consistency().await.map_err(map_port_error) })
    }
}

pub(super) fn map_port_error(error: SqliteAdapterError) -> StateStoreError {
    let kind = match error.kind() {
        SqliteFailureKind::StateConflict => StateStoreErrorKind::StateConflict,
        SqliteFailureKind::IdempotencyConflict => StateStoreErrorKind::IdempotencyConflict,
        SqliteFailureKind::TargetNotFound => StateStoreErrorKind::TargetNotFound,
        SqliteFailureKind::InternalInvariant | SqliteFailureKind::InconsistentSchema => {
            StateStoreErrorKind::InternalInvariant
        }
        _ => StateStoreErrorKind::Storage,
    };
    StateStoreError::new(kind)
}

fn inconsistent() -> SqliteAdapterError {
    SqliteAdapterError::new(SqliteFailureKind::InconsistentSchema)
}

struct PreparedBootstrap {
    capabilities_json: String,
    initialized: CraxiiInitializedV1,
    conversation: crate::domain::ConversationCreatedV1,
}

impl PreparedBootstrap {
    fn new(request: &LoadOrBootstrapIdentityRequest) -> Result<Self, SqliteAdapterError> {
        let logical_root =
            LogicalPathReference::absolute(request.observation.workspace_logical_root.clone())
                .map_err(|_| inconsistent())?;
        let default_shell =
            LogicalPathReference::absolute(request.observation.default_shell.clone())
                .map_err(|_| inconsistent())?;
        let limits = WorkstationCapabilityLimits::try_new(
            request.observation.max_execution_timeout_ms,
            request.observation.max_stdout_bytes,
            request.observation.max_stderr_bytes,
        )
        .map_err(|_| inconsistent())?;
        let capabilities = WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
            workstation_id: request.proposed.workstation_id,
            generation: request.observation.initial_generation,
            cpu_architecture: request.observation.architecture.clone(),
            os_release: request.observation.os_release.clone(),
            default_shell,
            flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
                filesystem_read: false,
                foreground_execute: false,
                cancel_execution: false,
                inspect_execution: false,
                privilege_user: true,
                privilege_administrative: request.observation.administrative_enabled,
                process_group_cleanup: false,
                cgroup_cleanup: false,
            }),
            limits,
            workspaces: vec![
                WorkspaceCapabilityRef::try_new(request.proposed.workspace_id, logical_root)
                    .map_err(|_| inconsistent())?,
            ],
        })
        .map_err(|_| inconsistent())?;
        let capabilities_json = encode_workstation_capabilities(&capabilities)?;
        let capabilities_sha256 = Sha256Digest::hash_bytes(capabilities_json.as_bytes());
        Ok(Self {
            initialized: CraxiiInitializedV1 {
                craxii_id: request.proposed.craxii_id,
                display_name: DISPLAY_NAME.to_owned(),
                owner_label: OWNER_LABEL.to_owned(),
                architecture_revision: ARCHITECTURE_REVISION.to_owned(),
                schema_revision: SchemaVersion::try_new(SCHEMA_REVISION)
                    .map_err(|_| inconsistent())?,
                workstation_id: request.proposed.workstation_id,
                workstation_generation: request.observation.initial_generation,
                workstation_architecture: request.observation.architecture.clone(),
                workstation_os_release: request.observation.os_release.clone(),
                capabilities_sha256,
                workspace_id: request.proposed.workspace_id,
                workspace_logical_name: request.observation.workspace_logical_name.clone(),
                workspace_logical_root: request.observation.workspace_logical_root.clone(),
                primary_conversation_id: request.proposed.conversation_id,
                created_at: request.created_at,
            },
            conversation: crate::domain::ConversationCreatedV1 {
                conversation_id: request.proposed.conversation_id,
                craxii_id: request.proposed.craxii_id,
                kind: ConversationKind::Primary,
                lifecycle: ConversationLifecycle::Active,
                next_work_ordinal: ConversationWorkOrdinal::try_new(1)
                    .map_err(|_| inconsistent())?,
                state_version: ProjectionVersion::try_new(1).map_err(|_| inconsistent())?,
                created_at: request.created_at,
            },
            capabilities_json,
        })
    }
}

async fn insert_root_rows(
    transaction: &mut WriteTransaction,
    prepared: &PreparedBootstrap,
    request: &LoadOrBootstrapIdentityRequest,
) -> Result<(), SqliteAdapterError> {
    sqlx::query(
        "INSERT INTO craxii_principals (craxii_id, display_name, owner_label, lifecycle_state, \
         primary_conversation_id, default_workspace_id, created_at, architecture_revision, \
         schema_revision) VALUES (?, ?, ?, 'active', NULL, NULL, ?, ?, ?)",
    )
    .bind(request.proposed.craxii_id.to_string())
    .bind(DISPLAY_NAME)
    .bind(OWNER_LABEL)
    .bind(request.created_at.to_string())
    .bind(ARCHITECTURE_REVISION)
    .bind(SCHEMA_REVISION)
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    sqlx::query(
        "INSERT INTO workstations (workstation_id, craxii_id, kind, generation, hosting_provider, \
         provider_instance_id, provider_image_id, provisioning_revision, architecture, os_release, \
         capabilities_json, created_at, last_seen_at) \
         VALUES (?, ?, 'local', ?, 'unclassified', NULL, NULL, NULL, ?, ?, ?, ?, ?)",
    )
    .bind(request.proposed.workstation_id.to_string())
    .bind(request.proposed.craxii_id.to_string())
    .bind(request.observation.initial_generation.get())
    .bind(&request.observation.architecture)
    .bind(&request.observation.os_release)
    .bind(&prepared.capabilities_json)
    .bind(request.created_at.to_string())
    .bind(request.created_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, craxii_id, workstation_id, logical_name, \
         logical_root, local_resolved_root, lifecycle_state, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'active', ?)",
    )
    .bind(request.proposed.workspace_id.to_string())
    .bind(request.proposed.craxii_id.to_string())
    .bind(request.proposed.workstation_id.to_string())
    .bind(&request.observation.workspace_logical_name)
    .bind(&request.observation.workspace_logical_root)
    .bind(&request.observation.workspace_resolved_root)
    .bind(request.created_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    sqlx::query(
        "INSERT INTO conversations (conversation_id, craxii_id, kind, lifecycle_state, created_at, \
         next_work_ordinal, state_version) VALUES (?, ?, 'primary', 'active', ?, 1, 1)",
    )
    .bind(request.proposed.conversation_id.to_string())
    .bind(request.proposed.craxii_id.to_string())
    .bind(request.created_at.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)?;
    sqlx::query(
        "UPDATE craxii_principals SET primary_conversation_id = ?, default_workspace_id = ? \
         WHERE craxii_id = ? AND primary_conversation_id IS NULL AND default_workspace_id IS NULL",
    )
    .bind(request.proposed.conversation_id.to_string())
    .bind(request.proposed.workspace_id.to_string())
    .bind(request.proposed.craxii_id.to_string())
    .execute(transaction.connection())
    .await
    .map_err(SqliteAdapterError::from_sqlx)
    .and_then(|result| {
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(inconsistent())
        }
    })
}

async fn product_row_count(
    connection: &mut sqlx::SqliteConnection,
) -> Result<i64, SqliteAdapterError> {
    sqlx::query_scalar(
        "SELECT \
         (SELECT COUNT(*) FROM craxii_principals) + \
         (SELECT COUNT(*) FROM workstations) + \
         (SELECT COUNT(*) FROM workspaces) + \
         (SELECT COUNT(*) FROM conversations) + \
         (SELECT COUNT(*) FROM runtime_instances) + \
         (SELECT COUNT(*) FROM client_devices) + \
         (SELECT COUNT(*) FROM work_items) + \
         (SELECT COUNT(*) FROM messages) + \
         (SELECT COUNT(*) FROM client_commands) + \
         (SELECT COUNT(*) FROM journal_events) + \
         (SELECT COUNT(*) FROM stream_heads) + \
         (SELECT COUNT(*) FROM work_item_inputs)",
    )
    .fetch_one(connection)
    .await
    .map_err(SqliteAdapterError::from_sqlx)
}

async fn validate_existing_bootstrap_in_write(
    transaction: &mut WriteTransaction,
    request: &LoadOrBootstrapIdentityRequest,
) -> Result<V0IdentityReference, SqliteAdapterError> {
    validate_exact_root_counts(transaction.connection()).await?;
    let root = load_root_snapshot(transaction.connection()).await?;
    let expected = PreparedBootstrap::new(&LoadOrBootstrapIdentityRequest {
        proposed: V0IdentityReference {
            craxii_id: root.principal.craxii_id(),
            conversation_id: root.primary_conversation.conversation_id(),
            workstation_id: root.workstation.workstation_id(),
            workspace_id: root.workspace.workspace_id(),
        },
        initialized_event_id: request.initialized_event_id,
        conversation_created_event_id: request.conversation_created_event_id,
        correlation_id: request.correlation_id,
        created_at: root.principal.created_at(),
        observation: BootstrapObservation {
            initial_generation: request.observation.initial_generation,
            architecture: request.observation.architecture.clone(),
            os_release: request.observation.os_release.clone(),
            default_shell: request.observation.default_shell.clone(),
            workspace_logical_name: request.observation.workspace_logical_name.clone(),
            workspace_logical_root: request.observation.workspace_logical_root.clone(),
            workspace_resolved_root: request.observation.workspace_resolved_root.clone(),
            max_execution_timeout_ms: request.observation.max_execution_timeout_ms,
            max_stdout_bytes: request.observation.max_stdout_bytes,
            max_stderr_bytes: request.observation.max_stderr_bytes,
            administrative_enabled: request.observation.administrative_enabled,
        },
    })?;
    if root.workstation.generation() != request.observation.initial_generation
        || root.workstation.cpu_architecture() != request.observation.architecture
        || root.workstation.os_release() != request.observation.os_release
        || root.workspace.logical_name() != request.observation.workspace_logical_name
        || root.workspace.logical_root().canonical() != request.observation.workspace_logical_root
        || root.capabilities != decode_capabilities_json(&expected.capabilities_json)?
    {
        return Err(inconsistent());
    }
    let resolved_root: String =
        sqlx::query_scalar("SELECT local_resolved_root FROM workspaces WHERE workspace_id = ?")
            .bind(root.workspace.workspace_id().to_string())
            .fetch_one(transaction.connection())
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
    if resolved_root != request.observation.workspace_resolved_root {
        return Err(inconsistent());
    }
    Ok(V0IdentityReference {
        craxii_id: root.principal.craxii_id(),
        conversation_id: root.primary_conversation.conversation_id(),
        workstation_id: root.workstation.workstation_id(),
        workspace_id: root.workspace.workspace_id(),
    })
}

fn decode_capabilities_json(json: &str) -> Result<WorkstationCapabilities, SqliteAdapterError> {
    super::codec::decode_workstation_capabilities(json)
}

async fn validate_exact_root_counts(
    connection: &mut sqlx::SqliteConnection,
) -> Result<(), SqliteAdapterError> {
    for (table, expected) in [
        ("craxii_principals", 1_i64),
        ("workstations", 1),
        ("workspaces", 1),
        ("conversations", 1),
    ] {
        let statement = format!("SELECT COUNT(*) FROM {table}");
        let count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(statement))
            .fetch_one(&mut *connection)
            .await
            .map_err(SqliteAdapterError::from_sqlx)?;
        if count != expected {
            return Err(inconsistent());
        }
    }
    Ok(())
}

struct RootSnapshot {
    principal: CraxiiPrincipal,
    workstation: WorkstationIdentity,
    capabilities: WorkstationCapabilities,
    workspace: WorkspaceIdentity,
    primary_conversation: Conversation,
    capabilities_json: String,
}

async fn load_root_snapshot(
    connection: &mut sqlx::SqliteConnection,
) -> Result<RootSnapshot, SqliteAdapterError> {
    let principal_row = sqlx::query("SELECT * FROM craxii_principals")
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let craxii_id = CraxiiId::parse_canonical(&principal_row.try_get::<String, _>("craxii_id")?)
        .map_err(|_| inconsistent())?;
    let conversation_id = crate::domain::ConversationId::parse_canonical(
        &principal_row
            .try_get::<Option<String>, _>("primary_conversation_id")?
            .ok_or_else(inconsistent)?,
    )
    .map_err(|_| inconsistent())?;
    let workspace_id = crate::domain::WorkspaceId::parse_canonical(
        &principal_row
            .try_get::<Option<String>, _>("default_workspace_id")?
            .ok_or_else(inconsistent)?,
    )
    .map_err(|_| inconsistent())?;
    let created_at =
        UtcTimestamp::parse_canonical(&principal_row.try_get::<String, _>("created_at")?)
            .map_err(|_| inconsistent())?;
    let principal = CraxiiPrincipal::try_new(CraxiiPrincipalInput {
        craxii_id,
        display_name: principal_row.try_get("display_name")?,
        owner_label: principal_row.try_get("owner_label")?,
        primary_conversation_id: conversation_id,
        default_workspace_id: workspace_id,
        created_at,
        architecture_revision: principal_row.try_get("architecture_revision")?,
        schema_revision: SchemaVersion::try_new(principal_row.try_get("schema_revision")?)
            .map_err(|_| inconsistent())?,
    })
    .map_err(|_| inconsistent())?;

    let workstation_row = sqlx::query("SELECT * FROM workstations")
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let capabilities_json: String = workstation_row.try_get("capabilities_json")?;
    let decoded_workstation = decode_workstation_row(&workstation_row)?;
    let workstation = decoded_workstation.identity().clone();
    let capabilities = decoded_workstation.capabilities().clone();

    let workspace_row = sqlx::query("SELECT * FROM workspaces")
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let workspace = WorkspaceIdentity::try_new(WorkspaceIdentityInput {
        workspace_id: crate::domain::WorkspaceId::parse_canonical(
            &workspace_row.try_get::<String, _>("workspace_id")?,
        )
        .map_err(|_| inconsistent())?,
        craxii_id: CraxiiId::parse_canonical(&workspace_row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        workstation_id: crate::domain::WorkstationId::parse_canonical(
            &workspace_row.try_get::<String, _>("workstation_id")?,
        )
        .map_err(|_| inconsistent())?,
        logical_name: workspace_row.try_get("logical_name")?,
        logical_root: LogicalPathReference::absolute(
            workspace_row.try_get::<String, _>("logical_root")?,
        )
        .map_err(|_| inconsistent())?,
        created_at: UtcTimestamp::parse_canonical(
            &workspace_row.try_get::<String, _>("created_at")?,
        )
        .map_err(|_| inconsistent())?,
    })
    .map_err(|_| inconsistent())?;

    let conversation_row = sqlx::query("SELECT * FROM conversations")
        .fetch_one(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if conversation_row.try_get::<String, _>("kind")? != "primary"
        || conversation_row.try_get::<String, _>("lifecycle_state")? != "active"
    {
        return Err(inconsistent());
    }
    let primary_conversation = Conversation::new(
        crate::domain::ConversationId::parse_canonical(
            &conversation_row.try_get::<String, _>("conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        CraxiiId::parse_canonical(&conversation_row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?,
        UtcTimestamp::parse_canonical(&conversation_row.try_get::<String, _>("created_at")?)
            .map_err(|_| inconsistent())?,
        ConversationWorkOrdinal::try_new(conversation_row.try_get("next_work_ordinal")?)
            .map_err(|_| inconsistent())?,
        ProjectionVersion::try_new(conversation_row.try_get("state_version")?)
            .map_err(|_| inconsistent())?,
    );

    if principal.craxii_id() != workstation.craxii_id()
        || principal.craxii_id() != workspace.craxii_id()
        || principal.craxii_id() != primary_conversation.craxii_id()
        || principal.primary_conversation_id() != primary_conversation.conversation_id()
        || principal.default_workspace_id() != workspace.workspace_id()
        || workstation.workstation_id() != workspace.workstation_id()
        || !matches!(principal.schema_revision().get(), 2 | SCHEMA_REVISION)
    {
        return Err(inconsistent());
    }
    Ok(RootSnapshot {
        principal,
        workstation,
        capabilities,
        workspace,
        primary_conversation,
        capabilities_json,
    })
}

fn compare_root_projection(
    root: &RootSnapshot,
    projected: &crate::application::projector::ProjectedState,
    events: &[JournalEvent],
) -> Result<(), SqliteAdapterError> {
    if events.len() < 2 {
        return Err(inconsistent());
    }
    let initialized = projected.root.as_ref().ok_or_else(inconsistent)?;
    let conversation = projected
        .primary_conversation
        .as_ref()
        .ok_or_else(inconsistent)?;
    if initialized.craxii_id != root.principal.craxii_id()
        || initialized.display_name != root.principal.display_name()
        || initialized.owner_label != root.principal.owner_label()
        || initialized.architecture_revision != root.principal.architecture_revision()
        || initialized.schema_revision != root.principal.schema_revision()
        || initialized.workstation_id != root.workstation.workstation_id()
        || initialized.workstation_generation != root.workstation.generation()
        || initialized.workstation_architecture != root.workstation.cpu_architecture()
        || initialized.workstation_os_release != root.workstation.os_release()
        || initialized.capabilities_sha256
            != Sha256Digest::hash_bytes(root.capabilities_json.as_bytes())
        || initialized.workspace_id != root.workspace.workspace_id()
        || initialized.workspace_logical_name != root.workspace.logical_name()
        || initialized.workspace_logical_root != root.workspace.logical_root().canonical()
        || initialized.primary_conversation_id != root.primary_conversation.conversation_id()
        || initialized.created_at != root.principal.created_at()
        || initialized.created_at != root.workstation.created_at()
        || initialized.created_at != root.workspace.created_at()
        || conversation.conversation_id != root.primary_conversation.conversation_id()
        || conversation.craxii_id != root.primary_conversation.craxii_id()
        || conversation.kind != root.primary_conversation.kind()
        || conversation.lifecycle != root.primary_conversation.lifecycle()
        || conversation.next_work_ordinal != root.primary_conversation.next_work_ordinal()
        || conversation.state_version != root.primary_conversation.projection_version()
        || conversation.created_at != root.primary_conversation.created_at()
    {
        return Err(inconsistent());
    }
    Ok(())
}

async fn compare_message_projection(
    connection: &mut sqlx::SqliteConnection,
    projected: &crate::application::projector::ProjectedState,
) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query("SELECT * FROM messages")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    let projected_messages = projected.messages.values().flatten().collect::<Vec<_>>();
    if rows.len() != projected_messages.len() {
        return Err(inconsistent());
    }
    for row in rows {
        let stored = decode_message_row(&row)?;
        let event = projected_messages
            .iter()
            .find(|message| message.message.message_id == stored.message_id())
            .ok_or_else(inconsistent)?;
        let payload = &event.message;
        if payload.craxii_id != stored.craxii_id()
            || payload.conversation_id != stored.conversation_id()
            || payload.role != stored.role()
            || &payload.content != stored.content()
            || payload.content_sha256 != stored.content_sha256()
            || payload.produced_by_work_id != stored.produced_by_work_id()
            || payload.device_id != stored.device_id()
            || payload.client_message_id != stored.client_message_id()
            || payload.committed_at != stored.committed_at()
        {
            return Err(inconsistent());
        }
    }
    Ok(())
}

async fn compare_work_projection(
    connection: &mut sqlx::SqliteConnection,
    projected: &crate::application::projector::ProjectedState,
) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query("SELECT * FROM work_items")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if rows.len() != projected.works.len() {
        return Err(inconsistent());
    }
    for row in rows {
        let work_id = WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
            .map_err(|_| inconsistent())?;
        let work = projected.works.get(&work_id).ok_or_else(inconsistent)?;
        let current_model: Option<ModelInvocationId> = decode_optional_id(
            row.try_get::<Option<String>, _>("current_model_invocation_id")?
                .as_deref(),
        )?;
        let current_tool: Option<ToolExecutionId> = decode_optional_id(
            row.try_get::<Option<String>, _>("current_tool_execution_id")?
                .as_deref(),
        )?;
        let current_attempt = match (current_model, current_tool) {
            (None, None) => JournalCurrentAttempt::None,
            (Some(id), None) => JournalCurrentAttempt::Model(id),
            (None, Some(id)) => JournalCurrentAttempt::Tool(id),
            (Some(_), Some(_)) => return Err(inconsistent()),
        };
        let runtime_owner: Option<RuntimeInstanceId> = decode_optional_id(
            row.try_get::<Option<String>, _>("runtime_instance_id")?
                .as_deref(),
        )?;
        let cancellation_reason = row
            .try_get::<Option<String>, _>("cancellation_reason_code")?
            .map(|value| super::codec::decode_cancellation_reason(&value))
            .transpose()?;
        let terminal_reason = row
            .try_get::<Option<String>, _>("terminal_reason_code")?
            .map(|value| decode_journal_terminal_reason(&value))
            .transpose()?;
        let created = &work.created;
        if CraxiiId::parse_canonical(&row.try_get::<String, _>("craxii_id")?)
            .map_err(|_| inconsistent())?
            != created.craxii_id
            || crate::domain::ConversationId::parse_canonical(
                &row.try_get::<String, _>("conversation_id")?,
            )
            .map_err(|_| inconsistent())?
                != created.conversation_id
            || row.try_get::<i64, _>("conversation_work_ordinal")?
                != created.conversation_work_ordinal.get()
            || row.try_get::<String, _>("kind")? != "conversational"
            || created.kind != crate::domain::WorkKind::Conversational
            || row.try_get::<i64, _>("priority")? != created.priority
            || crate::domain::WorkspaceId::parse_canonical(
                &row.try_get::<String, _>("workspace_id")?,
            )
            .map_err(|_| inconsistent())?
                != created.workspace_id
            || crate::domain::CorrelationId::parse_canonical(
                &row.try_get::<String, _>("correlation_id")?,
            )
            .map_err(|_| inconsistent())?
                != created.correlation_id
            || decode_timestamp(&row.try_get::<String, _>("created_at")?)? != created.created_at
            || decode_timestamp(&row.try_get::<String, _>("queued_at")?)? != created.queued_at
            || decode_work_state(&row.try_get::<String, _>("state")?)? != work.state
            || ProjectionVersion::try_new(row.try_get::<i64, _>("state_version")?)
                .map_err(|_| inconsistent())?
                != work.state_version
            || runtime_owner != work.runtime_owner
            || current_attempt != work.current_attempt
            || cancellation_reason != work.cancellation_reason
            || terminal_reason != work.terminal_reason
            || decode_optional_timestamp(
                row.try_get::<Option<String>, _>("started_at")?.as_deref(),
            )? != work.started_at
            || decode_optional_timestamp(
                row.try_get::<Option<String>, _>("cancel_requested_at")?
                    .as_deref(),
            )? != work.cancel_requested_at
            || decode_optional_timestamp(
                row.try_get::<Option<String>, _>("terminal_at")?.as_deref(),
            )? != work.terminal_at
        {
            return Err(inconsistent());
        }
    }
    Ok(())
}

fn decode_journal_terminal_reason(
    value: &str,
) -> Result<crate::domain::JournalWorkTerminalReason, SqliteAdapterError> {
    use crate::domain::JournalWorkTerminalReason as Reason;
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

async fn compare_work_inputs(
    connection: &mut sqlx::SqliteConnection,
    projected: &crate::application::projector::ProjectedState,
) -> Result<(), SqliteAdapterError> {
    let rows = sqlx::query("SELECT * FROM work_item_inputs")
        .fetch_all(&mut *connection)
        .await
        .map_err(SqliteAdapterError::from_sqlx)?;
    if rows.len() != projected.works.len() {
        return Err(inconsistent());
    }
    for row in rows {
        let work_id = WorkId::parse_canonical(&row.try_get::<String, _>("work_id")?)
            .map_err(|_| inconsistent())?;
        let work = projected.works.get(&work_id).ok_or_else(inconsistent)?;
        let trigger = &work.created.trigger;
        if JournalEventId::parse_canonical(&row.try_get::<String, _>("input_event_id")?)
            .map_err(|_| inconsistent())?
            != trigger.input_event_id
            || row.try_get::<String, _>("relationship")? != "trigger"
            || row.try_get::<i64, _>("ordinal_within_work")? != trigger.ordinal_within_work.get()
            || decode_timestamp(&row.try_get::<String, _>("attached_at")?)? != trigger.attached_at
            || row.try_get::<String, _>("attached_by_actor")? != "user"
        {
            return Err(inconsistent());
        }
    }
    Ok(())
}
