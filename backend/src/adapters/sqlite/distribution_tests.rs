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
                model_proxy: None,
                agent_limits: None,
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
            crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
            crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
        crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
        crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
        model_proxy: None,
        agent_limits: None,
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
        crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
        crate::distribution_contracts::environment::WorkerEnvironment::Development,
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
    let quiescence = f
        .runtime
        .as_ref()
        .unwrap()
        .quiescence_observation()
        .await
        .unwrap();
    assert!(quiescence.pending_exports > 0);
    assert_eq!(
        quiescence.state(),
        crate::guest_boundary::QuiescenceState::Busy
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
    let snapshot = store.load_bootstrap_snapshot().await.unwrap();
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
        model_proxy: None,
        agent_limits: None,
    };
    crate::distribution::fixture::complete(
        store,
        &config,
        &(snapshot.workstation, snapshot.workspace),
        claimed,
        // This terminal-only fixture uses the scripted provider and denied workstation;
        // it neither authorizes installed tools nor selects the central proxy.
        #[cfg(target_os = "linux")]
        None,
        #[cfg(target_os = "linux")]
        None,
    )
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
            model_proxy: None,
            agent_limits: None,
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
        model_proxy: None,
        agent_limits: None,
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
async fn configured_environment_must_match_every_delivery_and_config() {
    use crate::distribution_contracts::environment::WorkerEnvironment;
    let mut f = Fixture::new().await;
    let d = f.delivery(&request_id().unwrap(), 1);
    // A production worker refuses a development delivery before it reaches storage.
    assert!(
        ValidatedDelivery::from_authenticated(
            WorkerEnvironment::Production,
            d.message.clone(),
            d.profile.clone(),
            d.profile_key.clone(),
            &f.authority,
            &f.boot,
            &f.power,
        )
        .is_err()
    );
    let mut config = DistributionConfig {
        contract_version: 1,
        environment: "production".into(),
        origin: "https://localhost/".into(),
        state_root: f.root.join("state").to_str().unwrap().into(),
        artifact_root: f.root.join("artifacts").to_str().unwrap().into(),
        workspace_root: f.root.join("workspace").to_str().unwrap().into(),
        profile_public_key: URL_SAFE_NO_PAD.encode([0u8; 32]),
        power_operation_id: request_id().unwrap(),
        release_id: "fixture-worker".into(),
        release_sha256: "a".repeat(64),
        model_proxy: None,
        agent_limits: None,
    };
    config.validate().unwrap();
    assert_eq!(
        config.worker_environment().unwrap(),
        WorkerEnvironment::Production
    );
    for refused in ["test", "local", "staging", ""] {
        config.environment = refused.into();
        assert!(config.validate().is_err());
        assert!(config.worker_environment().is_err());
    }
    // Once a development delivery owns the store, a worker restarted with another environment or owner is
    // refused as a permanent identity mismatch (the worker stops at startup instead of skipping deliveries).
    let store = &f.runtime.as_ref().unwrap().store;
    store.admit_distribution(&d).await.unwrap();
    let (customer, agent) = (
        f.authority["customer_id"].as_str().unwrap().to_owned(),
        f.authority["logical_agent_id"].as_str().unwrap().to_owned(),
    );
    store
        .assert_distribution_identity(WorkerEnvironment::Development, &customer, &agent)
        .await
        .unwrap();
    for (environment, customer, agent) in [
        (
            WorkerEnvironment::Production,
            customer.as_str(),
            agent.as_str(),
        ),
        (WorkerEnvironment::Development, "other", agent.as_str()),
        (WorkerEnvironment::Development, customer.as_str(), "other"),
    ] {
        assert_eq!(
            store
                .assert_distribution_identity(environment, customer, agent)
                .await
                .unwrap_err(),
            DistributionError::Identity
        );
    }
    f.runtime.take().unwrap().shutdown().await.unwrap();
    std::fs::remove_dir_all(&f.root).unwrap();
}

