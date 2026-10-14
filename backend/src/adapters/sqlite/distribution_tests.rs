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
        let root =
            std::env::temp_dir().join(format!("craxii-distribution-{}", uuid::Uuid::now_v7()));
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
