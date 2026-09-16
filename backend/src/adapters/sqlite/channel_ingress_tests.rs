use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sqlx::Row as _;

use crate::application::channel_ingress::{
    ChannelIngressErrorKind, ChannelIngressService, DurableInboundOutcome,
    InboundAdmissionDisposition, UnsupportedInboundKind, VerifiedInboundEvent,
    VerifiedInboundPayload,
};
use crate::application::command_service::CommandPostCommit;
use crate::application::transport::MutationAdmission;
use crate::bootstrap::health::Health;
use crate::domain::{
    ChannelAccount, ChannelAccountId, ChannelAccountLifecycle, ChannelProviderId, ContentBlock,
    ConversationBinding, ConversationBindingId, ConversationBindingLifecycle, ConversationId,
    CorrelationId, CraxiiId, DiagnosticPid, ExternalAccountId, ExternalConversationId,
    ExternalEventId, ExternalIdentity, ExternalIdentityId, ExternalIdentityLifecycle,
    ExternalMessageId, ExternalSubjectId, InboundDelivery, InboundDeliveryId, InboundReceiptState,
    JournalActor, JournalEventId, JournalEventPayload, LinuxBootId, MessageAcceptedOriginV2,
    MessageContent, PackageVersion, RuntimeInstanceId, RuntimeStartEvidence,
    RuntimeStartEvidenceInput, SchemaVersion, UserId, UtcTimestamp, WorkId, WorkspaceId,
    WorkstationGeneration, WorkstationId,
};
use crate::ports::channel_identity::ChannelIdentityStore;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ClaimNextWorkRequest, CreateRuntimeRequest,
    ExecutionCapabilityObservation, LoadOrBootstrapIdentityRequest, RuntimeStateStore,
    SchedulerStateStore, V0IdentityReference,
};

use super::ch3_test_support::create_conversation;
use super::journal::load_global_events;
use super::{SqliteChannelIdentityStore, SqliteRuntimeGuard, SqliteStateStore};

const T0: &str = "2026-09-16T01:02:03.000000Z";
const T1: &str = "2026-09-16T01:02:04.000000Z";

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "craxii-ch2-test-{}-{}",
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

fn text(value: &str) -> MessageContent {
    MessageContent::try_new(vec![ContentBlock::text(value).unwrap()]).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectKind {
    Message,
    ActiveCancellation,
    DirectCancellation,
}

#[derive(Clone, Default)]
struct EffectRecorder(Arc<Mutex<Vec<(EffectKind, WorkId, i64)>>>);

impl EffectRecorder {
    fn snapshot(&self) -> Vec<(EffectKind, WorkId, i64)> {
        self.0.lock().unwrap().clone()
    }
}

impl CommandPostCommit for EffectRecorder {
    fn message_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::Message, work_id, cursor.get()));
    }

    fn active_cancellation_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::ActiveCancellation, work_id, cursor.get()));
    }

    fn direct_cancellation_committed(&self, work_id: WorkId, cursor: crate::domain::JournalOffset) {
        self.0
            .lock()
            .unwrap()
            .push((EffectKind::DirectCancellation, work_id, cursor.get()));
    }
}

struct Fixture {
    _root: TestRoot,
    guard: SqliteRuntimeGuard,
    store: Arc<SqliteStateStore>,
    owner: V0IdentityReference,
    account: ChannelAccount,
    identity: ExternalIdentity,
    binding: ConversationBinding,
    effects: EffectRecorder,
}

impl Fixture {
    fn service(&self) -> ChannelIngressService<SqliteStateStore, EffectRecorder> {
        let health = Health::new();
        health.mark_ready().unwrap();
        ChannelIngressService::new(
            Arc::clone(&self.store),
            health,
            MutationAdmission::new(),
            self.effects.clone(),
        )
    }

    fn event(&self, event_id: &str, message_id: &str, value: &str) -> VerifiedInboundEvent {
        VerifiedInboundEvent::new(
            self.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            Some(ExternalMessageId::try_new(message_id).unwrap()),
            self.identity.external_subject_id.clone(),
            self.binding.external_conversation_id.clone(),
            self.binding.external_thread_id.clone(),
            VerifiedInboundPayload::Text(text(value)),
            Some(at(T0)),
            at(T1),
        )
        .unwrap()
    }

