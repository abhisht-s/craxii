//! Separate composition entered before legacy credential discovery and listeners.
use super::{DistributionError, DistributionResult, invalid, timestamp};
use crate::adapters::{
    artifacts::LocalArtifactStore,
    sqlite::{DistributionAdmission, SqliteRuntimeGuard, SqliteStateStore},
    system_clock::SystemClock,
};
use crate::application::runtime::{RuntimeBootstrapReceipt, bootstrap_runtime};
use crate::domain::*;
use crate::ports::{
    artifact_store::ArtifactStore,
    runtime_observation::RuntimeProcessObserver,
    state_store::{
        BeginRuntimeStoppingRequest, BootstrapObservation, BootstrapStateStore,
        ExecutionCapabilityObservation, FinishRuntimeRequest, HeartbeatRuntimeRequest,
        LoadOrBootstrapIdentityRequest, RuntimeStateStore, V0IdentityReference,
    },
};
use crate::worker_auth::{
    AuthError, client::WorkerClient, key_store::KeyStore, state::IdentityState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionConfig {
    pub contract_version: u32,
    pub environment: String,
    pub origin: String,
    pub state_root: String,
    pub artifact_root: String,
    pub workspace_root: String,
    pub profile_public_key: String,
    pub power_operation_id: String,
    pub release_id: String,
    pub release_sha256: String,
}
impl DistributionConfig {
    pub fn validate(&self) -> DistributionResult<()> {
        if self.contract_version != 1
            || self.environment != "development"
            || self.release_id.is_empty()
            || self.release_id.len() > 128
        {
            return Err(invalid());
        }
        crate::distribution_contracts::primitives::binary(&self.profile_public_key, Some(32))
            .map_err(|_| invalid())?;
        crate::distribution_contracts::primitives::uuid(&self.power_operation_id, Some(4))
            .map_err(|_| invalid())?;
        if self.release_sha256.len() != 64
            || !self
                .release_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid());
        }
        for p in [&self.state_root, &self.artifact_root, &self.workspace_root] {
            if !Path::new(p).is_absolute() {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
struct ExportFlight<'a>(&'a std::sync::atomic::AtomicUsize);
impl Drop for ExportFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
pub struct DistributionRuntime {
    pub store: SqliteStateStore,
    guard: SqliteRuntimeGuard,
    runtime: RuntimeBootstrapReceipt,
    config: DistributionConfig,
    pub boot_id: String,
    in_flight_exports: std::sync::atomic::AtomicUsize,
    boundary: Option<crate::guest_boundary::config::GuestConfig>,
    #[cfg(target_os = "linux")]
    compute: Option<std::sync::Arc<crate::compute::worker::InstalledCompute>>,
}
impl DistributionRuntime {
    pub async fn open(config: DistributionConfig) -> DistributionResult<Self> {
        crate::guest_boundary::storage::harden_worker_process()
            .map_err(|_| DistributionError::Storage)?;
        config.validate()?;
        let boundary = crate::guest_boundary::storage::load_installed()
            .map_err(|_| DistributionError::Storage)?;
        crate::guest_boundary::storage::verify_installed(&boundary)
            .map_err(|_| DistributionError::Storage)?;
        if config.state_root != crate::guest_boundary::storage::STATE
            || config.artifact_root != crate::guest_boundary::storage::ARTIFACTS
            || config.workspace_root != crate::guest_boundary::storage::WORKSPACE
        {
            return Err(DistributionError::Storage);
        }
        let observation = crate::adapters::runtime_observation::SystemRuntimeProcessObserver
            .observe()
            .map_err(|_| invalid())?;
        Self::open_inner(
            config,
            observation.linux_boot_id.as_str().to_owned(),
            Some(observation.process_id),
            Some(boundary),
        )
        .await
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub async fn isolated_fixture(
        config: DistributionConfig,
        synthetic_boot_id: String,
    ) -> DistributionResult<Self> {
        crate::distribution_contracts::primitives::uuid(&synthetic_boot_id, None)
            .map_err(|_| invalid())?;
        Self::open_inner(
            config,
            synthetic_boot_id,
            Some(DiagnosticPid::try_new(i64::from(std::process::id())).map_err(|_| invalid())?),
            None,
        )
        .await
    }
    async fn open_inner(
        config: DistributionConfig,
        boot_id: String,
        pid: Option<DiagnosticPid>,
        boundary: Option<crate::guest_boundary::config::GuestConfig>,
    ) -> DistributionResult<Self> {
        config.validate()?;
        if let Some(c) = &boundary {
            crate::guest_boundary::storage::prepare_journal_open(c)
                .map_err(|_| DistributionError::Storage)?;
        }
        let guard = SqliteRuntimeGuard::start(Path::new(&config.state_root), 2)
            .await
            .map_err(|_| DistributionError::Sqlite)?;
        let artifacts = LocalArtifactStore::initialize(Path::new(&config.artifact_root))
            .map_err(|_| DistributionError::Storage)?;
        let workspace = std::fs::canonicalize(&config.workspace_root).map_err(|_| invalid())?;
        let store = SqliteStateStore::new(guard.runtime().clone());
        let at = timestamp()?;
        store
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
                created_at: at,
                observation: BootstrapObservation {
                    initial_generation: WorkstationGeneration::try_new(1).map_err(|_| invalid())?,
                    architecture: std::env::consts::ARCH.to_owned(),
                    os_release: std::env::consts::OS.to_owned(),
                    default_shell: "/bin/sh".to_owned(),
                    workspace_logical_name: "primary".to_owned(),
                    workspace_logical_root: config.workspace_root.clone(),
                    workspace_resolved_root: workspace.to_str().ok_or_else(invalid)?.to_owned(),
                    execution_capabilities: ExecutionCapabilityObservation::unavailable(),
                },
            })
            .await
            .map_err(|_| DistributionError::Storage)?;
        store
            .verify_application_consistency()
            .await
            .map_err(|_| DistributionError::Journal)?;
        store
            .reconcile_distribution_results()
            .await
            .map_err(|_| DistributionError::Reconciliation)?;
        store.verify_distribution_consistency().await?;
        store
            .verify_distribution_results()
            .await
            .map_err(|_| DistributionError::Reconciliation)?;
        let refs = store
            .load_referenced_artifacts()
            .await
            .map_err(|_| DistributionError::Storage)?;
        let mut keys = BTreeSet::new();
        for artifact in &refs {
            artifacts
                .verify(artifact)
                .map_err(|_| DistributionError::Storage)?;
            keys.insert(artifact.storage_key().clone());
        }
        let orphans = artifacts
            .scan_orphans(&keys, at)
            .map_err(|_| DistributionError::Artifacts)?;
        let snapshot = store
            .load_bootstrap_snapshot()
            .await
            .map_err(|_| DistributionError::Storage)?;
        let build = crate::bootstrap::metadata::BuildMetadata::embedded().map_err(|_| invalid())?;
        let evidence = RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: RuntimeInstanceId::generate(),
            craxii_id: snapshot.identity.craxii_id,
            workstation_id: snapshot.identity.workstation_id,
            workstation_generation: snapshot.workstation.generation(),
            linux_boot_id: Some(LinuxBootId::try_new(boot_id.clone()).map_err(|_| invalid())?),
            diagnostic_pid: pid,
            package_version: PackageVersion::try_new(build.package_version())
                .map_err(|_| invalid())?,
            git_revision: GitRevision::try_new(build.git_revision()).map_err(|_| invalid())?,
            schema_version: SchemaVersion::try_new(9).map_err(|_| invalid())?,
            started_at: timestamp()?,
        });
        let runtime = bootstrap_runtime(
            &store,
            evidence,
            orphans.orphans.len() as u64,
            &SystemClock::new(),
        )
        .await
        .map_err(|_| DistributionError::Recovery)?;
        store
            .verify_application_consistency()
            .await
            .map_err(|_| DistributionError::Journal)?;
        store
            .reconcile_distribution_results()
            .await
            .map_err(|_| DistributionError::Reconciliation)?;
        store.verify_distribution_consistency().await?;
        store
            .verify_distribution_results()
            .await
            .map_err(|_| DistributionError::Reconciliation)?;
        #[cfg(target_os = "linux")]
        let compute = if let Some(g) = &boundary {
            Some(std::sync::Arc::new(
                crate::compute::worker::InstalledCompute::load(
                    g.text("customer_id"),
                    g.text("logical_agent_id"),
                    g.text("installation_id"),
                    g.text("generation"),
                    &config.power_operation_id,
                )
                .map_err(|_| DistributionError::Storage)?,
            ))
        } else {
            None
        };
        // No AgentLoop, model/tool provider, executable scheduler or legacy delivery worker.
        Ok(Self {
            store,
            guard,
            runtime,
            config,
            boot_id,
            in_flight_exports: std::sync::atomic::AtomicUsize::new(0),
            boundary,
            #[cfg(target_os = "linux")]
            compute,
        })
    }
    fn recheck_boundary(&self) -> DistributionResult<()> {
        if let Some(c) = &self.boundary {
            crate::guest_boundary::storage::verify_installed(c)
                .map_err(|_| DistributionError::Storage)?;
        }
        Ok(())
    }
    pub fn hello(&self) -> Value {
        #[allow(unused_mut)]
        let mut hello = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"hello","body":{"release_id":self.config.release_id,"release_sha256":self.config.release_sha256,"supported_versions":[{"major":1,"minor":0}],"capabilities":["status_v1","signed_request_v1","admission_v1","cancel_v1","result_v1"],"canonicalization_versions":["craxii.cjson/1"],"signature_versions":["craxii.signature/1"],"sqlite_schema":"9","boot_id":self.boot_id,"runtime_id":self.runtime.runtime_instance_id.to_string()}});
        #[cfg(target_os = "linux")]
        if self.compute.is_some() {
            hello["body"]["capabilities"]
                .as_array_mut()
                .expect("hello capabilities")
                .push(json!("compute_v1"));
        }
        hello
    }
    pub async fn reconnect(&self, client: &mut WorkerClient) -> Result<(), AuthError> {
        self.recheck_boundary()
            .map_err(|_| AuthError::Unavailable)?;
        client.negotiate(&self.hello(), None).await?;
        client.renew().await?;
        client.status().await?;
        self.store
            .assert_distribution_identity(
                &client.identity().customer_id,
                &client.identity().logical_agent_id,
            )
            .await
            .map_err(|_| AuthError::InvalidContext)?;
        Ok(())
    }
    pub async fn transfer_pending(&self, client: &mut WorkerClient) -> Result<(), AuthError> {
        for receipt in self
            .store
            .pending_distribution_receipts()
            .await
            .map_err(|_| AuthError::Unavailable)?
        {
            self.transfer(client, receipt).await?;
        }
        for body in self
            .store
            .pending_distribution_controls()
            .await
            .map_err(|_| AuthError::Unavailable)?
        {
            client.send_receipt(&body, true).await?;
            self.store
                .acknowledge_distribution(&body, true)
                .await
                .map_err(|_| AuthError::Unavailable)?;
        }
        for body in self
            .store
            .pending_distribution_results()
            .await
            .map_err(|_| AuthError::Unavailable)?
        {
            self.in_flight_exports
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _flight = ExportFlight(&self.in_flight_exports);
            // Persist ambiguity before network I/O. SIGTERM, timeout or process death
            // cannot make a possibly accepted export look definitely unsent.
            self.store
                .record_distribution_result_transfer_failure(
                    body["snapshot"]["export_id"]
                        .as_str()
                        .ok_or(AuthError::InvalidContext)?,
                    true,
                    false,
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            let result = async {
                let ack = match client.query_result(&body).await {
                    Ok(ack) => ack,
                    Err(AuthError::ResultMissing) => client.send_result(&body).await?,
                    Err(e) => return Err(e),
                };
                self.store
                    .acknowledge_distribution_result(&ack["body"])
                    .await
                    .map_err(|_| AuthError::Unavailable)
            }
            .await;
            if let Err(error) = result {
                self.store
                    .record_distribution_result_transfer_failure(
                        body["snapshot"]["export_id"]
                            .as_str()
                            .ok_or(AuthError::InvalidContext)?,
                        // Once I/O may have begun, even a malformed acknowledgement cannot
                        // prove the central import did not commit. Preserve ambiguity.
                        true,
                        error == AuthError::ResultConflict,
                    )
                    .await
                    .map_err(|_| AuthError::Unavailable)?;
                return Err(error);
            }
        }
        Ok(())
    }
    pub async fn export_observation(&self) -> DistributionResult<Value> {
        let mut observation = self.store.distribution_observation().await?;
        let in_flight = self
            .in_flight_exports
            .load(std::sync::atomic::Ordering::SeqCst);
        observation["in_flight_export_count"] = json!(in_flight.to_string());
        observation["export_drained"] = json!(
            in_flight == 0
                && observation["pending_export_count"] == "0"
                && observation["unresolved_export_count"] == "0"
        );
        Ok(observation)
    }
    pub async fn quiescence_observation(
        &self,
    ) -> DistributionResult<crate::guest_boundary::QuiescenceObservation> {
        let v = self.export_observation().await?;
        let count = |name: &str| {
            v[name]
                .as_str()
                .and_then(|n| n.parse::<u64>().ok())
                .ok_or(DistributionError::Storage)
        };
        let checkpoint = self
            .guard
            .runtime()
            .checkpoint_passive()
            .await
            .map_err(|_| DistributionError::Sqlite)?;
        let descendants = crate::guest_boundary::storage::tool_descendant_count().ok();
        let cleanup = crate::guest_boundary::storage::pending_execution_cleanup().ok();
        Ok(crate::guest_boundary::QuiescenceObservation {
            active_tasks: count("active_work_count")?.saturating_add(count("queued_work_count")?),
            descendants,
            pending_cleanup: count("cancel_requested_count")?.saturating_add(cleanup.unwrap_or(0)),
            pending_receipts: count("pending_receipt_ack_count")?
                .saturating_add(count("pending_control_ack_count")?),
            pending_exports: count("pending_export_count")?
                .saturating_add(count("in_flight_export_count")?),
            uncertain_exports: count("uncertain_export_count")?
                .saturating_add(count("unresolved_export_count")?),
            journal_flushed: checkpoint.busy() == 0
                && checkpoint.log_frames() == checkpoint.checkpointed_frames(),
            storage_healthy: self.boundary.is_some()
                && self.recheck_boundary().is_ok()
                && crate::guest_boundary::storage::pressure().is_ok(),
            visibility_complete: descendants.is_some() && cleanup.is_some(),
        })
    }
    /// The receipt-only composition has no executor. A composition with tools must also
    /// merge its actual registry, including reservation and failed-cleanup observations.
    pub async fn quiescence_with_execution(
        &self,
        execution: crate::guest_boundary::ExecutionBoundaryObservation,
    ) -> DistributionResult<crate::guest_boundary::QuiescenceObservation> {
        let mut observed = self.quiescence_observation().await?;
        observed.active_tasks = observed
            .active_tasks
            .saturating_add(execution.active_executions);
        observed.pending_cleanup = observed
            .pending_cleanup
            .saturating_add(execution.pending_cleanup);
        observed.descendants = observed
            .descendants
            .zip(execution.descendants)
            .map(|(a, b)| a.max(b));
        observed.visibility_complete &= execution.visibility_complete;
        Ok(observed)
    }
    async fn transfer(
        &self,
        client: &mut WorkerClient,
        admitted: DistributionAdmission,
    ) -> Result<(), AuthError> {
        match admitted {
            DistributionAdmission::Receipt {
                receipt,
                duplicate,
                state,
                revision,
            } => {
                client.send_receipt(&json!({"receipt":receipt,"duplicate":duplicate,"current_work_state":state,"work_state_revision":revision}),false).await?;
                self.store
                    .acknowledge_distribution(&receipt, false)
                    .await
                    .map_err(|_| AuthError::Unavailable)?;
            }
            DistributionAdmission::Cancelled { control_receipt } => {
                client.send_receipt(&control_receipt, true).await?;
                self.store
                    .acknowledge_distribution(&control_receipt, true)
                    .await
                    .map_err(|_| AuthError::Unavailable)?;
            }
        }
        Ok(())
    }
    pub async fn poll_once(&self, client: &mut WorkerClient) -> Result<usize, AuthError> {
        self.recheck_boundary()
            .map_err(|_| AuthError::Unavailable)?;
        if self.boundary.is_some() {
            // Existing durable duties retain their narrow cleanup authority under disk pressure.
            self.transfer_pending(client).await?;
            crate::guest_boundary::storage::pressure().map_err(|_| AuthError::Unavailable)?;
        }
        #[allow(unused_mut)]
        let mut items = client
            .poll(
                &self.boot_id,
                &self.config.power_operation_id,
                &self.config.profile_public_key,
            )
            .await?;
        self.recheck_boundary()
            .map_err(|_| AuthError::Unavailable)?;
        #[cfg(target_os = "linux")]
        for item in &mut items {
            item.compute = self.compute.clone();
        }
        let count = items.len();
        // Commit controls first; a same-batch cancelled sequence is reconciled without a message.
        for item in items
            .iter()
            .filter(|m| m.message["type"] == "cancel_delivery")
        {
            self.store
                .cancel_distribution(item)
                .await
                .map_err(|_| AuthError::InvalidContext)?;
        }
        for item in items
            .iter()
            .filter(|m| m.message["type"] == "work_delivery")
        {
            self.store
                .admit_distribution(item)
                .await
                .map_err(|_| AuthError::InvalidContext)?;
        }
        self.transfer_pending(client).await?;
        self.store
            .heartbeat_runtime(HeartbeatRuntimeRequest {
                runtime_instance_id: self.runtime.runtime_instance_id,
                observed_at: timestamp().map_err(|_| AuthError::Unavailable)?,
            })
            .await
            .map_err(|_| AuthError::Unavailable)?;
        Ok(count)
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub async fn isolated_commit_without_ack(
        &self,
        client: &mut WorkerClient,
    ) -> Result<(), AuthError> {
        let items = client
            .poll(
                &self.boot_id,
                &self.config.power_operation_id,
                &self.config.profile_public_key,
            )
            .await?;
        for item in &items {
            if item.message["type"] == "work_delivery" {
                self.store
                    .admit_distribution(item)
                    .await
                    .map_err(|_| AuthError::InvalidContext)?;
                return Ok(());
            }
        }
        Err(AuthError::Unavailable)
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub async fn isolated_terminal_once(&self, client: &mut WorkerClient) -> Result<(), AuthError> {
        let items = client
            .poll(
                &self.boot_id,
                &self.config.power_operation_id,
                &self.config.profile_public_key,
            )
            .await?;
        let d = items
            .iter()
            .find(|d| d.message["type"] == "work_delivery")
            .ok_or(AuthError::Unavailable)?;
        d.verify_profile().map_err(|_| AuthError::InvalidContext)?;
        if d.profile
            .as_ref()
            .is_none_or(|p| p["body"]["profile"]["deterministic_program_id"] != "terminal_text_v1")
        {
            return Err(AuthError::InvalidContext);
        }
        let receipt = self
            .store
            .admit_distribution(d)
            .await
            .map_err(|_| AuthError::InvalidContext)?;
        self.transfer(client, receipt).await?;
        let claimed = self
            .store
            .isolated_claim_distribution(
                crate::ports::state_store::ClaimNextWorkRequest {
                    runtime_id: self.runtime.runtime_instance_id,
                    claimed_at: timestamp().map_err(|_| AuthError::Unavailable)?,
                    event_id: JournalEventId::generate(),
                },
                d,
            )
            .await
            .map_err(|_| AuthError::InvalidContext)?
            .ok_or(AuthError::InvalidContext)?;
        super::fixture::complete(
            &self.store,
            &self.config,
            claimed,
            #[cfg(target_os = "linux")]
            None,
        )
        .await
        .map_err(|e| {
            eprintln!("distribution_fixture_{e}");
            AuthError::InvalidContext
        })?;
        Ok(())
    }
    #[cfg(all(target_os = "linux", feature = "test-failpoints", debug_assertions))]
    pub async fn isolated_bounded_once(&self, client: &mut WorkerClient) -> Result<(), AuthError> {
        let mut items = client
            .poll(
                &self.boot_id,
                &self.config.power_operation_id,
                &self.config.profile_public_key,
            )
            .await
            .map_err(|e| {
                eprintln!("distribution_fixture_bounded_poll_{e:?}");
                e
            })?;
        for item in &mut items {
            item.compute = self.compute.clone();
        }
        let d = items
            .iter()
            .find(|d| d.message["type"] == "work_delivery")
            .ok_or_else(|| {
                eprintln!("distribution_fixture_bounded_delivery_absent");
                AuthError::Unavailable
            })?;
        d.verify_profile().map_err(|e| {
            eprintln!("distribution_fixture_bounded_profile_{e:?}");
            AuthError::InvalidContext
        })?;
        if d.profile
            .as_ref()
            .is_none_or(|p| p["body"]["profile"]["deterministic_program_id"] != "bounded_tool_v1")
        {
            return Err(AuthError::InvalidContext);
        }
        let receipt = self.store.admit_distribution(d).await.map_err(|e| {
            eprintln!("distribution_fixture_bounded_admit_{e:?}");
            AuthError::InvalidContext
        })?;
        self.transfer(client, receipt).await.map_err(|e| {
            eprintln!("distribution_fixture_bounded_transfer_{e:?}");
            e
        })?;
        let claimed = self
            .store
            .isolated_claim_distribution(
                crate::ports::state_store::ClaimNextWorkRequest {
                    runtime_id: self.runtime.runtime_instance_id,
                    claimed_at: timestamp().map_err(|_| AuthError::Unavailable)?,
                    event_id: JournalEventId::generate(),
                },
                d,
            )
            .await
            .map_err(|_| AuthError::InvalidContext)?
            .ok_or(AuthError::InvalidContext)?;
        let permit = self
            .store
            .distribution_compute_permit(d, claimed.work.work_id())
            .await
            .map_err(|e| {
                eprintln!("distribution_fixture_bounded_claim_{e:?}");
                AuthError::InvalidContext
            })?;
        super::fixture::complete(&self.store, &self.config, claimed, Some(permit))
            .await
            .map_err(|e| {
                eprintln!("distribution_fixture_{e}");
                AuthError::InvalidContext
            })?;
        Ok(())
    }
    #[cfg(all(feature = "test-failpoints", debug_assertions))]
    pub async fn isolated_crash_close(self) {
        self.guard.shutdown().await;
    }
    pub async fn shutdown(self) -> DistributionResult<()> {
        let at = timestamp()?;
        self.store
            .begin_runtime_stopping(BeginRuntimeStoppingRequest {
                event: RuntimeStoppingV1 {
                    runtime_instance_id: self.runtime.runtime_instance_id,
                    shutdown_requested_at: at,
                    shutdown_reason: RuntimeShutdownReason::GracefulShutdown,
                    grace_deadline: at,
                    active_work_count: 0,
                    active_task_count: 0,
                },
                event_id: JournalEventId::generate(),
                correlation_id: self.runtime.correlation_id,
            })
            .await
            .map_err(|_| DistributionError::Storage)?;
        self.store
            .finish_runtime_graceful(FinishRuntimeRequest {
                runtime_instance_id: self.runtime.runtime_instance_id,
                stopped_at: timestamp()?,
            })
            .await
            .map_err(|_| DistributionError::Storage)?;
        self.guard.shutdown().await;
        Ok(())
    }
    pub async fn run(self, mut client: WorkerClient) -> DistributionResult<()> {
        self.reconnect(&mut client).await.map_err(|_| invalid())?;
        #[cfg(unix)]
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| invalid())?;
        let mut delay = 1000u64;
        loop {
            let exchange = async {
                #[cfg(target_os = "linux")]
                if let Some(c) = &self.compute {
                    c.refresh(&mut client, &self.boot_id).await?;
                    c.running().map_err(|_| AuthError::Unavailable)?;
                }
                self.transfer_pending(&mut client).await?;
                self.poll_once(&mut client).await?;
                Ok::<_, AuthError>(())
            };
            let drain = async {
                loop {
                    #[cfg(target_os = "linux")]
                    if self.compute.as_ref().is_some_and(|c| c.should_drain()) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            };
            #[cfg(unix)]
            let result = tokio::select! { result=exchange=>result,_=drain=>break,_=termination.recv()=>break,result=tokio::signal::ctrl_c()=>{result.map_err(|_|invalid())?;break;} };
            #[cfg(not(unix))]
            let result = tokio::select! { result=exchange=>result,_=drain=>break,result=tokio::signal::ctrl_c()=>{result.map_err(|_|invalid())?;break;} };
            match result {
                Ok(()) => delay = 1000,
                Err(AuthError::SessionExpired) => {
                    self.reconnect(&mut client).await.map_err(|_| invalid())?;
                }
                Err(AuthError::Transport | AuthError::Unavailable) => delay = (delay * 2).min(5000),
                Err(_) => return Err(invalid()),
            }
            let jitter = crate::distribution::request_id()?.as_bytes()[0] as u64;
            #[cfg(unix)]
            tokio::select! {_=termination.recv()=>{break;},result=tokio::signal::ctrl_c()=>{result.map_err(|_|invalid())?;break;},_=tokio::time::sleep(Duration::from_millis(delay+jitter))=>{}}
            #[cfg(not(unix))]
            tokio::select! {result=tokio::signal::ctrl_c()=>{result.map_err(|_|invalid())?;break;},_=tokio::time::sleep(Duration::from_millis(delay+jitter))=>{}}
        }
        #[cfg(target_os = "linux")]
        if let Some(c) = &self.compute {
            if let Ok(cutoff) = c.cleanup_cutoff() {
                let _ = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(cutoff),
                    self.transfer_pending(&mut client),
                )
                .await;
            }
        }
        self.shutdown().await
    }
}
pub async fn run_protected() -> DistributionResult<()> {
    crate::guest_boundary::storage::harden_worker_process()
        .map_err(|_| DistributionError::Storage)?;
    let guest =
        crate::guest_boundary::storage::load_installed().map_err(|_| DistributionError::Storage)?;
    crate::guest_boundary::storage::verify_installed(&guest)
        .map_err(|_| DistributionError::Storage)?;
    // Trusted OS-root key/config descriptor checks precede all content/runtime composition.
    let uid = unsafe { nix::libc::geteuid() };
    let name = std::ffi::CString::new("craxii").map_err(|_| invalid())?;
    let user = unsafe { nix::libc::getpwnam(name.as_ptr()) };
    if user.is_null() {
        return Err(invalid());
    }
    let model_uid = unsafe { (*user).pw_uid };
    let key = KeyStore::open(uid, model_uid, false).map_err(|_| invalid())?;
    let config = key.distribution_config().map_err(|_| invalid())?;
    config.validate()?;
    let state = IdentityState::load(&key)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
    if state.customer_id != guest.text("customer_id")
        || state.logical_agent_id != guest.text("logical_agent_id")
        || state.installation_id != guest.text("installation_id")
        || state.generation != guest.text("generation")
    {
        return Err(DistributionError::Storage);
    }
    let client = WorkerClient::new(&config.origin, key, state).map_err(|_| invalid())?;
    DistributionRuntime::open(config).await?.run(client).await
}
