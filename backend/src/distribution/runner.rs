//! B2.5: the release build's agent loop for ordinary distribution work. One runner per worker; each claimed task
//! gets its own agent loop, built from the task's stored authorization (B2.1), its tool permit (B2.2, Linux), a
//! profile-free model binding (B2.3) and the VM config's limits (B2.4).
use crate::adapters::{
    artifacts::LocalArtifactStore, sqlite::OrdinaryAuthorization, sqlite::SqliteStateStore,
    system_clock::SystemClock,
};
use crate::application::{
    agent_loop::{AgentLoop, AgentLoopRuntimeContext},
    authority::{V0AuthorityConstraints, V0AuthorityEvaluator},
    context_assembler::{ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot},
    model_gateway::{ModelGateway, ModelGatewayLimits, NoopDraftSink},
    model_selection::{ModelSelectionPolicy, ModelTargetSnapshot},
    scheduler::{
        WorkCancellation, WorkRunner, WorkRunnerExit, WorkRunnerFuture, WorkRunnerStartError,
    },
    tool_execution_service::{ToolExecutionService, ToolRuntimeLimits},
    tool_registry::{ToolRegistry, ToolSemanticPolicy},
};
use crate::domain::model::RequiredModelCapabilities;
use crate::domain::*;
use crate::ports::{model_provider::*, state_store::*, workstation::*, workstation_preparation::*};
use crate::worker_auth::client::WorkerClient;
use std::sync::Arc;

/// No workstation: development builds off the VM have no tools.
/// What one task runs with: the compute authority for model calls, the workstation and its preparation, and
/// whether tools are enabled.
type Tooling = (
    Arc<dyn crate::adapters::model_proxy::ModelComputeAuthority>,
    Arc<dyn Workstation>,
    Arc<dyn WorkstationPreparation>,
    bool,
);

pub(crate) struct NoTools;
fn denied<T>() -> WorkstationFuture<'static, T> {
    Box::pin(async {
        Err(WorkstationError::new(
            WorkstationErrorKind::UnsupportedCapability,
        ))
    })
}
impl Workstation for NoTools {
    fn capabilities(&self, _: CapabilitiesRequest) -> WorkstationFuture<'_, CapabilitiesResult> {
        denied()
    }
    fn read_file(&self, _: FileReadRequest) -> WorkstationFuture<'_, FileReadResult> {
        denied()
    }
    fn execute(&self, _: ExecutionRequest) -> WorkstationFuture<'_, ExecutionResult> {
        denied()
    }
    fn inspect_execution(
        &self,
        _: ExecutionInspectionRequest,
    ) -> WorkstationFuture<'_, ExecutionInspection> {
        denied()
    }
    fn cancel_execution(
        &self,
        _: ExecutionCancellationRequest,
    ) -> WorkstationFuture<'_, CancellationResult> {
        denied()
    }
}
impl WorkstationPreparation for NoTools {
    fn prepare(
        &self,
        _: WorkstationPreparationRequest,
    ) -> WorkstationPreparationFuture<'_, WorkstationPreparationResult> {
        denied()
    }
}
/// A development build without the root compute authority: nothing to check model calls against.
struct Unmetered;
impl crate::adapters::model_proxy::ModelComputeAuthority for Unmetered {
    fn check(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        Ok(())
    }
}
struct SystemJitter;
impl FullJitterSource for SystemJitter {
    fn sample_inclusive(&mut self, upper_bound: u64) -> u64 {
        if upper_bound == 0 {
            return 0;
        }
        let mut bytes = [0_u8; 8];
        if getrandom::fill(&mut bytes).is_err() {
            return 0;
        }
        u64::from_le_bytes(bytes) % upper_bound.saturating_add(1)
    }
}

/// The scheduler's store for a worker running ordinary work: the claim rechecks each task's stored
/// authorization through `gate` (on a VM, against the root-accepted current grant); everything else delegates.
pub(crate) struct OrdinaryClaimStore {
    pub(crate) store: Arc<SqliteStateStore>,
    pub(crate) gate: Arc<dyn Fn(&OrdinaryAuthorization) -> bool + Send + Sync>,
}
impl SchedulerStateStore for OrdinaryClaimStore {
    fn claim_next_work(
        &self,
        request: ClaimNextWorkRequest,
    ) -> StateStoreFuture<'_, Option<ClaimedWork>> {
        Box::pin(async move { self.store.claim_next_ordinary(request, &*self.gate).await })
    }
    fn list_current_runtime_cancel_requested(
        &self,
        runtime_id: RuntimeInstanceId,
    ) -> StateStoreFuture<'_, Vec<CancelRequestedWork>> {
        self.store.list_current_runtime_cancel_requested(runtime_id)
    }
    fn finish_cancellation(
        &self,
        request: FinishCancellationRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.store.finish_cancellation(request)
    }
    fn interrupt_abnormal_runner(
        &self,
        request: InterruptOwnedWorkRequest,
    ) -> StateStoreFuture<'_, CommitReceipt> {
        self.store.interrupt_abnormal_runner(request)
    }
    fn request_owned_work_cancellation(
        &self,
        request: RequestOwnedCancellationRequest,
    ) -> StateStoreFuture<'_, Vec<WorkId>> {
        self.store.request_owned_work_cancellation(request)
    }
}