    fn unsupported(&self, event_id: &str) -> VerifiedInboundEvent {
        VerifiedInboundEvent::new(
            self.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            None,
            self.identity.external_subject_id.clone(),
            self.binding.external_conversation_id.clone(),
            self.binding.external_thread_id.clone(),
            VerifiedInboundPayload::Unsupported(UnsupportedInboundKind::NonText),
            None,
            at(T1),
        )
        .unwrap()
    }
}

async fn fixture() -> Fixture {
    let root = TestRoot::new();
    let guard = SqliteRuntimeGuard::start(root.path(), 4).await.unwrap();
    let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
    let owner = store
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
            created_at: at(T0),
            observation: BootstrapObservation {
                initial_generation: WorkstationGeneration::try_new(1).unwrap(),
                architecture: "aarch64".into(),
                os_release: "ch2-test".into(),
                default_shell: "/bin/sh".into(),
                workspace_logical_name: "primary".into(),
                workspace_logical_root: "/workspace".into(),
                workspace_resolved_root: "/workspace".into(),
                execution_capabilities: ExecutionCapabilityObservation::unavailable(),
            },
        })
        .await
        .unwrap()
        .identity;
    let identity_store = SqliteChannelIdentityStore::new(guard.runtime().clone());
    let account = ChannelAccount {
        channel_account_id: ChannelAccountId::generate(),
        craxii_id: owner.craxii_id,
        provider_id: ChannelProviderId::try_new("test.adapter").unwrap(),
        external_account_id: ExternalAccountId::try_new("owner-account").unwrap(),
        lifecycle: ChannelAccountLifecycle::Active,
        created_at: at(T0),
        disabled_at: None,
    };
    identity_store
        .persist_channel_account(account.clone())
        .await
        .unwrap();
    let identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: account.channel_account_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("owner-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(identity.clone())
        .await
        .unwrap();
    let binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: account.channel_account_id,
        external_identity_id: identity.external_identity_id,
        craxii_id: owner.craxii_id,
        user_id: owner.user_id,
        conversation_id: owner.conversation_id,
        external_conversation_id: ExternalConversationId::try_new("owner-conversation").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(binding.clone())
        .await
        .unwrap();
    Fixture {
        _root: root,
        guard,
        store,
        owner,
        account,
        identity,
        binding,
        effects: EffectRecorder::default(),
    }
}

async fn table_count(fixture: &Fixture, table: &str) -> i64 {
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

async fn create_runtime(fixture: &Fixture) -> RuntimeInstanceId {
    let runtime_id = RuntimeInstanceId::generate();
    fixture
        .store
        .create_runtime_and_started_event(CreateRuntimeRequest {
            evidence: RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: fixture.owner.craxii_id,
                workstation_id: fixture.owner.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("ch2-test-boot").unwrap()),
                diagnostic_pid: Some(DiagnosticPid::try_new(42).unwrap()),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: crate::domain::GitRevision::try_new("ch2-test").unwrap(),
                schema_version: SchemaVersion::try_new(6).unwrap(),
                started_at: at(T1),
            }),
            event_id: JournalEventId::generate(),
            correlation_id: CorrelationId::generate(),
        })
        .await
        .unwrap();
    runtime_id
}

