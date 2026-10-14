use super::*;
use crate::distribution::startup::{DistributionConfig, DistributionRuntime};
use crate::distribution::{DistributionError, ValidatedDelivery, request_id};
use crate::distribution_contracts::{
    canonical::{DigestDomain, canonical_bytes, digest_material, sha256},
    signing::{SignaturePurpose, signature_input},
};
use crate::ports::state_store::BootstrapStateStore;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
struct Fixture {
    root: std::path::PathBuf,
    runtime: Option<DistributionRuntime>,
    key: Ed25519KeyPair,
    authority: Value,
    boot: String,
    power: String,
    conversation: String,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::var_os("CRAXII_RESULT_ATOMIC_CHILD_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("craxii-distribution-{}", uuid::Uuid::now_v7()))
            });
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        for p in ["state", "artifacts", "workspace"] {
            std::fs::create_dir(root.join(p)).unwrap();
            std::fs::set_permissions(root.join(p), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let key = Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let boot = request_id().unwrap();
        let power = request_id().unwrap();
        let runtime = DistributionRuntime::isolated_fixture(
            DistributionConfig {
                contract_version: 1,
                environment: "development".into(),
                origin: "https://localhost/".into(),
                state_root: root.join("state").to_str().unwrap().into(),
                artifact_root: root.join("artifacts").to_str().unwrap().into(),
                workspace_root: root.join("workspace").to_str().unwrap().into(),
                profile_public_key: URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),
                power_operation_id: power.clone(),
                release_id: "fixture-worker".into(),
                release_sha256: "a".repeat(64),
            },
            boot.clone(),
        )
        .await
        .unwrap();
        Self {
            root,
            runtime: Some(runtime),
            key,
            authority: json!({"customer_id":request_id().unwrap(),"logical_agent_id":request_id().unwrap(),"installation_id":request_id().unwrap(),"generation":"1","session_id":request_id().unwrap()}),
            boot,
            power,
            conversation: request_id().unwrap(),
        }
    }
    fn delivery(&self, id: &str, sequence: i64) -> ValidatedDelivery {
        let at = time::OffsetDateTime::now_utc();
        let issued = crate::domain::UtcTimestamp::from_offset_datetime(at)
            .unwrap()
            .to_string();
        let expires =
            crate::domain::UtcTimestamp::from_offset_datetime(at + time::Duration::seconds(60))
                .unwrap()
                .to_string();
        let material = json!({"material_version":1,"environment":"development","customer_id":self.authority["customer_id"],"logical_agent_id":self.authority["logical_agent_id"],"work_id":id,"conversation_id":self.conversation,"ingress_id":request_id().unwrap(),"conversation_sequence":sequence.to_string(),"reply_binding":{"binding_id":request_id().unwrap(),"binding_version":"1"},"content":{"version":1,"blocks":[{"type":"text","text":"synthetic owned input"}]},"created_at":issued});
        let digest = digest_material(DigestDomain::Work, &material).unwrap();
        let profile_id = request_id().unwrap();
        let config=sha256(&canonical_bytes(&json!({"program":"receipt_only_v1","tools":[],"model_dispatch":false,"tool_dispatch":false})).unwrap());
        let p = json!({"profile_version":1,"profile_id":profile_id,"environment":"development","audience":"craxii.worker:development","acceptance_account_authorization_id":request_id().unwrap(),"customer_id":self.authority["customer_id"],"logical_agent_id":self.authority["logical_agent_id"],"installation_id":self.authority["installation_id"],"generation":self.authority["generation"],"work_id":id,"period_id":request_id().unwrap(),"compute_grant_id":request_id().unwrap(),"model_config_id":request_id().unwrap(),"price_config_id":request_id().unwrap(),"material_digest":digest,"model_config_digest":config,"price_config_digest":config,"issued_at":issued,"expires_at":expires,"allowed_tool_ids":[],"max_tool_duration_ms":"1","kind":"development_fixture","deterministic_program_id":"receipt_only_v1","config_digest":config});
        let profile = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"execution_profile","request_id":request_id().unwrap(),"environment":"development","authority":self.authority,"body":{"profile":p,"key_id":sha256(self.key.public_key().as_ref()),"signature":URL_SAFE_NO_PAD.encode(self.key.sign(&signature_input(SignaturePurpose::ExecutionProfile,&p).unwrap()).as_ref())}});
        let message = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"work_delivery","request_id":request_id().unwrap(),"environment":"development","authority":self.authority,"body":{"delivery_id":request_id().unwrap(),"power_operation_id":self.power,"delivery_revision":"1","boot_id":self.boot,"issued_at":issued,"expires_at":expires,"admission_operation_id":request_id().unwrap(),"material":material,"material_digest":digest,"execution_authorization":{"kind":"development_fixture","profile_id":profile_id,"profile_digest":sha256(&canonical_bytes(&p).unwrap())}}});
        ValidatedDelivery::from_authenticated(
            message,
            Some(profile),
            URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref()),
            &self.authority,
            &self.boot,
            &self.power,
        )
        .unwrap()
    }
    fn terminal_delivery(&self) -> ValidatedDelivery {
        let mut d = self.delivery(&request_id().unwrap(), 1);
        let p = &mut d.profile.as_mut().unwrap()["body"]["profile"];
        p["deterministic_program_id"] = json!("terminal_text_v1");
        let hash=sha256(&canonical_bytes(&json!({"program":"terminal_text_v1","tools":[],"model_dispatch":true,"tool_dispatch":false})).unwrap());
        for k in [
            "config_digest",
            "model_config_digest",
            "price_config_digest",
        ] {
            p[k] = json!(hash)
        }
        d.message["body"]["execution_authorization"]["profile_digest"] =
            json!(sha256(&canonical_bytes(p).unwrap()));
        let signature = URL_SAFE_NO_PAD.encode(
            self.key
                .sign(&signature_input(SignaturePurpose::ExecutionProfile, p).unwrap())
                .as_ref(),
        );
        d.profile.as_mut().unwrap()["body"]["signature"] = json!(signature);
        d
    }
    fn control(&self, work: &ValidatedDelivery) -> ValidatedDelivery {
        let mut message = work.message.clone();
        message["type"] = json!("cancel_delivery");
        let c = json!({"control_id":request_id().unwrap(),"customer_id":self.authority["customer_id"],"logical_agent_id":self.authority["logical_agent_id"],"work_id":work.message["body"]["material"]["work_id"],"material_digest":work.message["body"]["material_digest"],"command":"cancel","reason":"user_request"});
        let body = message["body"].as_object_mut().unwrap();
        for k in [
            "material",
            "material_digest",
            "execution_authorization",
            "admission_operation_id",
        ] {
            body.remove(k);
        }
        body.insert(
            "control_digest".into(),
            json!(digest_material(DigestDomain::Cancel, &c).unwrap()),
        );
        body.insert("control".into(), c);
        ValidatedDelivery::from_authenticated(
            message,
            None,
            URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref()),
            &self.authority,
            &self.boot,
            &self.power,
        )
        .unwrap()
    }
    async fn close(&mut self) {
        self.runtime.take().unwrap().shutdown().await.unwrap();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
#[tokio::test]
async fn distribution_atomic_receipt_replay_cancel_and_tombstone_fifo() {
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let w = f.delivery(&request_id().unwrap(), 1);
    let first = store.admit_distribution(&w).await.unwrap();
    let receipt = match first {
        DistributionAdmission::Receipt {
            receipt, duplicate, ..
        } => {
            assert!(!duplicate);
            receipt
        }
        _ => panic!(),
    };
    let mut replay = ValidatedDelivery::from_authenticated(
        w.message.clone(),
        None,
        w.profile_key.clone(),
        &f.authority,
        &f.boot,
        &f.power,
    )
    .unwrap();
    replay.profile = None;
    match store.admit_distribution(&replay).await.unwrap() {
        DistributionAdmission::Receipt {
            receipt: r,
            duplicate,
            ..
        } => {
            assert!(duplicate);
            assert_eq!(r, receipt)
        }
        _ => panic!(),
    }
    let c = f.control(&w);
    let cancelled = store.cancel_distribution(&c).await.unwrap();
    assert_eq!(cancelled["outcome"], "cancelled");
    assert_eq!(store.cancel_distribution(&c).await.unwrap(), cancelled);
    let next = f.delivery(&request_id().unwrap(), 2);
    let tomb = f.control(&next);
    let before = store.cancel_distribution(&tomb).await.unwrap();
    assert_eq!(before["outcome"], "cancelled_before_admission");
    assert!(matches!(
        store.admit_distribution(&next).await.unwrap(),
        DistributionAdmission::Cancelled { .. }
    ));
    let later = f.delivery(&request_id().unwrap(), 3);
    store.admit_distribution(&later).await.unwrap();
    store.verify_application_consistency().await.unwrap();
    store.verify_distribution_consistency().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM work_items")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM model_invocations")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        0
    );
    let mut conflict = ValidatedDelivery::from_authenticated(
        w.message.clone(),
        None,
        w.profile_key.clone(),
        &f.authority,
        &f.boot,
        &f.power,
    )
    .unwrap();
    conflict.message["body"]["material"]["content"]["blocks"][0]["text"] =
        json!("conflicting content");
    conflict.message["body"]["material_digest"] =
        json!(digest_material(DigestDomain::Work, &conflict.message["body"]["material"]).unwrap());
    assert_eq!(
        store.admit_distribution(&conflict).await.unwrap_err(),
        DistributionError::Conflict
    );
    assert_eq!(
        store.verify_distribution_consistency().await.unwrap_err(),
        DistributionError::Conflict
    );
    f.close().await;
}
#[tokio::test]
async fn distribution_profile_and_deadline_rejection_rolls_back_all_admission() {
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let mut invalid = f.delivery(&request_id().unwrap(), 1);
    invalid.profile.as_mut().unwrap()["body"]["profile"]["config_digest"] = json!("b".repeat(64));
    assert_eq!(
        store.admit_distribution(&invalid).await.unwrap_err(),
        DistributionError::Invalid
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM distribution_owners")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        0
    );
    let mut expired = f.delivery(&request_id().unwrap(), 1);
    expired.deadline = std::time::Instant::now();
    assert_eq!(
        store.admit_distribution(&expired).await.unwrap_err(),
        DistributionError::Expired
    );
    let out_of_order = f.delivery(&request_id().unwrap(), 2);
    assert_eq!(
        store.admit_distribution(&out_of_order).await.unwrap_err(),
        DistributionError::Reconciliation
    );
    f.close().await;
}
#[tokio::test]
async fn distribution_receipt_failure_rolls_back_kernel_and_claims_stay_closed() {
    use crate::ports::state_store::{ClaimNextWorkRequest, SchedulerStateStore};
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let delivery = f.delivery(&request_id().unwrap(), 1);
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM journal_events")
        .fetch_one(&store.runtime.inner.pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER distribution_fixture_receipt_abort BEFORE INSERT ON distribution_receipts BEGIN SELECT RAISE(ABORT,'fixture_receipt_failure'); END").execute(&store.runtime.inner.pool).await.unwrap();
    assert_eq!(
        store.admit_distribution(&delivery).await.unwrap_err(),
        DistributionError::Storage
    );
    for sql in [
        "SELECT count(*) FROM messages",
        "SELECT count(*) FROM work_items",
        "SELECT count(*) FROM work_item_inputs",
        "SELECT count(*) FROM distribution_owners",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&store.runtime.inner.pool)
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM journal_events")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        before
    );
    sqlx::query("DROP TRIGGER distribution_fixture_receipt_abort")
        .execute(&store.runtime.inner.pool)
        .await
        .unwrap();
    store.admit_distribution(&delivery).await.unwrap();
    let runtime = sqlx::query_scalar::<_, String>(
        "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
    )
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    assert!(
        store
            .claim_next_work(ClaimNextWorkRequest {
                runtime_id: runtime.parse().unwrap(),
                claimed_at: crate::distribution::timestamp().unwrap(),
                event_id: crate::domain::JournalEventId::generate()
            })
            .await
            .unwrap()
            .is_none()
    );
    let mut waiting = f.delivery(&request_id().unwrap(), 2);
    waiting.deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
    let lock = store
        .runtime
        .inner
        .write_coordinator
        .clone()
        .lock_owned()
        .await;
    let pending = store.clone();
    let task = tokio::spawn(async move { pending.admit_distribution(&waiting).await });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    drop(lock);
    assert_eq!(task.await.unwrap().unwrap_err(), DistributionError::Expired);
    f.close().await;
}
#[tokio::test]
async fn distribution_replacement_replays_original_receipt_after_cancellation_recovery() {
    use super::cancellation::decode_loaded_cancellation_work;
    use super::journal::{JournalAppendIntent, append_event, prepare_event};
    use super::projection::{WorkProjectionTimes, guarded_work_update};
    use super::transaction::WriteTransaction;
    use crate::domain::*;
    let mut f = Fixture::new().await;
    let w = f.delivery(&request_id().unwrap(), 1);
    let store = f.runtime.as_ref().unwrap().store.clone();
    let original = match store.admit_distribution(&w).await.unwrap() {
        DistributionAdmission::Receipt { receipt, .. } => receipt,
        _ => panic!(),
    };
    let id: WorkId = original["local_work_id"].as_str().unwrap().parse().unwrap();
    let rid: RuntimeInstanceId = sqlx::query_scalar::<_, String>(
        "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
    )
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap()
    .parse()
    .unwrap();
    let mut tx = WriteTransaction::begin(&store.runtime, "distribution_uncertain_recovery_fixture")
        .await
        .unwrap();
    let row = sqlx::query("SELECT * FROM work_items WHERE work_id=?")
        .bind(id.to_string())
        .fetch_one(tx.connection())
        .await
        .unwrap();
    let loaded = decode_loaded_cancellation_work(&row, id).unwrap();
    let next = decide_work_transition(
        &loaded.snapshot,
        WorkTransitionGuard::for_snapshot(&loaded.snapshot),
        WorkTransitionRequest::Start { runtime_owner: rid },
    )
    .unwrap()
    .into_next();
    let at = crate::distribution::timestamp().unwrap();
    guarded_work_update(
        &mut tx,
        &loaded.snapshot,
        &next,
        WorkProjectionTimes {
            started_at: Some(at),
            cancel_requested_at: None,
            terminal_at: None,
        },
    )
    .await
    .unwrap();
    let cause = sqlx::query_scalar::<_, String>(
        "SELECT event_id FROM journal_events WHERE work_id=? ORDER BY stream_seq DESC LIMIT 1",
    )
    .bind(id.to_string())
    .fetch_one(tx.connection())
    .await
    .unwrap();
    append_event(
        &mut tx,
        prepare_event(JournalAppendIntent {
            event_id: JournalEventId::generate(),
            craxii_id: loaded.craxii_id,
            stream_id: JournalStreamId::Work(id),
            conversation_id: Some(loaded.conversation_id),
            work_id: Some(id),
            causation_event_id: Some(cause.parse().unwrap()),
            correlation_id: loaded.correlation_id,
            actor: JournalActor::Runtime(rid),
            runtime_instance_id: Some(rid),
            payload: JournalEventPayload::WorkStarted(WorkTransitionV1 {
                work_id: id,
                from_state: WorkState::Queued,
                to_state: WorkState::Running,
                expected_state_version: loaded.snapshot.projection_version(),
                expected_runtime_owner: None,
                expected_current_attempt: JournalCurrentAttempt::None,
                expected_cancellation_reason: None,
                state_version: next.projection_version(),
                runtime_owner: Some(rid),
                current_attempt: JournalCurrentAttempt::None,
                cancellation_reason: None,
                terminal_reason: None,
                transitioned_at: at,
            }),
            recorded_at: at,
            occurred_at: None,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let c = f.control(&w);
    let requested = store.cancel_distribution(&c).await.unwrap();
    assert_eq!(requested["outcome"], "requested");
    assert_eq!(requested["cleanup_pending"], true);
    drop(store);
    f.runtime.take().unwrap().isolated_crash_close().await;
    let config = DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://localhost/".into(),
        state_root: f.root.join("state").to_str().unwrap().into(),
        artifact_root: f.root.join("artifacts").to_str().unwrap().into(),
        workspace_root: f.root.join("workspace").to_str().unwrap().into(),
        profile_public_key: URL_SAFE_NO_PAD.encode(f.key.public_key().as_ref()),
        power_operation_id: f.power.clone(),
        release_id: "fixture-worker".into(),
        release_sha256: "a".repeat(64),
    };
    f.runtime = Some(
        DistributionRuntime::isolated_fixture(config, f.boot.clone())
            .await
            .unwrap(),
    );
    f.authority["installation_id"] = json!(request_id().unwrap());
    f.authority["generation"] = json!("2");
    f.authority["session_id"] = json!(request_id().unwrap());
    let mut message = w.message.clone();
    message["authority"] = f.authority.clone();
    let replay = ValidatedDelivery::from_authenticated(
        message,
        None,
        w.profile_key.clone(),
        &f.authority,
        &f.boot,
        &f.power,
    )
    .unwrap();
    match f
        .runtime
        .as_ref()
        .unwrap()
        .store
        .admit_distribution(&replay)
        .await
        .unwrap()
    {
        DistributionAdmission::Receipt {
            receipt,
            state,
            duplicate,
            ..
        } => {
            assert_eq!(receipt, original);
            assert!(duplicate);
            assert_eq!(state, "cancelled");
        }
        _ => panic!(),
    }
    assert_eq!(
        f.runtime
            .as_ref()
            .unwrap()
            .store
            .cancel_distribution(&c)
            .await
            .unwrap_err(),
        DistributionError::Invalid
    );
    let mut current = c.message.clone();
    current["authority"] = f.authority.clone();
    let current = ValidatedDelivery::from_authenticated(
        current,
        None,
        c.profile_key.clone(),
        &f.authority,
        &f.boot,
        &f.power,
    )
    .unwrap();
    assert_eq!(
        f.runtime
            .as_ref()
            .unwrap()
            .store
            .cancel_distribution(&current)
            .await
            .unwrap(),
        requested
    );
    f.close().await;
}

#[tokio::test]
async fn distribution_result_cancel_notice_ack_and_sleep_blocker() {
    let mut f = Fixture::new().await;
    let w = f.delivery(&request_id().unwrap(), 1);
    let store = &f.runtime.as_ref().unwrap().store;
    store.admit_distribution(&w).await.unwrap();
    store.cancel_distribution(&f.control(&w)).await.unwrap();
    let pending = store.pending_distribution_results().await.unwrap();
    assert_eq!(pending.len(), 1);
    let result = &pending[0];
    assert_eq!(result["snapshot"]["work_state"], "cancelled");
    assert!(result["snapshot"].get("local_message_id").is_none());
    assert_eq!(
        f.runtime
            .as_ref()
            .unwrap()
            .export_observation()
            .await
            .unwrap()["export_drained"],
        false
    );
    let ack = json!({"work_id":result["snapshot"]["work_id"],"export_id":result["snapshot"]["export_id"],"export_revision":"1","export_digest":result["export_digest"],"acceptance_id":request_id().unwrap(),"outbox_source_id":request_id().unwrap(),"accepted_at":crate::distribution::timestamp().unwrap().to_string(),"duplicate":false});
    let mut conflict = ack.clone();
    conflict["export_digest"] = json!("f".repeat(64));
    assert!(
        store
            .acknowledge_distribution_result(&conflict)
            .await
            .is_err()
    );
    assert_eq!(store.pending_distribution_results().await.unwrap(), pending);
    store.acknowledge_distribution_result(&ack).await.unwrap();
    let mut duplicate = ack.clone();
    duplicate["duplicate"] = json!(true);
    store
        .acknowledge_distribution_result(&duplicate)
        .await
        .unwrap();
    assert!(
        store
            .pending_distribution_results()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.runtime
            .as_ref()
            .unwrap()
            .export_observation()
            .await
            .unwrap()["export_drained"],
        true
    );
    conflict = ack.clone();
    conflict["acceptance_id"] = json!(request_id().unwrap());
    assert!(
        store
            .acknowledge_distribution_result(&conflict)
            .await
            .is_err()
    );
    store.verify_distribution_results().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn distribution_result_scripted_completion_and_exact_export() {
    // Real subprocess failpoints never write a core file.
    unsafe {
        nix::libc::setrlimit(
            nix::libc::RLIMIT_CORE,
            &nix::libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            },
        );
    }

    let mut f = Fixture::new().await;
    let d = f.terminal_delivery();
    let runtime = f.runtime.as_ref().unwrap();
    let store = &runtime.store;
    store.admit_distribution(&d).await.unwrap();
    let id: String = sqlx::query_scalar(
        "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
    )
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    let claimed = store
        .isolated_claim_distribution(
            crate::ports::state_store::ClaimNextWorkRequest {
                runtime_id: id.parse().unwrap(),
                claimed_at: crate::distribution::timestamp().unwrap(),
                event_id: crate::domain::JournalEventId::generate(),
            },
            &d,
        )
        .await
        .unwrap()
        .unwrap();
    let config = DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://localhost/".into(),
        state_root: f.root.join("state").to_str().unwrap().into(),
        artifact_root: f.root.join("artifacts").to_str().unwrap().into(),
        workspace_root: f.root.join("workspace").to_str().unwrap().into(),
        profile_public_key: URL_SAFE_NO_PAD.encode(f.key.public_key().as_ref()),
        power_operation_id: f.power.clone(),
        release_id: "fixture-worker".into(),
        release_sha256: "a".repeat(64),
    };
    crate::distribution::fixture::complete(store, &config, claimed)
        .await
        .unwrap();
    let results = store.pending_distribution_results().await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["snapshot"]["work_state"], "completed");
    use crate::domain::*;
    use crate::ports::state_store::{
        CommitAssistantCompletionRequest, CompletionStateStore, EventIntent, ModelExpectation,
        WorkExpectation,
    };
    use sqlx::Row;
    let original=sqlx::query("SELECT e.*,m.content_json,m.craxii_id,m.conversation_id,t.payload_json,a.event_id assistant_event,a.causation_event_id model_cause,a.correlation_id,model.model_invocation_id FROM distribution_results e JOIN messages m ON m.message_id=e.assistant_message_id JOIN journal_events t ON t.event_id=e.terminal_event_id JOIN journal_events a ON a.work_id=e.local_work_id AND a.event_type='assistant.message_committed' JOIN model_invocations model ON model.work_id=e.local_work_id").fetch_one(&store.runtime.inner.pool).await.unwrap();
    let transition: Value =
        serde_json::from_str(&original.try_get::<String, _>("payload_json").unwrap()).unwrap();
    let make_request = |different: u8| {
        let work = WorkId::parse_canonical(
            original
                .try_get::<String, _>("local_work_id")
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let running = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: work,
            state: WorkState::Running,
            projection_version: ProjectionVersion::try_new(
                transition["expected_state_version"].as_i64().unwrap(),
            )
            .unwrap(),
            runtime_owner: Some(
                transition["expected_runtime_owner"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            ),
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: None,
        })
        .unwrap();
        let next = decide_work_transition(
            &running,
            WorkTransitionGuard::for_snapshot(&running),
            WorkTransitionRequest::Complete {
                reason: WorkCompletionReason::Answered,
                evidence: WorkCompletionEvidence::SATISFIED,
            },
        )
        .unwrap()
        .into_next();
        let assistant: JournalEventId = original
            .try_get::<String, _>("assistant_event")
            .unwrap()
            .parse()
            .unwrap();
        let correlation = original
            .try_get::<String, _>("correlation_id")
            .unwrap()
            .parse()
            .unwrap();
        CommitAssistantCompletionRequest {
            expected_work: WorkExpectation::for_snapshot(&running),
            expected_model: ModelExpectation {
                model_invocation_id: original
                    .try_get::<String, _>("model_invocation_id")
                    .unwrap()
                    .parse()
                    .unwrap(),
                state: ModelInvocationState::Completed,
            },
            assistant_message: Message::try_new(MessageInput {
                message_id: if different == 1 {
                    MessageId::generate()
                } else {
                    original
                        .try_get::<String, _>("assistant_message_id")
                        .unwrap()
                        .parse()
                        .unwrap()
                },
                craxii_id: original
                    .try_get::<String, _>("craxii_id")
                    .unwrap()
                    .parse()
                    .unwrap(),
                conversation_id: original
                    .try_get::<String, _>("conversation_id")
                    .unwrap()
                    .parse()
                    .unwrap(),
                role: MessageRole::Assistant,
                content: super::codec::decode_message_content(
                    &original.try_get::<String, _>("content_json").unwrap(),
                    &original
                        .try_get::<String, _>("native_content_sha256")
                        .unwrap(),
                )
                .unwrap(),
                author_user_id: None,
                produced_by_work_id: Some(work),
                device_id: None,
                client_message_id: None,
                inbound_delivery_id: None,
                committed_at: original
                    .try_get::<String, _>("terminal_at")
                    .unwrap()
                    .parse()
                    .unwrap(),
            })
            .unwrap(),
            assistant_event: EventIntent {
                event_id: assistant,
                correlation_id: correlation,
                causation_event_id: Some(
                    original
                        .try_get::<String, _>("model_cause")
                        .unwrap()
                        .parse()
                        .unwrap(),
                ),
            },
            completion_event: EventIntent {
                event_id: original
                    .try_get::<String, _>("terminal_event_id")
                    .unwrap()
                    .parse()
                    .unwrap(),
                correlation_id: correlation,
                causation_event_id: Some(assistant),
            },
            work_next: next,
            deliveries: vec![],
        }
    };
    store
        .commit_assistant_completion(make_request(0))
        .await
        .unwrap();
    assert!(
        store
            .commit_assistant_completion(make_request(1))
            .await
            .is_err()
    );
    assert_eq!(store.pending_distribution_results().await.unwrap(), results);
    store.verify_application_consistency().await.unwrap();
    store.verify_distribution_results().await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn distribution_result_atomic_process_crashes_and_conservative_recovery() {
    for (hook, committed) in [
        ("final_answer_after_all_rows_before_commit", false),
        ("final_answer_after_commit_before_notification", true),
    ] {
        let root =
            std::env::temp_dir().join(format!("craxii-result-atomic-{}", uuid::Uuid::now_v7()));
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","adapters::sqlite::distribution_tests::distribution_result_scripted_completion_and_exact_export","--test-threads=1"]).env("CRAXII_RESULT_ATOMIC_CHILD_ROOT",&root).env("CRAXII_TEST_ABORT_AT_FAILPOINT",hook).output().unwrap();
        assert!(!output.status.success());
        let mut c = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.join("state/db/craxii.sqlite3"))
            .foreign_keys(true)
            .connect()
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE role='assistant'")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert_eq!(count, i64::from(committed));
        let exports: i64 = sqlx::query_scalar("SELECT count(*) FROM distribution_results")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert_eq!(exports, i64::from(committed));
        let completions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM journal_events WHERE event_type='work.completed'",
        )
        .fetch_one(&mut c)
        .await
        .unwrap();
        assert_eq!(completions, i64::from(committed));
        use sqlx::{ConnectOptions, Connection};
        c.close().await.unwrap();
        let config = DistributionConfig {
            contract_version: 1,
            environment: "development".into(),
            origin: "https://localhost/".into(),
            state_root: root.join("state").to_str().unwrap().into(),
            artifact_root: root.join("artifacts").to_str().unwrap().into(),
            workspace_root: root.join("workspace").to_str().unwrap().into(),
            profile_public_key: URL_SAFE_NO_PAD.encode([0u8; 32]),
            power_operation_id: request_id().unwrap(),
            release_id: "fixture-worker".into(),
            release_sha256: "a".repeat(64),
        };
        let runtime = DistributionRuntime::isolated_fixture(config, request_id().unwrap())
            .await
            .unwrap();
        let pending = runtime.store.pending_distribution_results().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0]["snapshot"]["work_state"],
            if committed {
                "completed"
            } else {
                "interrupted"
            }
        );
        let models: i64 = sqlx::query_scalar("SELECT count(*) FROM model_invocations")
            .fetch_one(&runtime.store.runtime.inner.pool)
            .await
            .unwrap();
        assert_eq!(models, 1);
        runtime.shutdown().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn distribution_result_populated_v8_terminal_upgrade_reconciles_without_rewriting_history() {
    use sqlx::{ConnectOptions, Connection};
    let mut f = Fixture::new().await;
    let d = f.delivery(&request_id().unwrap(), 1);
    let store = &f.runtime.as_ref().unwrap().store;
    store.admit_distribution(&d).await.unwrap();
    store.cancel_distribution(&f.control(&d)).await.unwrap();
    f.runtime.take().unwrap().shutdown().await.unwrap();
    let root = std::env::temp_dir().join(format!("craxii-result-upgrade-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    for p in ["state", "artifacts", "workspace", "state/db"] {
        std::fs::create_dir(root.join(p)).unwrap();
        std::fs::set_permissions(root.join(p), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(root.join("state/db/craxii.sqlite3"))
        .create_if_missing(true)
        .foreign_keys(false)
        .connect()
        .await
        .unwrap();
    sqlx::query("CREATE TEMP TABLE ch1_owner_seed(user_id TEXT NOT NULL)")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::migrate::Migrator::with_migrations(
        super::schema::MIGRATOR.iter().take(8).cloned().collect(),
    )
    .run(&mut connection)
    .await
    .unwrap();
    sqlx::query("ATTACH DATABASE ? AS prior")
        .bind(f.root.join("state/db/craxii.sqlite3").to_str().unwrap())
        .execute(&mut connection)
        .await
        .unwrap();
    // Construct a populated released V8 fixture from unchanged legacy columns; never
    // run a down migration on any application state. The result table is not copied.
    for t in super::schema::V8_PRODUCT_TABLES {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO main.{t} SELECT * FROM prior.{t}"
        )))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    let before: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event_id,payload_json,payload_sha256 FROM journal_events ORDER BY journal_offset",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let receipt: String = sqlx::query_scalar("SELECT receipt_canonical FROM distribution_receipts")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    let config = DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://localhost/".into(),
        state_root: root.join("state").to_str().unwrap().into(),
        artifact_root: root.join("artifacts").to_str().unwrap().into(),
        workspace_root: f.root.join("workspace").to_str().unwrap().into(),
        profile_public_key: URL_SAFE_NO_PAD.encode([0u8; 32]),
        power_operation_id: request_id().unwrap(),
        release_id: "fixture-worker".into(),
        release_sha256: "a".repeat(64),
    };
    std::fs::set_permissions(
        root.join("state/db/craxii.sqlite3"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let runtime = DistributionRuntime::isolated_fixture(config, request_id().unwrap())
        .await
        .unwrap();
    let rows = runtime.store.pending_distribution_results().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["snapshot"]["work_state"], "cancelled");
    let current=sqlx::query_as::<_,(String,String,String)>("SELECT event_id,payload_json,payload_sha256 FROM journal_events ORDER BY journal_offset LIMIT ?").bind(before.len() as i64).fetch_all(&runtime.store.runtime.inner.pool).await.unwrap();
    assert_eq!(current, before);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT receipt_canonical FROM distribution_receipts")
            .fetch_one(&runtime.store.runtime.inner.pool)
            .await
            .unwrap(),
        receipt
    );
    runtime
        .store
        .reconcile_distribution_results()
        .await
        .unwrap();
    assert_eq!(
        runtime.store.pending_distribution_results().await.unwrap(),
        rows
    );
    runtime.shutdown().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(&f.root).unwrap();
}