#[tokio::test]
async fn populated_v9_upgrade_admits_production_without_changing_rows() {
    use sqlx::{ConnectOptions, Connection};
    let mut f = Fixture::new().await;
    let d = f.delivery(&request_id().unwrap(), 1);
    let store = &f.runtime.as_ref().unwrap().store;
    store.admit_distribution(&d).await.unwrap();
    store.cancel_distribution(&f.control(&d)).await.unwrap();
    f.runtime.take().unwrap().shutdown().await.unwrap();
    let root = std::env::temp_dir().join(format!("craxii-env-upgrade-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    for p in ["state", "artifacts", "workspace", "state/db"] {
        std::fs::create_dir(root.join(p)).unwrap();
        std::fs::set_permissions(root.join(p), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = root.join("state/db/craxii.sqlite3");
    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
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
        super::schema::MIGRATOR.iter().take(9).cloned().collect(),
    )
    .run(&mut connection)
    .await
    .unwrap();
    sqlx::query("ATTACH DATABASE ? AS prior")
        .bind(f.root.join("state/db/craxii.sqlite3").to_str().unwrap())
        .execute(&mut connection)
        .await
        .unwrap();
    // A released V9 database built from the same rows; schema 10 changes no columns.
    for t in super::schema::V10_PRODUCT_TABLES {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO main.{t} SELECT * FROM prior.{t}"
        )))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    const SNAPSHOT: &str = "SELECT \
        (SELECT json_group_array(json_array(environment,customer_id,logical_agent_id,user_id,craxii_id,highest_generation,current_installation_id)) FROM distribution_owners), \
        (SELECT json_group_array(json_array(export_id,environment,central_work_id,work_state,state_version,export_digest,transfer_state,metadata_canonical)) FROM distribution_results)";
    let before: (String, String) = sqlx::query_as(SNAPSHOT)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_ne!(before.0, "[]");
    assert_ne!(before.1, "[]");
    connection.close().await.unwrap();
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600)).unwrap();
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
        model_proxy: None,
        agent_limits: None,
    };
    let runtime = DistributionRuntime::isolated_fixture(config, request_id().unwrap())
        .await
        .unwrap();
    let pool = &runtime.store.runtime.inner.pool;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(pool)
            .await
            .unwrap(),
        super::schema::MAX_SUPPORTED_SCHEMA_VERSION
    );
    let after: (String, String) = sqlx::query_as(SNAPSHOT).fetch_one(pool).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pragma_foreign_key_check")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    // The immutability guards on results and their messages survive the rebuild.
    let guarded = sqlx::query("DELETE FROM distribution_results")
        .execute(pool)
        .await;
    assert!(
        guarded
            .unwrap_err()
            .to_string()
            .contains("immutable_result")
    );
    runtime.shutdown().await.unwrap();

    let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .foreign_keys(false)
        .connect()
        .await
        .unwrap();
    let mut outcomes = Vec::new();
    for (environment, user) in [
        ("production", "production-owner"),
        ("staging", "staging-owner"),
    ] {
        outcomes.push(
            sqlx::query(
                "INSERT INTO distribution_owners(environment,customer_id,logical_agent_id,user_id,craxii_id,highest_generation,current_installation_id) \
                 SELECT ?,customer_id,logical_agent_id,?,craxii_id,highest_generation,current_installation_id FROM distribution_owners LIMIT 1",
            )
            .bind(environment)
            .bind(user)
            .execute(&mut connection)
            .await,
        );
    }
    let refused = outcomes.pop().unwrap().unwrap_err();
    outcomes.pop().unwrap().unwrap();
    assert!(refused.to_string().contains("CHECK constraint failed"));
    connection.close().await.unwrap();
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