#[tokio::test]
async fn message_admission_is_atomic_canonical_and_duplicate_safe() {
    let fixture = fixture().await;
    let service = fixture.service();
    let original = fixture.event("event-1", "message-1", "  exact é  ");
    let first = service.classify(original.clone()).await.unwrap();
    assert_eq!(
        first.disposition,
        InboundAdmissionDisposition::NewlyClassified
    );
    assert!(first.acknowledgement_safe());
    let DurableInboundOutcome::MessageAccepted {
        inbound_delivery_id,
        message_id,
        work_id,
        work_ordinal,
        committed_cursor,
    } = first.outcome
    else {
        panic!("message outcome expected");
    };
    assert_eq!(work_ordinal.get(), 1);
    assert_eq!(table_count(&fixture, "inbound_deliveries").await, 1);
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 1);
    assert_eq!(
        fixture.effects.snapshot(),
        vec![(EffectKind::Message, work_id, committed_cursor.get())]
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let row = sqlx::query(
        "SELECT d.classification, d.message_id, d.work_id, m.content_json, \
                m.author_user_id, m.inbound_delivery_id, w.reply_binding_id \
         FROM inbound_deliveries d JOIN messages m ON m.message_id = d.message_id \
         JOIN work_items w ON w.work_id = d.work_id WHERE d.inbound_delivery_id = ?",
    )
    .bind(inbound_delivery_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<String, _>("classification").unwrap(),
        "message"
    );
    assert_eq!(
        row.try_get::<String, _>("message_id").unwrap(),
        message_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("work_id").unwrap(),
        work_id.to_string()
    );
    assert!(
        row.try_get::<String, _>("content_json")
            .unwrap()
            .contains("  exact é  ")
    );
    assert_eq!(
        row.try_get::<String, _>("author_user_id").unwrap(),
        fixture.owner.user_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("inbound_delivery_id").unwrap(),
        inbound_delivery_id.to_string()
    );
    assert_eq!(
        row.try_get::<String, _>("reply_binding_id").unwrap(),
        fixture.binding.conversation_binding_id.to_string()
    );
    drop(connection);

    let events = load_global_events(fixture.guard.runtime()).await.unwrap();
    assert!(events.iter().any(|event| {
        event.actor == JournalActor::UserV2(fixture.owner.user_id)
            && matches!(
                &event.payload,
                JournalEventPayload::MessageAcceptedV2(payload)
                    if payload.message_id == message_id
                        && payload.origin == MessageAcceptedOriginV2::InboundDelivery {
                            inbound_delivery_id
                        }
            )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload,
            JournalEventPayload::WorkQueuedV2(payload)
                if payload.work_id == work_id
                    && payload.reply_binding_id == Some(fixture.binding.conversation_binding_id)
        )
    }));

    let duplicate = service.classify(original).await.unwrap();
    assert_eq!(
        duplicate.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(duplicate.outcome, first.outcome);
    let secondary = service
        .classify(fixture.event("event-2", "message-1", "  exact é  "))
        .await
        .unwrap();
    assert_eq!(
        secondary.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(secondary.outcome, first.outcome);
    assert_eq!(fixture.effects.snapshot().len(), 1);
    assert_eq!(table_count(&fixture, "messages").await, 1);

    let conflict = service
        .classify(fixture.event("event-1", "message-1", "changed"))
        .await
        .unwrap_err();
    assert_eq!(
        conflict.kind(),
        ChannelIngressErrorKind::ContradictoryReplay
    );
    assert!(!conflict.acknowledgement_safe());
    let secondary_conflict = service
        .classify(fixture.event("event-3", "message-1", "changed"))
        .await
        .unwrap_err();
    assert_eq!(
        secondary_conflict.kind(),
        ChannelIngressErrorKind::ContradictoryReplay
    );

    assert_eq!(
        service
            .classify(fixture.event("event-real-2", "message-real-2", "second"))
            .await
            .unwrap()
            .disposition,
        InboundAdmissionDisposition::NewlyClassified
    );
    let split_keys = service
        .classify(fixture.event("event-1", "message-real-2", "second"))
        .await
        .unwrap_err();
    assert_eq!(
        split_keys.kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );
    assert_eq!(table_count(&fixture, "messages").await, 2);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn queued_control_is_v2_eventful_and_duplicate_cannot_cancel_next_work() {
    let fixture = fixture().await;
    let service = fixture.service();
    let accepted = service
        .classify(fixture.event("event-message-1", "message-1", "ordinary"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted { work_id, .. } = accepted.outcome else {
        panic!("message outcome expected");
    };
    let control_event = fixture.event("event-control-1", "message-control-1", "  /CaNcEl  ");
    let control = service.classify(control_event.clone()).await.unwrap();
    let DurableInboundOutcome::ControlApplied {
        inbound_delivery_id,
        target_work_id,
    } = control.outcome
    else {
        panic!("control outcome expected");
    };
    assert_eq!(target_work_id, work_id);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(work_id.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "cancelled"
    );
    drop(connection);
    let events = load_global_events(fixture.guard.runtime()).await.unwrap();
    assert!(events.iter().any(|event| {
        event.actor == JournalActor::UserV2(fixture.owner.user_id)
            && matches!(
                &event.payload,
                JournalEventPayload::WorkCancelledV2(payload)
                    if payload.inbound_delivery_id == inbound_delivery_id
                        && payload.transition.work_id == work_id
            )
    }));
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 1);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::DirectCancellation
    );

    let second = service
        .classify(fixture.event("event-message-2", "message-2", "next"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: second_work,
        ..
    } = second.outcome
    else {
        panic!("message outcome expected");
    };
    let effect_count = fixture.effects.snapshot().len();
    let duplicate = service.classify(control_event).await.unwrap();
    assert_eq!(
        duplicate.disposition,
        InboundAdmissionDisposition::Duplicate
    );
    assert_eq!(duplicate.outcome, control.outcome);
    assert_eq!(fixture.effects.snapshot().len(), effect_count);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(second_work.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "queued"
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn active_control_precedes_queued_and_already_requested_is_eventless() {
    let fixture = fixture().await;
    let service = fixture.service();
    let first = service
        .classify(fixture.event("active-message", "active-message", "first"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: active_work,
        ..
    } = first.outcome
    else {
        panic!("message outcome expected");
    };
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), active_work);
    let second = service
        .classify(fixture.event("queued-message", "queued-message", "second"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: queued_work,
        ..
    } = second.outcome
    else {
        panic!("message outcome expected");
    };

    let first_control = service
        .classify(fixture.event("active-control", "active-control", "STOP"))
        .await
        .unwrap();
    let DurableInboundOutcome::ControlApplied { target_work_id, .. } = first_control.outcome else {
        panic!("control outcome expected");
    };
    assert_eq!(target_work_id, active_work);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::ActiveCancellation
    );
    let journal_count = table_count(&fixture, "journal_events").await;
    let effect_count = fixture.effects.snapshot().len();

    let second_control = service
        .classify(fixture.event("second-control", "second-control", "cancel"))
        .await
        .unwrap();
    assert_eq!(
        second_control.outcome,
        DurableInboundOutcome::ControlApplied {
            inbound_delivery_id: match second_control.outcome {
                DurableInboundOutcome::ControlApplied {
                    inbound_delivery_id,
                    ..
                } => inbound_delivery_id,
                _ => unreachable!(),
            },
            target_work_id: active_work,
        }
    );
    assert_eq!(table_count(&fixture, "journal_events").await, journal_count);
    assert_eq!(fixture.effects.snapshot().len(), effect_count);
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id = ?")
            .bind(queued_work.to_string())
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "queued"
    );
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn non_root_active_control_is_conversation_local_and_signals_only_its_work() {
    let fixture = fixture().await;
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());
    let user_b = UserId::generate();
    let conversation_b = create_conversation(
        fixture.guard.runtime(),
        fixture.owner.craxii_id,
        user_b,
        true,
        at(T0),
    )
    .await
    .unwrap()
    .conversation_id;
    let identity_b = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: user_b,
        external_subject_id: ExternalSubjectId::try_new("secondary-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(identity_b.clone())
        .await
        .unwrap();
    let binding_b = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: identity_b.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: user_b,
        conversation_id: conversation_b,
        external_conversation_id: ExternalConversationId::try_new("secondary-destination").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(binding_b.clone())
        .await
        .unwrap();
    let service = fixture.service();
    let event_b = |event_id: &str, message_id: &str, payload: VerifiedInboundPayload| {
        VerifiedInboundEvent::new(
            fixture.account.channel_account_id,
            ExternalEventId::try_new(event_id).unwrap(),
            Some(ExternalMessageId::try_new(message_id).unwrap()),
            identity_b.external_subject_id.clone(),
            binding_b.external_conversation_id.clone(),
            None,
            payload,
            None,
            at(T1),
        )
        .unwrap()
    };
    let admitted_b = service
        .classify(event_b(
            "secondary-message-event",
            "secondary-message",
            VerifiedInboundPayload::Text(text("secondary work")),
        ))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: work_b, ..
    } = admitted_b.outcome
    else {
        panic!("secondary message outcome expected");
    };
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), work_b);

    let admitted_a = service
        .classify(fixture.event("root-message-event", "root-message", "root work"))
        .await
        .unwrap();
    let DurableInboundOutcome::MessageAccepted {
        work_id: work_a, ..
    } = admitted_a.outcome
    else {
        panic!("root message outcome expected");
    };
    let control = service
        .classify(event_b(
            "secondary-control-event",
            "secondary-control",
            VerifiedInboundPayload::Text(text("STOP")),
        ))
        .await
        .unwrap();
    let DurableInboundOutcome::ControlApplied { target_work_id, .. } = control.outcome else {
        panic!("secondary control outcome expected");
    };
    assert_eq!(target_work_id, work_b);
    assert_eq!(
        fixture.effects.snapshot().last().unwrap().0,
        EffectKind::ActiveCancellation
    );
    assert_eq!(fixture.effects.snapshot().last().unwrap().1, work_b);

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let states: (String, String) = sqlx::query_as(
        "SELECT (SELECT state FROM work_items WHERE work_id = ?), \
                (SELECT state FROM work_items WHERE work_id = ?)",
    )
    .bind(work_a.to_string())
    .bind(work_b.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(states.0, "queued");
    assert_eq!(states.1, "cancel_requested");
    let root_cancellation_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM journal_events WHERE work_id = ? \
         AND event_type IN ('work.cancellation_requested','work.cancelled')",
    )
    .bind(work_a.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(root_cancellation_events, 0);
    drop(connection);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn unsupported_rejected_no_target_and_received_replay_are_fail_closed() {
    let fixture = fixture().await;
    let service = fixture.service();
    assert!(matches!(
        service
            .classify(fixture.unsupported("unsupported"))
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::Unsupported { .. }
    ));
    assert!(matches!(
        service
            .classify(fixture.event("no-target", "no-target", "/stop"))
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::ControlNoOp { .. }
    ));

    let missing_identity = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("missing-identity").unwrap(),
        Some(ExternalMessageId::try_new("missing-identity").unwrap()),
        ExternalSubjectId::try_new("unknown-subject").unwrap(),
        fixture.binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(missing_identity).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let received = fixture.event("stuck-received", "stuck-received", "hello");
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());
    identity_store
        .persist_inbound_delivery(InboundDelivery {
            inbound_delivery_id: InboundDeliveryId::generate(),
            channel_account_id: fixture.account.channel_account_id,
            craxii_id: fixture.owner.craxii_id,
            external_event_id: received.external_event_id().clone(),
            external_message_id: received.external_message_id().cloned(),
            external_subject_id: received.sender_subject_id().clone(),
            external_conversation_id: received.external_conversation_id().clone(),
            external_thread_id: received.external_thread_id().cloned(),
            material_sha256: received.material_digest(),
            provider_occurred_at: received.provider_occurred_at(),
            received_at: received.observed_at(),
            receipt_state: InboundReceiptState::Received,
            classification: None,
            external_identity_id: None,
            conversation_binding_id: None,
            user_id: None,
            conversation_id: None,
            message_id: None,
            work_id: None,
            control_target_work_id: None,
            classified_at: None,
        })
        .await
        .unwrap();
    let error = service.classify(received).await.unwrap_err();
    assert_eq!(error.kind(), ChannelIngressErrorKind::StorageInconsistent);
    assert!(!error.acknowledgement_safe());
    assert_eq!(table_count(&fixture, "messages").await, 0);
    assert_eq!(table_count(&fixture, "work_items").await, 0);
    assert_eq!(fixture.effects.snapshot().len(), 0);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn authorization_failures_reject_but_unknown_or_corrupt_topology_fails_closed() {
    let fixture = fixture().await;
    let service = fixture.service();
    let identity_store = SqliteChannelIdentityStore::new(fixture.guard.runtime().clone());

    let disabled_account = ChannelAccount {
        channel_account_id: ChannelAccountId::generate(),
        craxii_id: fixture.owner.craxii_id,
        provider_id: ChannelProviderId::try_new("disabled.adapter").unwrap(),
        external_account_id: ExternalAccountId::try_new("disabled-account").unwrap(),
        lifecycle: ChannelAccountLifecycle::Disabled,
        created_at: at(T0),
        disabled_at: Some(at(T1)),
    };
    identity_store
        .persist_channel_account(disabled_account.clone())
        .await
        .unwrap();
    let disabled = VerifiedInboundEvent::new(
        disabled_account.channel_account_id,
        ExternalEventId::try_new("disabled-event").unwrap(),
        Some(ExternalMessageId::try_new("disabled-message").unwrap()),
        ExternalSubjectId::try_new("disabled-subject").unwrap(),
        ExternalConversationId::try_new("disabled-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(disabled).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let revoked_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("revoked-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Revoked,
        created_at: at(T0),
        revoked_at: Some(at(T1)),
    };
    identity_store
        .persist_external_identity(revoked_identity.clone())
        .await
        .unwrap();
    let revoked_event = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("revoked-event").unwrap(),
        Some(ExternalMessageId::try_new("revoked-message").unwrap()),
        revoked_identity.external_subject_id.clone(),
        ExternalConversationId::try_new("revoked-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(revoked_event).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let revoked_binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: fixture.identity.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        conversation_id: fixture.owner.conversation_id,
        external_conversation_id: ExternalConversationId::try_new("revoked-destination").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Revoked,
        created_at: at(T0),
        revoked_at: Some(at(T1)),
    };
    identity_store
        .persist_conversation_binding(revoked_binding.clone())
        .await
        .unwrap();
    let revoked_binding_event = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("revoked-binding-event").unwrap(),
        Some(ExternalMessageId::try_new("revoked-binding-message").unwrap()),
        fixture.identity.external_subject_id.clone(),
        revoked_binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service
            .classify(revoked_binding_event)
            .await
            .unwrap()
            .outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let other_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: fixture.owner.user_id,
        external_subject_id: ExternalSubjectId::try_new("other-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(other_identity.clone())
        .await
        .unwrap();
    let mismatched_sender = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("mismatched-event").unwrap(),
        Some(ExternalMessageId::try_new("mismatched-message").unwrap()),
        other_identity.external_subject_id.clone(),
        fixture.binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert!(matches!(
        service.classify(mismatched_sender).await.unwrap().outcome,
        DurableInboundOutcome::Rejected { .. }
    ));

    let second_user = UserId::generate();
    let second_conversation = create_conversation(
        fixture.guard.runtime(),
        fixture.owner.craxii_id,
        second_user,
        true,
        at(T0),
    )
    .await
    .unwrap()
    .conversation_id;
    let second_identity = ExternalIdentity {
        external_identity_id: ExternalIdentityId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: second_user,
        external_subject_id: ExternalSubjectId::try_new("second-user-subject").unwrap(),
        lifecycle: ExternalIdentityLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_external_identity(second_identity.clone())
        .await
        .unwrap();
    let second_binding = ConversationBinding {
        conversation_binding_id: ConversationBindingId::generate(),
        channel_account_id: fixture.account.channel_account_id,
        external_identity_id: second_identity.external_identity_id,
        craxii_id: fixture.owner.craxii_id,
        user_id: second_user,
        conversation_id: second_conversation,
        external_conversation_id: ExternalConversationId::try_new("second-conversation").unwrap(),
        external_thread_id: None,
        lifecycle: ConversationBindingLifecycle::Active,
        created_at: at(T0),
        revoked_at: None,
    };
    identity_store
        .persist_conversation_binding(second_binding.clone())
        .await
        .unwrap();
    let non_root = VerifiedInboundEvent::new(
        fixture.account.channel_account_id,
        ExternalEventId::try_new("non-root-event").unwrap(),
        Some(ExternalMessageId::try_new("non-root-message").unwrap()),
        second_identity.external_subject_id.clone(),
        second_binding.external_conversation_id.clone(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    let non_root_outcome = service.classify(non_root).await.unwrap().outcome;
    let (non_root_work_id, non_root_message_id) = match non_root_outcome {
        DurableInboundOutcome::MessageAccepted {
            work_id,
            message_id,
            ..
        } => (work_id, message_id),
        other => panic!("valid non-root admission was not accepted: {other:?}"),
    };
    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    let route: (String, String, String) = sqlx::query_as(
        "SELECT m.conversation_id, w.conversation_id, w.reply_binding_id \
         FROM messages m JOIN work_items w ON w.work_id = ? \
         WHERE m.message_id = ?",
    )
    .bind(non_root_work_id.to_string())
    .bind(non_root_message_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert_eq!(route.0, second_conversation.to_string());
    assert_eq!(route.1, second_conversation.to_string());
    assert_eq!(route.2, second_binding.conversation_binding_id.to_string());
    drop(connection);
    let runtime_id = create_runtime(&fixture).await;
    let claimed = fixture
        .store
        .claim_next_work(ClaimNextWorkRequest {
            runtime_id,
            claimed_at: at(T1),
            event_id: JournalEventId::generate(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.work_id(), non_root_work_id);
    assert_eq!(claimed.work.conversation_id(), second_conversation);

    let unknown_account = VerifiedInboundEvent::new(
        ChannelAccountId::generate(),
        ExternalEventId::try_new("unknown-account-event").unwrap(),
        Some(ExternalMessageId::try_new("unknown-account-message").unwrap()),
        ExternalSubjectId::try_new("unknown-account-subject").unwrap(),
        ExternalConversationId::try_new("unknown-account-conversation").unwrap(),
        None,
        VerifiedInboundPayload::Text(text("hello")),
        None,
        at(T1),
    )
    .unwrap();
    assert_eq!(
        service.classify(unknown_account).await.unwrap_err().kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );

    let mut connection = fixture.guard.runtime().acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE external_identities SET craxii_id = ? WHERE external_identity_id = ?")
        .bind(CraxiiId::generate().to_string())
        .bind(fixture.identity.external_identity_id.to_string())
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        service
            .classify(fixture.event("corrupt-event", "corrupt-message", "hello"))
            .await
            .unwrap_err()
            .kind(),
        ChannelIngressErrorKind::StorageInconsistent
    );
    assert_eq!(table_count(&fixture, "messages").await, 1);
    assert_eq!(table_count(&fixture, "work_items").await, 1);
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn concurrent_events_linearize_dedupe_and_canonical_ordinals() {
    let fixture = fixture().await;
    let service = Arc::new(fixture.service());
    let same = fixture.event("same-event", "same-message", "same");
    let first_service = Arc::clone(&service);
    let first_event = same.clone();
    let first = tokio::spawn(async move { first_service.classify(first_event).await.unwrap() });
    let second_service = Arc::clone(&service);
    let second = tokio::spawn(async move { second_service.classify(same).await.unwrap() });
    let dispositions = [
        first.await.unwrap().disposition,
        second.await.unwrap().disposition,
    ];
    assert_eq!(
        dispositions
            .iter()
            .filter(|value| **value == InboundAdmissionDisposition::NewlyClassified)
            .count(),
        1
    );
    assert_eq!(
        dispositions
            .iter()
            .filter(|value| **value == InboundAdmissionDisposition::Duplicate)
            .count(),
        1
    );

    let a_service = Arc::clone(&service);
    let a_event = fixture.event("event-a", "message-a", "a");
    let a = tokio::spawn(async move { a_service.classify(a_event).await.unwrap() });
    let b_service = Arc::clone(&service);
    let b_event = fixture.event("event-b", "message-b", "b");
    let b = tokio::spawn(async move { b_service.classify(b_event).await.unwrap() });
    let outcomes = [a.await.unwrap().outcome, b.await.unwrap().outcome];
    let mut ordinals = outcomes
        .iter()
        .map(|outcome| match outcome {
            DurableInboundOutcome::MessageAccepted { work_ordinal, .. } => work_ordinal.get(),
            _ => panic!("message outcome expected"),
        })
        .collect::<Vec<_>>();
    ordinals.sort_unstable();
    assert_eq!(ordinals, vec![2, 3]);
    assert_eq!(table_count(&fixture, "messages").await, 3);
    assert_eq!(table_count(&fixture, "work_items").await, 3);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[tokio::test]
async fn non_exact_controls_and_whitespace_preserve_native_message_semantics() {
    let fixture = fixture().await;
    let service = fixture.service();
    for (index, value) in [
        "stop please",
        "please stop",
        "don't stop",
        "can you stop?",
        "   ",
    ]
    .into_iter()
    .enumerate()
    {
        let result = service
            .classify(fixture.event(
                &format!("ordinary-event-{index}"),
                &format!("ordinary-message-{index}"),
                value,
            ))
            .await
            .unwrap();
        assert!(matches!(
            result.outcome,
            DurableInboundOutcome::MessageAccepted { .. }
        ));
    }
    assert_eq!(table_count(&fixture, "messages").await, 5);
    assert_eq!(table_count(&fixture, "work_items").await, 5);
    assert_eq!(table_count(&fixture, "work_item_inputs").await, 5);
    fixture
        .store
        .verify_application_consistency()
        .await
        .unwrap();
    fixture.guard.shutdown().await;
}

#[test]
fn generic_ingress_sources_remain_provider_neutral_and_outbound_free() {
    let sources = [
        include_str!("../../application/channel_ingress.rs"),
        include_str!("../../application/control_message_policy.rs"),
        include_str!("../../ports/channel_ingress.rs"),
        include_str!("channel_ingress.rs"),
    ]
    .join("\n");
    for forbidden in [
        "Telegram",
        "update_id",
        "chat_id",
        "WAMID",
        "wa_id",
        "Graph API",
        "Bot API",
        "Slack event",
        "Discord gateway",
        "webhook",
        "polling offset",
        "outbound_deliveries",
        "delivery worker",
    ] {
        assert!(
            !sources.contains(forbidden),
            "found forbidden term: {forbidden}"
        );
    }
}