struct Inner {
    store: Arc<SqliteStateStore>,
    config: super::startup::DistributionConfig,
    environment: super::WorkerEnvironment,
    identity: (WorkstationIdentity, WorkspaceIdentity),
    client: Arc<tokio::sync::Mutex<WorkerClient>>,
    artifacts: Arc<LocalArtifactStore>,
    clock: Arc<SystemClock>,
    #[cfg(target_os = "linux")]
    compute: Option<Arc<crate::compute::worker::InstalledCompute>>,
}
/// Builds and runs one agent loop per claimed ordinary task.
pub(crate) struct OrdinaryRunner(Arc<Inner>);
impl OrdinaryRunner {
    pub(crate) fn new(
        store: Arc<SqliteStateStore>,
        config: super::startup::DistributionConfig,
        environment: super::WorkerEnvironment,
        identity: (WorkstationIdentity, WorkspaceIdentity),
        client: Arc<tokio::sync::Mutex<WorkerClient>>,
        #[cfg(target_os = "linux")] compute: Option<Arc<crate::compute::worker::InstalledCompute>>,
    ) -> super::DistributionResult<Self> {
        if config.model_proxy.is_none() {
            return Err(super::invalid());
        }
        #[cfg(target_os = "linux")]
        let metered = compute.is_some();
        #[cfg(not(target_os = "linux"))]
        let metered = false;
        // Production always runs under the root compute authority.
        if environment == super::WorkerEnvironment::Production && !metered {
            return Err(super::invalid());
        }
        let artifacts = Arc::new(
            LocalArtifactStore::initialize(std::path::Path::new(&config.artifact_root))
                .map_err(|_| super::DistributionError::Storage)?,
        );
        Ok(Self(Arc::new(Inner {
            store,
            config,
            environment,
            identity,
            client,
            artifacts,
            clock: Arc::new(SystemClock::new()),
            #[cfg(target_os = "linux")]
            compute,
        })))
    }
}
impl Inner {
    async fn agent_loop(&self, claimed: &ClaimedWork) -> Result<AgentLoop, &'static str> {
        let local = claimed.work.work_id();
        let (receipt, authorization) = self.store.ordinary_task(local).await.map_err(|_| "task")?;
        let settings = self.config.model_proxy.clone().ok_or("model_proxy")?;
        let limits = self.config.agent_limits();
        let loop_limits = limits.loop_limits();
        let target = settings.ordinary_target().map_err(|_| "target")?;
        let binding = crate::adapters::model_proxy::ProxyBinding::ordinary(
            self.environment,
            receipt,
            &authorization,
            settings,
        )
        .map_err(|_| "binding")?;
        let (workstation_identity, workspace_identity) = &self.identity;
        #[cfg(target_os = "linux")]
        let (compute_authority, workstation, preparation, tools_enabled): Tooling = if let Some(c) =
            &self.compute
        {
            use crate::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
            use crate::guest_boundary::storage;
            let deadline = std::time::Instant::now() + loop_limits.work_duration;
            let permit = self
                .store
                .ordinary_compute_permit(c.clone(), local, deadline, limits.tool_call_timeout_ms())
                .await
                .map_err(|_| "permit")?;
            let w = Arc::new(
                LocalWorkstation::new_installed(
                    workstation_identity,
                    workspace_identity,
                    LocalWorkstationOptions {
                        default_shell: LogicalPathReference::absolute("/bin/bash")
                            .map_err(|_| "shell")?,
                        configured_workspace_root: storage::WORKSPACE.into(),
                        read_hard_limit: HARD_FILE_READ_MAX_BYTES,
                        artifact_store: self.artifacts.clone(),
                        administrative_enabled: false,
                        user_switch_launcher: Some(
                            "/opt/craxii/current/craxii-workstation-launcher".into(),
                        ),
                        credential_free_direct_execution: false,
                        delegated_cgroup_root: Some(storage::TOOLS_CGROUP.into()),
                        clock: self.clock.clone(),
                    },
                )
                .and_then(|w| w.with_compute_permit(permit))
                .map_err(|_| "workstation")?,
            );
            (c.clone(), w.clone(), w, true)
        } else {
            (
                Arc::new(Unmetered),
                Arc::new(NoTools),
                Arc::new(NoTools),
                false,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let (compute_authority, workstation, preparation, tools_enabled): Tooling = (
            Arc::new(Unmetered),
            Arc::new(NoTools),
            Arc::new(NoTools),
            false,
        );
        let provider: Arc<dyn ModelProvider> = Arc::new(
            crate::adapters::model_proxy::CentralModelProvider::new(
                self.client.clone(),
                binding,
                target.clone(),
                self.clock.clone(),
                compute_authority,
            )
            .map_err(|_| "provider")?,
        );
        let tool_ms = limits.tool_call_timeout_ms();
        let policy = ToolSemanticPolicy {
            read_file_default_bytes: 4096,
            read_file_max_bytes: 65536,
            run_shell_command_max_bytes: 65536,
            run_shell_default_timeout_ms: tool_ms.min(60_000),
            run_shell_max_timeout_ms: tool_ms,
        };
        let registry = Arc::new(ToolRegistry::v0(policy).map_err(|_| "registry")?);
        let store = self.store.clone();
        let tools = Arc::new(
            ToolExecutionService::new(
                registry.clone(),
                Arc::new(V0AuthorityEvaluator),
                store.clone(),
                workstation,
                preparation,
                self.artifacts.clone(),
                self.clock.clone(),
                ToolRuntimeLimits {
                    read_file_default_bytes: 4096,
                    read_file_max_bytes: 65536,
                    run_shell_command_max_bytes: 65536,
                    run_shell_default_timeout_ms: tool_ms.min(60_000),
                    run_shell_max_timeout_ms: tool_ms,
                    stdout_capture_bytes: 32768,
                    stderr_capture_bytes: 32768,
                    inline_model_result_bytes: 65536,
                    per_stream_projection_bytes: 32768,
                },
            )
            .map_err(|_| "tools")?,
        );
        let assembler = Arc::new(ContextAssembler::new(
            store.clone(),
            Some(self.artifacts.clone()),
            Arc::new(crate::adapters::openai::OpenAiConservativeEstimator::new(
                target.estimator().clone(),
            )),
            registry,
            VersionedInstructionSnapshot::v0(),
            self.clock.clone(),
        ));
        let gateway = Arc::new(
            ModelGateway::new(
                store.clone(),
                self.artifacts.clone(),
                provider,
                Arc::new(NoopDraftSink),
                self.clock.clone(),
                Box::new(SystemJitter),
                ModelGatewayLimits {
                    maximum_attempts_per_work: loop_limits.provider_attempts_per_work,
                    ..ModelGatewayLimits::default()
                },
            )
            .map_err(|_| "gateway")?,
        );
        let selection = Arc::new(ModelSelectionPolicy::new(Arc::new(
            ModelTargetSnapshot::try_new(
                target.reference().model_target_id().clone(),
                vec![target.clone()],
            )
            .map_err(|_| "selection")?,
        )));
        AgentLoop::new(
            selection,
            assembler,
            ContextAssemblyVersions::v0(),
            gateway,
            tools,
            store,
            self.clock.clone(),
            RequiredModelCapabilities {
                text_input: true,
                text_output: true,
                custom_tool_calling: true,
                streaming: true,
                ordered_output_items: true,
                structured_output: false,
                reasoning_continuation: false,
                required_output_tokens: target.requested_output_tokens(),
            },
            AgentLoopRuntimeContext {
                workstation: workstation_identity.clone(),
                workspace: workspace_identity.clone(),
                authority_constraints: V0AuthorityConstraints {
                    tool_allowed: tools_enabled,
                    administrative_allowed: false,
                },
            },
            loop_limits,
        )
        .map_err(|_| "agent_loop")
    }
}
impl WorkRunner for OrdinaryRunner {
    fn start(
        &self,
        work: ClaimedWork,
        cancellation: WorkCancellation,
    ) -> Result<WorkRunnerFuture, WorkRunnerStartError> {
        let inner = self.0.clone();
        Ok(Box::pin(async move {
            let agent = match inner.agent_loop(&work).await {
                Ok(a) => a,
                Err(stage) => {
                    tracing::warn!(target: "craxii::distribution", event = "runner_unavailable", stage);
                    return WorkRunnerExit::Abnormal;
                }
            };
            match agent.start(work, cancellation) {
                Ok(task) => task.await,
                Err(_) => WorkRunnerExit::Abnormal,
            }
        }))
    }
}
