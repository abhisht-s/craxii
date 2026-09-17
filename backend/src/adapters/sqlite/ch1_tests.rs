use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::channel_topology::{ChannelTopologyErrorKind, ChannelTopologyService};
use crate::domain::{
    ChannelAccount, ChannelAccountId, ChannelAccountLifecycle, ChannelProviderId, ClientMessageId,
    ConversationBinding, ConversationBindingId, ConversationBindingLifecycle, ConversationId,
    CorrelationId, CraxiiId, ExternalAccountId, ExternalConversationId, ExternalEventId,
    ExternalIdentity, ExternalIdentityId, ExternalIdentityLifecycle, ExternalSubjectId,
    ExternalThreadId, InboundDeliveryId, JournalEventId, MessageId, UserId, UtcTimestamp, WorkId,
    WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::channel_identity::ChannelIdentityStore;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ExecutionCapabilityObservation,
    LoadOrBootstrapIdentityRequest, V0IdentityReference,
};
use sha2::{Digest as _, Sha256};

use super::{SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2026-09-16T01:02:03.000000Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-ch1-test-{}-{}",
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

fn at() -> UtcTimestamp {
    T0.parse().unwrap()
}

async fn fresh() -> (
    TestRoot,
    SqliteRuntimeGuard,
    SqliteStateStore,
    V0IdentityReference,
) {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 2).await.unwrap();
    let store = SqliteStateStore::new(guard.runtime().clone());
    let identity = store
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
            created_at: at(),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".to_owned(),
                os_release: "ch1-test".to_owned(),
                default_shell: "/bin/sh".to_owned(),
                workspace_logical_name: "primary".to_owned(),
                workspace_logical_root: "/workspace".to_owned(),
                workspace_resolved_root: "/workspace".to_owned(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    (root, guard, store, identity)
}

#[tokio::test]
async fn exact_topology_account_and_owner_identity_are_idempotent_and_conflicts_fail_closed() {
    let (_root, guard, _state, owner) = fresh().await;
    let service = ChannelTopologyService::new(Arc::new(SqliteChannelIdentityStore::new(
        guard.runtime().clone(),
    )));
    let account_id = ChannelAccountId::generate();
    let provider = ChannelProviderId::try_new("telegram").unwrap();
    let external_account = ExternalAccountId::try_new("10001").unwrap();
    let subject = ExternalSubjectId::try_new("20002").unwrap();
    let first = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            provider.clone(),
            external_account.clone(),
            owner.user_id,
            subject.clone(),
            at(),
        )
        .await
        .unwrap();
    let reused = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            provider.clone(),
            external_account.clone(),
            owner.user_id,
            subject.clone(),
            at(),
        )
        .await
        .unwrap();
    assert_eq!(first, reused);

    for result in [
        service
            .ensure_account_and_owner_identity(
                ChannelAccountId::generate(),
                owner.craxii_id,
                provider.clone(),
                external_account.clone(),
                owner.user_id,
                subject.clone(),
                at(),
            )
            .await,
        service
            .ensure_account_and_owner_identity(
                account_id,
                owner.craxii_id,
                provider,
                ExternalAccountId::try_new("different-bot").unwrap(),
                owner.user_id,
                subject,
                at(),
            )
            .await,
        service
            .ensure_account_and_owner_identity(
                account_id,
                CraxiiId::generate(),
                ChannelProviderId::try_new("telegram").unwrap(),
                external_account.clone(),
                owner.user_id,
                ExternalSubjectId::try_new("20002").unwrap(),
                at(),
            )
            .await,
        service
            .ensure_account_and_owner_identity(
                account_id,
                owner.craxii_id,
                ChannelProviderId::try_new("other-provider").unwrap(),
                external_account.clone(),
                owner.user_id,
                ExternalSubjectId::try_new("20002").unwrap(),
                at(),
            )
            .await,
    ] {
        assert_eq!(
            result.unwrap_err().kind(),
            ChannelTopologyErrorKind::Conflict
        );
    }

    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM channel_accounts")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM external_identities")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    sqlx::query(
        "UPDATE channel_accounts SET lifecycle_state = 'disabled', disabled_at = ? \
         WHERE channel_account_id = ?",
    )
    .bind(T0)
    .bind(account_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let error = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new("telegram").unwrap(),
            ExternalAccountId::try_new("10001").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("20002").unwrap(),
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ChannelTopologyErrorKind::Conflict);
    guard.shutdown().await;
}

