use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::channel_topology::{ChannelTopologyErrorKind, ChannelTopologyService};
use crate::domain::{
    ChannelAccountId, ChannelAccountLifecycle, ChannelProviderId, ConversationId, CorrelationId,
    CraxiiId, ExternalAccountId, ExternalSubjectId, JournalEventId, UserId, UtcTimestamp,
    WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::channel_identity::{ChannelIdentityStore, DisableChannelAccountOutcome};
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ExecutionCapabilityObservation,
    LoadOrBootstrapIdentityRequest, V0IdentityReference,
};

use super::{SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-ch6-channel-admin-{}-{}",
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

fn at(value: &str) -> UtcTimestamp {
    value.parse().unwrap()
}

#[tokio::test]
async fn disable_is_durable_idempotent_and_existing_ensure_cannot_reactivate() {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let state_store = SqliteStateStore::new(guard.runtime().clone());
    let owner = state_store
        .load_or_bootstrap_v0_identity(LoadOrBootstrapIdentityRequest {
            proposed: V0IdentityReference {
                craxii_id: CraxiiId::generate(),
                user_id: UserId::generate(),
                conversation_id: ConversationId::generate(),
                workstation_id: WorkstationId::generate(),
                workspace_id: WorkspaceId::generate(),
            },
            initialized_event_id: JournalEventId::generate(),
            conversation_created_event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
            created_at: at("2026-09-18T00:00:00.000000Z"),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "synthetic".into(),
                os_release: "synthetic".into(),
                default_shell: "/bin/sh".into(),
                workspace_logical_name: "primary".into(),
                workspace_logical_root: "/synthetic/workspace".into(),
                workspace_resolved_root: "/synthetic/workspace".into(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;

    let account_id = ChannelAccountId::generate();
    let provider = ChannelProviderId::try_new("telegram").unwrap();
    let external_account = ExternalAccountId::try_new("synthetic-bot-id").unwrap();
    let external_subject = ExternalSubjectId::try_new("synthetic-owner-id").unwrap();
    let store = Arc::new(SqliteChannelIdentityStore::new(guard.runtime().clone()));
    let topology = ChannelTopologyService::new(Arc::clone(&store));
    topology
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            provider.clone(),
            external_account.clone(),
            owner.user_id,
            external_subject.clone(),
            at("2026-09-18T00:00:01.000000Z"),
        )
        .await
        .unwrap();

    let disabled_at = at("2026-09-18T00:00:02.000000Z");
    let first = store
        .disable_channel_account(account_id, disabled_at)
        .await
        .unwrap();
    assert!(matches!(
        first,
        DisableChannelAccountOutcome::Disabled(ref account)
            if account.lifecycle == ChannelAccountLifecycle::Disabled
                && account.disabled_at == Some(disabled_at)
    ));
    let repeated = store
        .disable_channel_account(account_id, at("2026-09-18T00:00:03.000000Z"))
        .await
        .unwrap();
    assert!(matches!(
        repeated,
        DisableChannelAccountOutcome::AlreadyDisabled(ref account)
            if account.lifecycle == ChannelAccountLifecycle::Disabled
                && account.disabled_at == Some(disabled_at)
    ));
    let loaded = store
        .load_channel_account(account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.lifecycle, ChannelAccountLifecycle::Disabled);
    assert_eq!(loaded.disabled_at, Some(disabled_at));

    let identities_before = {
        let mut connection = guard.runtime().acquire_for_test().await.unwrap();
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM external_identities")
            .fetch_one(&mut *connection)
            .await
            .unwrap()
    };
    let error = topology
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            provider,
            external_account,
            owner.user_id,
            external_subject,
            at("2026-09-18T00:00:04.000000Z"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ChannelTopologyErrorKind::Conflict);
    let mut connection = guard.runtime().acquire_for_test().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM external_identities")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        identities_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT lifecycle_state FROM channel_accounts WHERE channel_account_id = ?",
        )
        .bind(account_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        "disabled"
    );
    drop(connection);
    guard.shutdown().await;
}