#[tokio::test]
async fn distribution_result_failure_and_confirmed_active_cancellation_are_truthful() {
    use crate::domain::*;
    use crate::ports::state_store::{
        EventIntent, FinishCancellationRequest, ModelStateStore, SchedulerStateStore,
        TerminalizeOwnedWorkRequest, WorkExpectation,
    };
    for cancel in [false, true] {
        let mut f = Fixture::new().await;
        let d = f.terminal_delivery();
        let store = &f.runtime.as_ref().unwrap().store;
        store.admit_distribution(&d).await.unwrap();
        let runtime: String = sqlx::query_scalar(
            "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
        )
        .fetch_one(&store.runtime.inner.pool)
        .await
        .unwrap();
        let runtime: RuntimeInstanceId = runtime.parse().unwrap();
        let claimed = store
            .isolated_claim_distribution(
                crate::ports::state_store::ClaimNextWorkRequest {
                    runtime_id: runtime,
                    claimed_at: crate::distribution::timestamp().unwrap(),
                    event_id: JournalEventId::generate(),
                },
                &d,
            )
            .await
            .unwrap()
            .unwrap();
        if cancel {
            store.cancel_distribution(&f.control(&d)).await.unwrap();
            assert!(
                store
                    .pending_distribution_results()
                    .await
                    .unwrap()
                    .is_empty()
            );
            store
                .finish_cancellation(FinishCancellationRequest {
                    work_id: claimed.work.work_id(),
                    runtime_id: runtime,
                    confirmed_at: crate::distribution::timestamp().unwrap(),
                    event_id: JournalEventId::generate(),
                })
                .await
                .unwrap();
        } else {
            let next = decide_work_transition(
                &claimed.lifecycle,
                WorkTransitionGuard::for_snapshot(&claimed.lifecycle),
                WorkTransitionRequest::Fail {
                    reason: WorkFailureReason::Limit(LifecycleLimit::Context),
                    cleanup_status: CleanupStatus::NotRequired,
                },
            )
            .unwrap()
            .into_next();
            let cause:String=sqlx::query_scalar("SELECT event_id FROM journal_events WHERE work_id=? ORDER BY journal_offset DESC LIMIT 1").bind(claimed.work.work_id().to_string()).fetch_one(&store.runtime.inner.pool).await.unwrap();
            store
                .terminalize_owned_work(TerminalizeOwnedWorkRequest {
                    expected_work: WorkExpectation::for_snapshot(&claimed.lifecycle),
                    work_next: next,
                    terminal_at: crate::distribution::timestamp().unwrap(),
                    event: EventIntent {
                        event_id: JournalEventId::generate(),
                        correlation_id: claimed.work.correlation_id(),
                        causation_event_id: Some(cause.parse().unwrap()),
                    },
                })
                .await
                .unwrap();
        }
        let rows = store.pending_distribution_results().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["snapshot"]["work_state"],
            if cancel { "cancelled" } else { "failed" }
        );
        assert!(rows[0]["snapshot"].get("local_message_id").is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE role='assistant'")
                .fetch_one(&store.runtime.inner.pool)
                .await
                .unwrap(),
            0
        );
        store.verify_application_consistency().await.unwrap();
        store.verify_distribution_results().await.unwrap();
        f.close().await;
    }
}