#[tokio::test]
async fn exact_owner_identity_never_rebinds_or_reactivates() {
    let (_root, guard, _state, owner) = fresh().await;
    let service = ChannelTopologyService::new(Arc::new(SqliteChannelIdentityStore::new(
        guard.runtime().clone(),
    )));
    let account_id = ChannelAccountId::generate();
    let ensured = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new("telegram").unwrap(),
            ExternalAccountId::try_new("30003").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("40004").unwrap(),
            at(),
        )
        .await
        .unwrap();
    let other_user = UserId::generate();
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO users (user_id, craxii_id, lifecycle_state, created_at) \
         VALUES (?, ?, 'active', ?)",
    )
    .bind(other_user.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let wrong_user = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new("telegram").unwrap(),
            ExternalAccountId::try_new("30003").unwrap(),
            other_user,
            ExternalSubjectId::try_new("40004").unwrap(),
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_user.kind(), ChannelTopologyErrorKind::Conflict);

    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE external_identities SET lifecycle_state = 'revoked', revoked_at = ? \
         WHERE external_identity_id = ?",
    )
    .bind(T0)
    .bind(ensured.external_identity_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let revoked = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new("telegram").unwrap(),
            ExternalAccountId::try_new("30003").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("40004").unwrap(),
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(revoked.kind(), ChannelTopologyErrorKind::Conflict);
    guard.shutdown().await;
}