/// Uses the standard installed runtime and its real mounted schema9 journal. No fixture
/// constructor/path override is used. Execute only after the launcher qualification cases.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "owned credential-free installed systemd guest only"]
async fn installed_quiescence_tracks_journal_receipts_exports_and_flush() {
    use crate::guest_boundary::{QuiescenceState, storage};
    use sqlx::Connection;
    assert_eq!(
        std::fs::read_to_string("/etc/craxii-v001-fixture-instance")
            .unwrap()
            .trim(),
        "13131313-1313-4313-8313-131313131314"
    );
    storage::require_worker_principal().unwrap();
    let guest = storage::load_installed().unwrap();
    let key = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let power = request_id().unwrap();
    let runtime = DistributionRuntime::open(DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://127.0.0.1:19443".into(),
        state_root: storage::STATE.into(),
        artifact_root: storage::ARTIFACTS.into(),
        workspace_root: storage::WORKSPACE.into(),
        profile_public_key: URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),
        power_operation_id: power.clone(),
        release_id: "qualification-only".into(),
        release_sha256: "1".repeat(64),
        model_proxy: None,
        agent_limits: None,
    })
    .await
    .unwrap();
    let mut f = Fixture {
        root: "/no-generic-installed-fixture-cleanup".into(),
        boot: runtime.boot_id.clone(),
        runtime: Some(runtime),
        key,
        authority: json!({"customer_id":guest.text("customer_id"),"logical_agent_id":guest.text("logical_agent_id"),"installation_id":guest.text("installation_id"),"generation":guest.text("generation"),"session_id":request_id().unwrap()}),
        power,
        conversation: request_id().unwrap(),
    };
    // Qualification restarts preserve the existing journal. Reuse its established central
    // conversation and next sequence instead of attempting a conflicting second mapping.
    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT central_conversation_id,next_sequence FROM distribution_conversations",
    )
    .fetch_optional(&f.runtime.as_ref().unwrap().store.runtime.inner.pool)
    .await
    .unwrap();
    let sequence = if let Some((conversation, next)) = existing {
        f.conversation = conversation;
        next
    } else {
        1
    };
    let rt = f.runtime.as_ref().unwrap();
    assert_eq!(
        rt.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    let mut reader =
        sqlx::SqliteConnection::connect("sqlite:///var/lib/craxii/db/craxii.sqlite3?mode=ro")
            .await
            .unwrap();
    sqlx::query("BEGIN").execute(&mut reader).await.unwrap();
    sqlx::query("SELECT count(*) FROM journal_events")
        .fetch_one(&mut reader)
        .await
        .unwrap();
    let work = f.delivery(&request_id().unwrap(), sequence);
    rt.store.admit_distribution(&work).await.unwrap();
    let q = rt.quiescence_observation().await.unwrap();
    assert!(q.active_tasks > 0 && q.pending_receipts > 0);
    assert_eq!(q.state(), QuiescenceState::Busy);
    rt.store
        .cancel_distribution(&f.control(&work))
        .await
        .unwrap();
    for admission in rt.store.pending_distribution_receipts().await.unwrap() {
        let DistributionAdmission::Receipt { receipt, .. } = admission else {
            panic!("unexpected receipt type");
        };
        rt.store
            .acknowledge_distribution(
                crate::distribution_contracts::environment::WorkerEnvironment::Development,
                &receipt,
                false,
            )
            .await
            .unwrap();
    }
    for receipt in rt.store.pending_distribution_controls().await.unwrap() {
        rt.store
            .acknowledge_distribution(
                crate::distribution_contracts::environment::WorkerEnvironment::Development,
                &receipt,
                true,
            )
            .await
            .unwrap();
    }
    let pending = rt.store.pending_distribution_results().await.unwrap();
    assert_eq!(pending.len(), 1);
    let result = &pending[0];
    rt.store
        .record_distribution_result_transfer_failure(
            result["snapshot"]["export_id"].as_str().unwrap(),
            true,
            false,
        )
        .await
        .unwrap();
    let q = rt.quiescence_observation().await.unwrap();
    assert!(q.pending_exports > 0 && q.uncertain_exports > 0);
    assert_eq!(q.state(), QuiescenceState::Busy);
    let ack = json!({"work_id":result["snapshot"]["work_id"],"export_id":result["snapshot"]["export_id"],"export_revision":"1","export_digest":result["export_digest"],"acceptance_id":request_id().unwrap(),"outbox_source_id":request_id().unwrap(),"accepted_at":crate::distribution::timestamp().unwrap().to_string(),"duplicate":false});
    rt.store
        .acknowledge_distribution_result(&ack)
        .await
        .unwrap();
    let q = rt.quiescence_observation().await.unwrap();
    assert!(!q.journal_flushed);
    assert_eq!(q.state(), QuiescenceState::Unknown);
    sqlx::query("ROLLBACK").execute(&mut reader).await.unwrap();
    reader.close().await.unwrap();
    assert_eq!(
        rt.quiescence_observation().await.unwrap().state(),
        QuiescenceState::LocallyQuiescent
    );
    f.runtime.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn distribution_startup_verified_snapshot_rejects_corruption_before_recovery() {
    use sqlx::{ConnectOptions, Connection};
    for corrupt_payload in [true, false] {
        let mut f = Fixture::new().await;
        f.runtime.take().unwrap().shutdown().await.unwrap();
        let mut c = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(f.root.join("state/db/craxii.sqlite3"))
            .connect()
            .await
            .unwrap();
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_instances")
            .fetch_one(&mut c)
            .await
            .unwrap();
        if corrupt_payload {
            sqlx::query("UPDATE journal_events SET payload_sha256=? WHERE journal_offset=1")
                .bind("a".repeat(64))
                .execute(&mut c)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE runtime_instances SET binary_version='0.0.2'")
                .execute(&mut c)
                .await
                .unwrap();
        }
        c.close().await.unwrap();
        let result = DistributionRuntime::isolated_fixture(
            DistributionConfig {
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
                model_proxy: None,
                agent_limits: None,
            },
            request_id().unwrap(),
        )
        .await;
        assert!(matches!(result, Err(DistributionError::Journal)));
        let mut c = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(f.root.join("state/db/craxii.sqlite3"))
            .connect()
            .await
            .unwrap();
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_instances")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert_eq!(after, before, "corruption reached runtime recovery/start");
        c.close().await.unwrap();
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}
impl Fixture {
    /// B1: the production form of a work delivery. It names the boot's compute grant, period and epoch, and
    /// carries no profile.
    fn ordinary_delivery(&self, id: &str, sequence: i64) -> ValidatedDelivery {
        let mut d = self.delivery(id, sequence);
        d.message["body"]["execution_authorization"] = json!({"kind":"ordinary","period_id":request_id().unwrap(),"compute_grant_id":request_id().unwrap(),"authorization_epoch":"1"});
        d.profile = None;
        d
    }
}
#[tokio::test]
async fn ordinary_work_is_admitted_without_a_profile_and_replays_its_receipt() {
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let id = request_id().unwrap();
    let d = f.ordinary_delivery(&id, 1);
    let DistributionAdmission::Receipt {
        receipt,
        duplicate,
        state,
        ..
    } = store.admit_distribution(&d).await.unwrap()
    else {
        panic!("ordinary work must be receipted");
    };
    assert!(!duplicate);
    assert_eq!(state, "queued");
    assert_eq!(receipt["work_id"], id);
    // The same delivery again returns the same receipt.
    let DistributionAdmission::Receipt {
        receipt: again,
        duplicate,
        ..
    } = store.admit_distribution(&d).await.unwrap()
    else {
        panic!("replay must return the receipt");
    };
    assert!(duplicate);
    assert_eq!(again, receipt);
    // Ordinary work carrying a profile, or naming no grant, is refused and changes nothing.
    let mut with_profile = f.ordinary_delivery(&request_id().unwrap(), 2);
    with_profile.profile = f.delivery(&request_id().unwrap(), 2).profile;
    assert_eq!(
        store.admit_distribution(&with_profile).await.unwrap_err(),
        DistributionError::Invalid
    );
    let mut unnamed = f.ordinary_delivery(&request_id().unwrap(), 2);
    unnamed.message["body"]["execution_authorization"]["kind"] = json!("qualified_candidate");
    assert_eq!(
        store.admit_distribution(&unnamed).await.unwrap_err(),
        DistributionError::Invalid
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM distribution_receipts")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        1
    );
    f.close().await;
}
#[tokio::test]
async fn production_refuses_fixture_work_and_ordinary_work_it_cannot_check() {
    let mut f = Fixture::new().await;
    let production = crate::distribution_contracts::environment::WorkerEnvironment::Production;
    let mut fixture = f.delivery(&request_id().unwrap(), 1);
    fixture.environment = production;
    assert_eq!(
        fixture.verify_authorization().unwrap_err(),
        DistributionError::Invalid
    );
    // Without the installed compute authority there is no grant to check ordinary work against.
    let mut ordinary = f.ordinary_delivery(&request_id().unwrap(), 1);
    ordinary.environment = production;
    assert_eq!(
        ordinary.verify_authorization().unwrap_err(),
        DistributionError::Invalid
    );
    // A development build admits both.
    f.delivery(&request_id().unwrap(), 1)
        .verify_authorization()
        .unwrap();
    f.ordinary_delivery(&request_id().unwrap(), 1)
        .verify_authorization()
        .unwrap();
    f.close().await;
}
#[tokio::test]
async fn a_production_poll_drops_development_profiles_instead_of_failing() {
    let mut f = Fixture::new().await;
    let d = f.delivery(&request_id().unwrap(), 1);
    let id = request_id().unwrap();
    let response = json!({"contract_version":1,"request_id":id,"items":[d.profile.clone().unwrap(),d.message.clone()],"retry_after_ms":1000});
    use crate::distribution_contracts::environment::WorkerEnvironment;
    let dev = crate::distribution::transport::poll_response(
        &response,
        &id,
        WorkerEnvironment::Development,
    )
    .unwrap();
    assert_eq!(dev.len(), 2);
    let prod = crate::distribution::transport::poll_response(
        &response,
        &id,
        WorkerEnvironment::Production,
    )
    .unwrap();
    assert_eq!(prod.len(), 1);
    assert_eq!(prod[0]["type"], "work_delivery");
    f.close().await;
}
#[tokio::test]
async fn ordinary_material_for_cancelled_work_is_admitted_as_cancelled() {
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    // The cancel arrives first; the work's material then reconciles its sequence. Its grant need not list it.
    let work = f.ordinary_delivery(&request_id().unwrap(), 1);
    store.cancel_distribution(&f.control(&work)).await.unwrap();
    assert!(matches!(
        store.admit_distribution(&work).await.unwrap(),
        DistributionAdmission::Cancelled { .. }
    ));
    f.close().await;
}
#[test]
fn the_vm_config_may_name_the_model_and_a_malformed_one_is_refused() {
    let base = json!({"contract_version":1,"environment":"production","origin":"https://api.craxii.com/","state_root":"/var/lib/craxii-worker/state","artifact_root":"/var/lib/craxii-worker/artifacts","workspace_root":"/srv/craxii-data/workspace","profile_public_key":URL_SAFE_NO_PAD.encode([7u8;32]),"power_operation_id":request_id().unwrap(),"release_id":"worker-1","release_sha256":"a".repeat(64)});
    // A config written before B1.6 still loads, and names no model.
    let old: DistributionConfig = serde_json::from_value(base.clone()).unwrap();
    old.validate().unwrap();
    assert!(old.model_proxy.is_none());
    assert!(!serde_json::to_string(&old).unwrap().contains("model_proxy"));
    let model = json!({"contract_version":1,"model_id":"agent-default","model_config_id":request_id().unwrap(),"model_config_digest":"b".repeat(64),"price_config_id":request_id().unwrap(),"price_config_digest":"c".repeat(64),"max_output_tokens":4096});
    let mut with = base.clone();
    with["model_proxy"] = model.clone();
    let named: DistributionConfig = serde_json::from_value(with).unwrap();
    named.validate().unwrap();
    assert_eq!(
        named.model_proxy.as_ref().unwrap().model_id,
        "agent-default"
    );
    for (k, v) in [
        ("model_config_digest", json!("B".repeat(64))),
        ("max_output_tokens", json!(0)),
        ("model_id", json!("no spaces allowed")),
    ] {
        let mut bad = base.clone();
        bad["model_proxy"] = model.clone();
        bad["model_proxy"][k] = v;
        let c: DistributionConfig = serde_json::from_value(bad).unwrap();
        assert!(c.validate().is_err(), "{k}");
    }
}
#[test]
fn a_grant_is_adopted_only_as_a_later_named_successor_that_ends_no_earlier() {
    use crate::compute::authorization::adoptable_successor;
    let (boot, reservation) = (request_id().unwrap(), request_id().unwrap());
    let held = json!({"grant_id":request_id().unwrap(),"boot_id":boot,"compute_reservation_id":reservation,"issued_at":"2026-11-25T10:00:00.000000Z"});
    let named = request_id().unwrap();
    let next = |issued: &str, reservation: &str| json!({"grant_id":named,"boot_id":boot,"compute_reservation_id":reservation,"issued_at":issued});
    let (later, same_time) = ("2026-11-25T10:00:00.000001Z", "2026-11-25T10:00:00.000000Z");
    let cutoff = "1795600000000000";
    // A superseding grant: same cutoff and reservation, issued later.
    assert!(adoptable_successor(
        &named,
        &next(later, &reservation),
        cutoff,
        &held,
        cutoff
    ));
    // A renewal: a later cutoff, a new reservation.
    assert!(adoptable_successor(
        &named,
        &next(later, &request_id().unwrap()),
        "1795600000000001",
        &held,
        cutoff
    ));
    // Refused: another grant than the one named, another boot, not issued later, ending earlier,
    // or the same cutoff with another reservation, or unreadable times.
    assert!(!adoptable_successor(
        &request_id().unwrap(),
        &next(later, &reservation),
        cutoff,
        &held,
        cutoff
    ));
    let mut other_boot = next(later, &reservation);
    other_boot["boot_id"] = json!(request_id().unwrap());
    assert!(!adoptable_successor(
        &named,
        &other_boot,
        cutoff,
        &held,
        cutoff
    ));
    assert!(!adoptable_successor(
        &named,
        &next(same_time, &reservation),
        cutoff,
        &held,
        cutoff
    ));
    assert!(!adoptable_successor(
        &named,
        &next(later, &reservation),
        "1795599999999999",
        &held,
        cutoff
    ));
    assert!(!adoptable_successor(
        &named,
        &next(later, &request_id().unwrap()),
        cutoff,
        &held,
        cutoff
    ));
    assert!(!adoptable_successor(
        &named,
        &next("not a time", &reservation),
        cutoff,
        &held,
        cutoff
    ));
    assert!(!adoptable_successor(
        &named,
        &next(later, &reservation),
        "x",
        &held,
        cutoff
    ));
}
#[tokio::test]
async fn a_production_poll_still_fails_on_a_malformed_item_that_is_not_a_profile() {
    let mut f = Fixture::new().await;
    let d = f.delivery(&request_id().unwrap(), 1);
    let mut broken = d.message.clone();
    broken["body"]["material_digest"] = json!("not a digest");
    let id = request_id().unwrap();
    let response =
        json!({"contract_version":1,"request_id":id,"items":[broken],"retry_after_ms":1000});
    use crate::distribution_contracts::environment::WorkerEnvironment;
    assert!(
        crate::distribution::transport::poll_response(
            &response,
            &id,
            WorkerEnvironment::Production
        )
        .is_err()
    );
    f.close().await;
}
#[tokio::test]
async fn ordinary_work_keeps_its_authorization_and_is_claimed_only_through_the_gate() {
    use crate::adapters::sqlite::OrdinaryAuthorization;
    use crate::ports::state_store::{ClaimNextWorkRequest, SchedulerStateStore};
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let id = request_id().unwrap();
    let d = f.ordinary_delivery(&id, 1);
    store.admit_distribution(&d).await.unwrap();
    let a = &d.message["body"]["execution_authorization"];
    let stored = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT central_work_id,period_id,compute_grant_id,authorization_epoch FROM distribution_authorizations",
    )
    .fetch_all(&store.runtime.inner.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![(
            id.clone(),
            a["period_id"].as_str().unwrap().into(),
            a["compute_grant_id"].as_str().unwrap().into(),
            "1".into()
        )]
    );
    // A development fixture stores none.
    store
        .admit_distribution(&f.delivery(&request_id().unwrap(), 2))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM distribution_authorizations")
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        1
    );
    // The stored authorization never changes and is never removed.
    for sql in [
        "UPDATE distribution_authorizations SET authorization_epoch='2'",
        "DELETE FROM distribution_authorizations",
    ] {
        assert!(
            sqlx::query(sql)
                .execute(&store.runtime.inner.pool)
                .await
                .is_err(),
            "{sql}"
        );
    }
    let runtime = sqlx::query_scalar::<_, String>(
        "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
    )
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    let request = || ClaimNextWorkRequest {
        runtime_id: runtime.parse().unwrap(),
        claimed_at: crate::distribution::timestamp().unwrap(),
        event_id: crate::domain::JournalEventId::generate(),
    };
    // The plain scheduler claim never takes distribution work.
    assert!(store.claim_next_work(request()).await.unwrap().is_none());
    // A gate that refuses (no current grant lists it): the head waits, still queued.
    let refuse = |_: &OrdinaryAuthorization| false;
    assert!(
        store
            .claim_next_ordinary(request(), &refuse)
            .await
            .unwrap()
            .is_none()
    );
    let local: String = sqlx::query_scalar(
        "SELECT local_work_id FROM distribution_receipts WHERE central_work_id=?",
    )
    .bind(&id)
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM work_items WHERE work_id=?")
            .bind(&local)
            .fetch_one(&store.runtime.inner.pool)
            .await
            .unwrap(),
        "queued"
    );
    // A gate that accepts gets exactly the stored authorization, and the task is claimed.
    let seen = std::sync::Mutex::new(None);
    let accept = |o: &OrdinaryAuthorization| {
        *seen.lock().unwrap() = Some(o.clone());
        true
    };
    let claimed = store
        .claim_next_ordinary(request(), &accept)
        .await
        .unwrap()
        .expect("claimed");
    assert_eq!(claimed.work.work_id().to_string(), local);
    assert_eq!(seen.lock().unwrap().as_ref().unwrap().central_work_id, id);
    // Only one task runs at a time: nothing else is claimed while it runs.
    assert!(
        store
            .claim_next_ordinary(request(), &accept)
            .await
            .unwrap()
            .is_none()
    );
    f.close().await;
}
#[tokio::test]
async fn an_ordinary_task_gets_its_tool_permit_material_only_while_it_runs() {
    use crate::adapters::sqlite::OrdinaryAuthorization;
    use crate::ports::state_store::ClaimNextWorkRequest;
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let id = request_id().unwrap();
    let d = f.ordinary_delivery(&id, 1);
    store.admit_distribution(&d).await.unwrap();
    let local: String = sqlx::query_scalar(
        "SELECT local_work_id FROM distribution_receipts WHERE central_work_id=?",
    )
    .bind(&id)
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    let work = crate::domain::WorkId::parse_canonical(&local).unwrap();
    // Queued: no permit.
    assert_eq!(
        store.ordinary_permit_material(work).await.unwrap_err(),
        DistributionError::Invalid
    );
    let runtime = sqlx::query_scalar::<_, String>(
        "SELECT runtime_instance_id FROM runtime_instances WHERE state='running'",
    )
    .fetch_one(&store.runtime.inner.pool)
    .await
    .unwrap();
    let accept = |_: &OrdinaryAuthorization| true;
    store
        .claim_next_ordinary(
            ClaimNextWorkRequest {
                runtime_id: runtime.parse().unwrap(),
                claimed_at: crate::distribution::timestamp().unwrap(),
                event_id: crate::domain::JournalEventId::generate(),
            },
            &accept,
        )
        .await
        .unwrap()
        .expect("claimed");
    // Running: the central work id and the grant it arrived under.
    let grant = d.message["body"]["execution_authorization"]["compute_grant_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        store.ordinary_permit_material(work).await.unwrap(),
        (id, grant)
    );
    // A fixture task has no ordinary permit.
    assert_eq!(
        store
            .ordinary_permit_material(crate::domain::WorkId::generate())
            .await
            .unwrap_err(),
        DistributionError::Invalid
    );
    f.close().await;
}
#[tokio::test]
async fn an_ordinary_model_binding_has_no_profile_and_names_the_stored_authorization() {
    use crate::adapters::model_proxy::ProxyBinding;
    use crate::adapters::sqlite::OrdinaryAuthorization;
    use crate::distribution::model_config::{ModelProxySettings, ORDINARY_ESTIMATOR};
    let mut f = Fixture::new().await;
    let store = &f.runtime.as_ref().unwrap().store;
    let id = request_id().unwrap();
    let d = f.ordinary_delivery(&id, 1);
    let DistributionAdmission::Receipt { receipt, .. } =
        store.admit_distribution(&d).await.unwrap()
    else {
        panic!("receipt")
    };
    let a = &d.message["body"]["execution_authorization"];
    let auth = OrdinaryAuthorization {
        central_work_id: id.clone(),
        period_id: a["period_id"].as_str().unwrap().into(),
        compute_grant_id: a["compute_grant_id"].as_str().unwrap().into(),
        authorization_epoch: "1".into(),
    };
    let settings: ModelProxySettings = serde_json::from_value(json!({"contract_version":1,"model_id":"agent-default","model_config_id":request_id().unwrap(),"model_config_digest":"b".repeat(64),"price_config_id":request_id().unwrap(),"price_config_digest":"c".repeat(64),"max_output_tokens":4096,"requested_output_tokens":2048,"context_window_tokens":200000})).unwrap();
    let env = crate::distribution_contracts::environment::WorkerEnvironment::Development;
    let binding = ProxyBinding::ordinary(env, receipt.clone(), &auth, settings.clone()).unwrap();
    assert_eq!(
        binding.named(),
        (
            id.clone(),
            auth.period_id.clone(),
            auth.compute_grant_id.clone(),
            settings.model_config_id.clone(),
            None,
            true
        )
    );
    // Another task's authorization, or a bad epoch, is refused.
    let other = OrdinaryAuthorization {
        central_work_id: request_id().unwrap(),
        ..auth.clone()
    };
    assert!(ProxyBinding::ordinary(env, receipt.clone(), &other, settings.clone()).is_err());
    let bad_epoch = OrdinaryAuthorization {
        authorization_epoch: "x".into(),
        ..auth.clone()
    };
    assert!(ProxyBinding::ordinary(env, receipt, &bad_epoch, settings.clone()).is_err());
    // The ordinary target names the conservative estimator and the configured sizes.
    let target = settings.ordinary_target().unwrap();
    assert_eq!(
        target.estimator(),
        &crate::domain::TokenEstimatorIdentity::try_new(ORDINARY_ESTIMATOR, 1).unwrap()
    );
    assert_eq!(target.requested_output_tokens().get(), 2048);
    assert_eq!(
        target
            .reference()
            .capabilities()
            .context_window_tokens()
            .get(),
        200000
    );
    assert_eq!(settings.call_timeout_seconds(), 600);
    // Out-of-range settings are refused.
    for (k, v) in [
        ("requested_output_tokens", json!(5000)),
        ("context_window_tokens", json!(10)),
        ("call_timeout_seconds", json!(1)),
    ] {
        let mut bad = serde_json::to_value(&settings).unwrap();
        bad[k] = v;
        let s: ModelProxySettings = serde_json::from_value(bad).unwrap();
        assert!(s.validate().is_err(), "{k}");
    }
    f.close().await;
}
#[test]
fn the_vm_config_may_set_agent_limits_and_bad_values_are_refused() {
    let base = json!({"contract_version":1,"environment":"production","origin":"https://api.craxii.com/","state_root":"/var/lib/craxii-worker/state","artifact_root":"/var/lib/craxii-worker/artifacts","workspace_root":"/srv/craxii-data/workspace","profile_public_key":URL_SAFE_NO_PAD.encode([7u8;32]),"power_operation_id":request_id().unwrap(),"release_id":"worker-1","release_sha256":"a".repeat(64)});
    let none: DistributionConfig = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(
        none.agent_limits(),
        crate::distribution::startup::AgentLimits::default()
    );
    let mut set = base.clone();
    set["agent_limits"] = json!({"tool_call_timeout_seconds":600,"repeated_failure_limit":0});
    let c: DistributionConfig = serde_json::from_value(set).unwrap();
    c.validate().unwrap();
    assert_eq!(c.agent_limits().tool_call_timeout_ms(), 600_000);
    assert_eq!(c.agent_limits().loop_limits().repeated_failure_limit, 0);
    for bad in [
        json!({"tool_call_timeout_seconds":901}),
        json!({"task_duration_seconds":30}),
        json!({"max_model_calls":0}),
        json!({"repeated_failure_limit":1}),
    ] {
        let mut v = base.clone();
        v["agent_limits"] = bad.clone();
        let c: DistributionConfig = serde_json::from_value(v).unwrap();
        assert!(c.validate().is_err(), "{bad}");
    }
    let mut unknown = base;
    unknown["agent_limits"] = json!({"budget":1});
    assert!(serde_json::from_value::<DistributionConfig>(unknown).is_err());
}