#[tokio::test]
async fn first_binding_is_race_safe_exact_and_never_rebinds_or_reactivates() {
    let (_root, guard, _state, owner) = fresh().await;
    let service = Arc::new(ChannelTopologyService::new(Arc::new(
        SqliteChannelIdentityStore::new(guard.runtime().clone()),
    )));
    let account_id = ChannelAccountId::generate();
    let ensured = service
        .ensure_account_and_owner_identity(
            account_id,
            owner.craxii_id,
            ChannelProviderId::try_new("telegram").unwrap(),
            ExternalAccountId::try_new("50005").unwrap(),
            owner.user_id,
            ExternalSubjectId::try_new("60006").unwrap(),
            at(),
        )
        .await
        .unwrap();
    let destination = ExternalConversationId::try_new("60006").unwrap();
    let left = {
        let service = Arc::clone(&service);
        let destination = destination.clone();
        tokio::spawn(async move {
            service
                .ensure_first_binding(
                    account_id,
                    ensured.external_identity_id,
                    owner.craxii_id,
                    owner.user_id,
                    owner.conversation_id,
                    destination,
                    at(),
                )
                .await
        })
    };
    let right = {
        let service = Arc::clone(&service);
        let destination = destination.clone();
        tokio::spawn(async move {
            service
                .ensure_first_binding(
                    account_id,
                    ensured.external_identity_id,
                    owner.craxii_id,
                    owner.user_id,
                    owner.conversation_id,
                    destination,
                    at(),
                )
                .await
        })
    };
    let first = left.await.unwrap().unwrap();
    let second = right.await.unwrap().unwrap();
    assert_eq!(first, second);

    let conflict = service
        .ensure_first_binding(
            account_id,
            ensured.external_identity_id,
            owner.craxii_id,
            owner.user_id,
            owner.conversation_id,
            ExternalConversationId::try_new("different-chat").unwrap(),
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.kind(), ChannelTopologyErrorKind::Conflict);
    for (user_id, conversation_id) in [
        (UserId::generate(), owner.conversation_id),
        (owner.user_id, ConversationId::generate()),
    ] {
        let error = service
            .ensure_first_binding(
                account_id,
                ensured.external_identity_id,
                owner.craxii_id,
                user_id,
                conversation_id,
                destination.clone(),
                at(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ChannelTopologyErrorKind::Conflict);
    }

    let identity_store = SqliteChannelIdentityStore::new(guard.runtime().clone());
    let second_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: account_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("70007").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(second_identity.clone())
        .await
        .unwrap();
    let destination_owned = service
        .ensure_first_binding(
            account_id,
            second_identity.external_identity_id,
            owner.craxii_id,
            owner.user_id,
            owner.conversation_id,
            destination.clone(),
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(destination_owned.kind(), ChannelTopologyErrorKind::Conflict);

    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversations")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversation_bindings")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    sqlx::query(
        "UPDATE conversation_bindings SET lifecycle_state = 'revoked', revoked_at = ? \
         WHERE conversation_binding_id = ?",
    )
    .bind(T0)
    .bind(first.conversation_binding_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let revoked = service
        .ensure_first_binding(
            account_id,
            ensured.external_identity_id,
            owner.craxii_id,
            owner.user_id,
            owner.conversation_id,
            destination,
            at(),
        )
        .await
        .unwrap_err();
    assert_eq!(revoked.kind(), ChannelTopologyErrorKind::Conflict);
    guard.shutdown().await;
}

#[tokio::test]
async fn fresh_v6_bootstrap_has_one_distinct_owner_and_v2_conversation_evidence() {
    let (_root, guard, store, identity) = fresh().await;
    assert_ne!(identity.user_id.to_string(), identity.craxii_id.to_string());
    assert_ne!(
        identity.user_id.to_string(),
        identity.conversation_id.to_string()
    );

    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
    assert_eq!(snapshot.identity, identity);
    assert_eq!(
        snapshot.primary_conversation.owner_user_id(),
        identity.user_id
    );
    assert_eq!(snapshot.principal.schema_revision().get(), 5);

    let mut connection = guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_as::<_, (String, i64)>(
            "SELECT event_type, event_version FROM journal_events \
             WHERE event_type = 'conversation.created'"
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        ("conversation.created".to_owned(), 2)
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .is_empty()
    );
    drop(connection);
    guard.shutdown().await;
}

#[tokio::test]
async fn provider_neutral_channel_foundation_roundtrips_and_fails_closed() {
    let (_root, guard, _state, owner) = fresh().await;
    let store = SqliteChannelIdentityStore::new(guard.runtime().clone());
    let account = ChannelAccount {
        channel_account_id: ChannelAccountId::generate(),
        craxii_id: owner.craxii_id,
        provider_id: ChannelProviderId::try_new("future.adapter-1").unwrap(),
        external_account_id: ExternalAccountId::try_new("account opaque α").unwrap(),
        lifecycle: ChannelAccountLifecycle::Active,
        created_at: at(),
        disabled_at: None,
    };
    store
        .persist_channel_account(account.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .load_channel_account(account.channel_account_id)
            .await
            .unwrap(),
        Some(account.clone())
    );

    let identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: account.channel_account_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("subject opaque β").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(),
        revoked_at: None,
    };
    store
        .persist_external_identity(identity.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .load_external_identity(identity.external_identity_id)
            .await
            .unwrap(),
        Some(identity.clone())
    );

    let binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: account.channel_account_id,
        external_identity_id: identity.external_identity_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        conversation_id: owner.conversation_id,
        external_conversation_id: ExternalConversationId::try_new("destination opaque γ").unwrap(),
        external_thread_id: Some(ExternalThreadId::try_new("thread opaque δ").unwrap()),
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(),
        revoked_at: None,
    };
    store
        .persist_conversation_binding(binding.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .load_conversation_binding(binding.conversation_binding_id)
            .await
            .unwrap(),
        Some(binding.clone())
    );

    let delivery_id = InboundDeliveryId::generate();
    let delivery = crate::domain::InboundDelivery {
        inbound_delivery_id: delivery_id,
        channel_account_id: account.channel_account_id,
        craxii_id: owner.craxii_id,
        external_event_id: ExternalEventId::try_new("event opaque ε").unwrap(),
        external_message_id: None,
        external_subject_id: identity.external_subject_id.clone(),
        external_conversation_id: binding.external_conversation_id.clone(),
        external_thread_id: binding.external_thread_id.clone(),
        material_sha256: crate::domain::Sha256Digest::hash_bytes(b"normalized material"),
        provider_occurred_at: None,
        received_at: at(),
        receipt_state: crate::domain::InboundReceiptState::Received,
        classification: None,
        external_identity_id: None,
        conversation_binding_id: None,
        user_id: None,
        conversation_id: None,
        message_id: None,
        work_id: None,
        control_target_work_id: None,
        classified_at: None,
    };
    store
        .persist_inbound_delivery(delivery.clone())
        .await
        .unwrap();
    assert_eq!(
        store.load_inbound_delivery(delivery_id).await.unwrap(),
        Some(delivery)
    );

    let classified_delivery_id = InboundDeliveryId::generate();
    store
        .persist_inbound_delivery(crate::domain::InboundDelivery {
            inbound_delivery_id: classified_delivery_id,
            channel_account_id: account.channel_account_id,
            craxii_id: owner.craxii_id,
            external_event_id: ExternalEventId::try_new("classified event").unwrap(),
            external_message_id: None,
            external_subject_id: identity.external_subject_id.clone(),
            external_conversation_id: binding.external_conversation_id.clone(),
            external_thread_id: binding.external_thread_id.clone(),
            material_sha256: crate::domain::Sha256Digest::hash_bytes(b"classified material"),
            provider_occurred_at: None,
            received_at: at(),
            receipt_state: crate::domain::InboundReceiptState::Classified,
            classification: Some(crate::domain::InboundClassification::Control),
            external_identity_id: Some(identity.external_identity_id),
            conversation_binding_id: Some(binding.conversation_binding_id),
            user_id: Some(owner.user_id),
            conversation_id: Some(owner.conversation_id),
            message_id: None,
            work_id: None,
            control_target_work_id: None,
            classified_at: Some(at()),
        })
        .await
        .unwrap();
    _state.verify_application_consistency().await.unwrap();

    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE inbound_deliveries SET external_subject_id = 'wrong-subject' \
         WHERE inbound_delivery_id = ?",
    )
    .bind(classified_delivery_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert!(_state.verify_application_consistency().await.is_err());
    let mut connection = guard.runtime().acquire().await.unwrap();
    sqlx::query(
        "UPDATE inbound_deliveries SET external_subject_id = ? WHERE inbound_delivery_id = ?",
    )
    .bind(identity.external_subject_id.as_str())
    .bind(classified_delivery_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();

    let second_user = UserId::generate();
    let second_conversation = ConversationId::generate();
    sqlx::query(
        "INSERT INTO users (user_id, craxii_id, lifecycle_state, created_at) \
         VALUES (?, ?, 'active', ?)",
    )
    .bind(second_user.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    let duplicate = sqlx::query(
        "INSERT INTO conversations \
         (conversation_id, craxii_id, owner_user_id, kind, lifecycle_state, \
          next_work_ordinal, state_version, created_at) \
         VALUES (?, ?, ?, 'primary', 'active', 1, 1, ?)",
    )
    .bind(ConversationId::generate().to_string())
    .bind(owner.craxii_id.to_string())
    .bind(owner.user_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await;
    assert!(duplicate.is_err());
    sqlx::query(
        "INSERT INTO conversations \
         (conversation_id, craxii_id, owner_user_id, kind, lifecycle_state, \
          next_work_ordinal, state_version, created_at) \
         VALUES (?, ?, ?, 'primary', 'active', 1, 1, ?)",
    )
    .bind(second_conversation.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(second_user.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    let mismatched = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        conversation_id: second_conversation,
        ..binding
    };
    assert!(
        store
            .persist_conversation_binding(mismatched)
            .await
            .is_err()
    );
    guard.shutdown().await;
}

#[tokio::test]
async fn channel_message_provenance_uses_inbound_delivery_without_native_identity() {
    let (_root, guard, _state, owner) = fresh().await;
    let mut connection = guard.runtime().acquire().await.unwrap();
    let account_id = ChannelAccountId::generate();
    let external_identity_id = ExternalIdentityId::generate();
    let binding_id = ConversationBindingId::generate();
    let delivery_id = InboundDeliveryId::generate();
    let message_id = MessageId::generate();
    let work_id = WorkId::generate();

    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA defer_foreign_keys = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_accounts VALUES (?, ?, 'future', 'account', 'active', ?, NULL)",
    )
    .bind(account_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO external_identities VALUES (?, ?, ?, ?, 'subject', 'active', ?, NULL)",
    )
    .bind(external_identity_id.to_string())
    .bind(account_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(owner.user_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversation_bindings VALUES \
         (?, ?, ?, ?, ?, ?, 'destination', NULL, 'active', ?, NULL)",
    )
    .bind(binding_id.to_string())
    .bind(account_id.to_string())
    .bind(external_identity_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(owner.user_id.to_string())
    .bind(owner.conversation_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO work_items \
         (work_id, craxii_id, conversation_id, conversation_work_ordinal, kind, state, \
          state_version, priority, workspace_id, correlation_id, created_at, queued_at, \
          reply_binding_id) \
         VALUES (?, ?, ?, 1, 'conversational', 'queued', 1, 0, ?, ?, ?, ?, ?)",
    )
    .bind(work_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(owner.conversation_id.to_string())
    .bind(owner.workspace_id.to_string())
    .bind(CorrelationId::for_work(work_id).to_string())
    .bind(T0)
    .bind(T0)
    .bind(binding_id.to_string())
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO inbound_deliveries \
         (inbound_delivery_id, channel_account_id, craxii_id, external_event_id, \
          external_message_id, external_subject_id, external_conversation_id, external_thread_id, \
          material_sha256, provider_occurred_at, received_at, receipt_state, classification, \
          external_identity_id, conversation_binding_id, user_id, conversation_id, message_id, \
          work_id, control_target_work_id, classified_at) \
         VALUES (?, ?, ?, 'event', 'external-message', 'subject', 'destination', NULL, ?, NULL, ?, \
                 'classified', 'message', ?, ?, ?, ?, ?, ?, NULL, ?)",
    )
    .bind(delivery_id.to_string())
    .bind(account_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(crate::domain::Sha256Digest::hash_bytes(b"material").to_string())
    .bind(T0)
    .bind(external_identity_id.to_string())
    .bind(binding_id.to_string())
    .bind(owner.user_id.to_string())
    .bind(owner.conversation_id.to_string())
    .bind(message_id.to_string())
    .bind(work_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    let content = crate::domain::MessageContent::try_new(vec![
        crate::domain::ContentBlock::text("from channel").unwrap(),
    ])
    .unwrap();
    let (content_json, content_sha256) = super::codec::encode_message_content(&content).unwrap();
    sqlx::query(
        "INSERT INTO messages \
         (message_id, craxii_id, conversation_id, role, content_json, content_sha256, \
          author_user_id, produced_by_work_id, client_device_id, client_message_id, \
          inbound_delivery_id, committed_at) \
         VALUES (?, ?, ?, 'user', ?, ?, ?, NULL, NULL, NULL, ?, ?)",
    )
    .bind(message_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(owner.conversation_id.to_string())
    .bind(content_json.clone())
    .bind(content_sha256.to_string())
    .bind(owner.user_id.to_string())
    .bind(delivery_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .unwrap();

    let row = sqlx::query("SELECT * FROM messages WHERE message_id = ?")
        .bind(message_id.to_string())
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let decoded = super::codec::decode_message_row(&row).unwrap();
    assert_eq!(decoded.author_user_id(), Some(owner.user_id));
    assert_eq!(decoded.inbound_delivery_id(), Some(delivery_id));
    assert_eq!(decoded.device_id(), None);
    assert_eq!(decoded.client_message_id(), None);
    super::state_store::verify_reply_binding_route(
        &mut connection,
        binding_id,
        owner.conversation_id,
    )
    .await
    .unwrap();

    let second_user_id = UserId::generate();
    let second_conversation_id = ConversationId::generate();
    let second_identity_id = ExternalIdentityId::generate();
    let second_binding_id = ConversationBindingId::generate();
    sqlx::query(
        "INSERT INTO users (user_id, craxii_id, lifecycle_state, created_at) \
         VALUES (?, ?, 'active', ?)",
    )
    .bind(second_user_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversations \
         (conversation_id, craxii_id, owner_user_id, kind, lifecycle_state, \
          next_work_ordinal, state_version, created_at) \
         VALUES (?, ?, ?, 'primary', 'active', 1, 1, ?)",
    )
    .bind(second_conversation_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(second_user_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO external_identities VALUES (?, ?, ?, ?, 'second-subject', 'active', ?, NULL)",
    )
    .bind(second_identity_id.to_string())
    .bind(account_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(second_user_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO conversation_bindings VALUES \
         (?, ?, ?, ?, ?, ?, 'second-destination', NULL, 'active', ?, NULL)",
    )
    .bind(second_binding_id.to_string())
    .bind(account_id.to_string())
    .bind(second_identity_id.to_string())
    .bind(owner.craxii_id.to_string())
    .bind(second_user_id.to_string())
    .bind(second_conversation_id.to_string())
    .bind(T0)
    .execute(&mut *connection)
    .await
    .unwrap();
    assert!(
        super::state_store::verify_reply_binding_route(
            &mut connection,
            second_binding_id,
            owner.conversation_id,
        )
        .await
        .is_err()
    );

    for (device, client, inbound) in [
        (Some(crate::domain::DeviceId::generate()), None, None),
        (
            Some(crate::domain::DeviceId::generate()),
            Some(
                ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string())
                    .unwrap(),
            ),
            Some(delivery_id),
        ),
    ] {
        let result = sqlx::query(
            "INSERT INTO messages \
             (message_id, craxii_id, conversation_id, role, content_json, content_sha256, \
              author_user_id, produced_by_work_id, client_device_id, client_message_id, \
              inbound_delivery_id, committed_at) \
             VALUES (?, ?, ?, 'user', ?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(MessageId::generate().to_string())
        .bind(owner.craxii_id.to_string())
        .bind(owner.conversation_id.to_string())
        .bind(&content_json)
        .bind(content_sha256.to_string())
        .bind(owner.user_id.to_string())
        .bind(device.map(|id| id.to_string()))
        .bind(client.map(|id| id.to_string()))
        .bind(inbound.map(|id| id.to_string()))
        .bind(T0)
        .execute(&mut *connection)
        .await;
        assert!(result.is_err());
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .is_empty()
    );
    drop(connection);
    guard.shutdown().await;
}

#[test]
fn historical_migration_hashes_and_generic_source_boundary_are_frozen() {
    let migrations = [
        (
            include_bytes!("../../../migrations/0001_core_durable_schema.sql").as_slice(),
            "a66da57b131dadd27af13fc192d2d25827cfd878e796611b94c868c548bd1e33",
        ),
        (
            include_bytes!("../../../migrations/0002_journal_and_work_inputs.sql").as_slice(),
            "1cf3dea391bdcc363e43d3f92e6aa21540493d0b5f96e886298439659674d296",
        ),
        (
            include_bytes!("../../../migrations/0003_context_model_tool_artifacts.sql").as_slice(),
            "e60437dd6ce04a34ee40ba194b58f3aff58341bcaa71bb5842895c56cb1f22ee",
        ),
        (
            include_bytes!("../../../migrations/0004_model_attempt_outcome_evidence.sql")
                .as_slice(),
            "b427d5540333bc2302306d38ced968455f6d953347f1e3309a32061a27a51aaa",
        ),
        (
            include_bytes!("../../../migrations/0005_tool_terminal_outcome_evidence.sql")
                .as_slice(),
            "808a4ff7020632fc109a982a081936b93dcf0a2bda12ca6175422d46fbceb886",
        ),
    ];
    for (bytes, expected) in migrations {
        let actual = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(actual, expected);
    }

    let generic_sources = [
        include_str!("../../domain/channel.rs"),
        include_str!("../../ports/channel_identity.rs"),
        include_str!("channel_identity.rs"),
    ]
    .join("\n")
    .to_ascii_lowercase();
    for forbidden in [
        "update_id",
        "chat_id",
        "whatsapp",
        "wamid",
        "wa_id",
        "slack event",
        "discord gateway",
    ] {
        assert!(!generic_sources.contains(forbidden), "{forbidden}");
    }
}
