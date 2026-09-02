use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Display, Formatter};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::adapters::artifacts::LocalArtifactStore;
use crate::adapters::http::{ConnectionRegistry, HttpState, ServerHandle};
use crate::adapters::local_workstation::{
    LocalWorkstation, LocalWorkstationOptions, observe_execution_support,
};
use crate::adapters::openai::{OpenAiConservativeEstimator, OpenAiProvider};
use crate::adapters::runtime_observation::SystemRuntimeProcessObserver;
use crate::adapters::sqlite::{SqliteFailureKind, SqliteRuntimeGuard, SqliteStateStore};
use crate::adapters::system_clock::SystemClock;
use crate::adapters::telemetry::{Telemetry, TelemetryError};
use crate::application::ApplicationShell;
use crate::application::agent_loop::{AgentLoop, AgentLoopLimits, AgentLoopRuntimeContext};
use crate::application::authority::{
    AuthorityEvaluator, V0AuthorityConstraints, V0AuthorityEvaluator,
};
use crate::application::context_assembler::{
    ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot,
};
use crate::application::event_delivery::LiveEventBroker;
use crate::application::model_gateway::{DraftSink, ModelGateway, ModelGatewayLimits};
use crate::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
use crate::application::runtime::{
    HeartbeatTask, RuntimeControlError, ShutdownController, bootstrap_runtime,
};
use crate::application::tool_execution_service::{ToolExecutionService, ToolRuntimeLimits};
use crate::application::tool_registry::{ToolRegistry, ToolSemanticPolicy};
use crate::application::transport::{CursorBroadcaster, MutationAdmission};
use crate::bootstrap::config;
use crate::bootstrap::credential::{CredentialLoadErrorKind, load_credentials};
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{BuildMetadata, ProcessMetadata};
use crate::domain::model::RequiredModelCapabilities;
use crate::domain::{
    ConversationId, CorrelationId, CraxiiId, GitRevision, JournalEventId, PackageVersion,
    RuntimeInstanceId, RuntimeStartEvidence, RuntimeStartEvidenceInput, SchemaVersion,
    UtcTimestamp, WorkspaceId, WorkstationGeneration, WorkstationId,
};
use crate::ports::artifact_store::{ArtifactOrphanReport, ArtifactStore, ArtifactStoreErrorKind};
use crate::ports::clock::Clock;
use crate::ports::context_source_store::ContextSourceStore;
use crate::ports::model_provider::{FullJitterSource, ModelProvider, TokenEstimator};
use crate::ports::runtime_observation::RuntimeProcessObserver;
use crate::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, LoadOrBootstrapIdentityRequest, ModelStateStore,
    RuntimeStateStore, StateStoreErrorKind, ToolStateStore, V0IdentityReference,
};
use crate::ports::workstation::Workstation;
use crate::ports::workstation_preparation::WorkstationPreparation;

pub async fn run_from_env() -> Result<RunningBootstrap, StartupError> {
    let arguments: Vec<_> = std::env::args_os().collect();

    #[cfg(all(feature = "test-failpoints", unix))]
    if arguments
        .get(1)
        .is_some_and(|argument| argument == OsStr::new(crate::test_failpoints::CONTROL_ARGUMENT))
    {
        if arguments.len() != 2 {
            return Err(StartupError::TestControl);
        }
        crate::test_failpoints::run_controlled_startup().map_err(|_| StartupError::TestControl)?;
        return Err(StartupError::TestControl);
    }

    run(arguments).await
}

pub async fn run(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<RunningBootstrap, StartupError> {
    let cli = Cli::parse(arguments)?;
    let config = config::load(&cli.config_path).map_err(|_| StartupError::Configuration)?;
    let model_targets = Arc::new(
        ModelTargetSnapshot::from_validated_config(config.models())
            .map_err(|_| StartupError::Configuration)?,
    );
    let model_selection_policy = Arc::new(ModelSelectionPolicy::new(Arc::clone(&model_targets)));
    let credentials = match load_credentials(
        config.credentials().source(),
        config
            .models()
            .targets()
            .iter()
            .filter(|target| target.enabled())
            .map(|target| target.credential()),
    ) {
        Ok(credentials) => Some(credentials),
        Err(error) if error.kind() == CredentialLoadErrorKind::Missing => None,
        Err(_) => return Err(StartupError::ProviderCredential),
    };
    let clock = Arc::new(SystemClock::new());
    let model_provider: Option<Arc<dyn ModelProvider>> = credentials
        .map(|credentials| {
            let provider_clock: Arc<dyn Clock> = clock.clone();
            OpenAiProvider::try_new(credentials, provider_clock)
                .map(|provider| Arc::new(provider) as Arc<dyn ModelProvider>)
        })
        .transpose()
        .map_err(|_| StartupError::ProviderComposition)?;
    if let Some(provider) = model_provider.as_ref() {
        let default_target = model_targets
            .target(model_targets.default_target())
            .ok_or(StartupError::ProviderComposition)?;
        for target in model_targets
            .targets()
            .iter()
            .filter(|target| target.enabled())
        {
            provider
                .capabilities(target)
                .map_err(|_| StartupError::ProviderComposition)?;
            if target.reference().capabilities().structured_output()
                || target.reference().capabilities().reasoning_continuation()
                || target.provider_native_options().reasoning_continuation()
                || target.estimator() != default_target.estimator()
            {
                return Err(StartupError::ProviderComposition);
            }
        }
    }
    let build = BuildMetadata::embedded().map_err(|_| StartupError::BuildMetadata)?;
    let process = ProcessMetadata::capture(build, config.fingerprint(), clock.as_ref())
        .map_err(|_| StartupError::Clock)?;
    let health = Health::new();
    let telemetry =
        Telemetry::initialize_global(config.tracing()).map_err(StartupError::Telemetry)?;
    // Binding precedes every RuntimeInstance write. The socket is not served until recovery is
    // coherent, so a bind failure cannot create runtime.started or accept application traffic.
    let listener = tokio::net::TcpListener::bind(config.server().bind_address())
        .await
        .map_err(|_| StartupError::ServerBind)?;
    let sqlite_runtime = SqliteRuntimeGuard::start(
        config.paths().state_root(),
        config.sqlite().pool_connections(),
    )
    .await
    .map_err(StartupError::from_sqlite)?;
    let artifact_store = Arc::new(
        LocalArtifactStore::initialize(config.paths().artifact_root())
            .map_err(StartupError::from_artifact_initialization)?,
    );
    let observation = bootstrap_observation(&config)?;
    let created_at =
        UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| StartupError::Clock)?)
            .map_err(|_| StartupError::Clock)?;
    let state_store = SqliteStateStore::new(sqlite_runtime.runtime().clone());
    let bootstrap = state_store
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
            created_at,
            observation,
        })
        .await
        .map_err(StartupError::from_state_store)?;
    let _consistency = state_store
        .verify_application_consistency()
        .await
        .map_err(StartupError::from_state_store)?;
    let referenced_artifacts = state_store
        .load_referenced_artifacts()
        .await
        .map_err(StartupError::from_sqlite)?;
    let mut referenced_keys = BTreeSet::new();
    for artifact in &referenced_artifacts {
        artifact_store
            .verify(artifact)
            .map_err(StartupError::from_artifact_integrity)?;
        referenced_keys.insert(artifact.storage_key().clone());
    }
    let orphan_report = artifact_store
        .scan_orphans(&referenced_keys, created_at)
        .map_err(StartupError::from_artifact_integrity)?;
    let snapshot = state_store
        .load_bootstrap_snapshot()
        .await
        .map_err(StartupError::from_state_store)?;
    if snapshot.identity != bootstrap.identity {
        return Err(StartupError::DatabaseIntegrity);
    }
    let default_shell = crate::domain::LogicalPathReference::absolute(
        config
            .shell()
            .executable()
            .to_str()
            .ok_or(StartupError::Configuration)?,
    )
    .map_err(|_| StartupError::Configuration)?;
    let workstation_clock: Arc<dyn Clock> = clock.clone();
    let workstation_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
    let local_workstation = Arc::new(
        LocalWorkstation::new(
            &snapshot.workstation,
            &snapshot.workspace,
            LocalWorkstationOptions {
                default_shell,
                configured_workspace_root: config.paths().primary_workspace_root().to_owned(),
                read_hard_limit: config.limits().tools().read_file_max_bytes(),
                artifact_store: workstation_artifacts,
                administrative_enabled: config.shell().administrative_enabled(),
                delegated_cgroup_root: config.shell().delegated_cgroup_root().map(Path::to_owned),
                clock: workstation_clock,
            },
        )
        .map_err(|_| StartupError::WorkstationLifecycle)?,
    );
    if local_workstation.capabilities_snapshot() != &snapshot.workstation_capabilities {
        return Err(StartupError::DatabaseIntegrity);
    }
    let workstation: Arc<dyn Workstation> = local_workstation.clone();
    let observation = SystemRuntimeProcessObserver
        .observe()
        .map_err(|_| StartupError::RuntimeLifecycle)?;
    let runtime_started_at =
        UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| StartupError::Clock)?)
            .map_err(|_| StartupError::Clock)?;
    let runtime_evidence = RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
        runtime_instance_id: RuntimeInstanceId::generate(),
        craxii_id: snapshot.identity.craxii_id,
        workstation_id: snapshot.identity.workstation_id,
        workstation_generation: snapshot.workstation.generation(),
        linux_boot_id: Some(observation.linux_boot_id),
        diagnostic_pid: Some(observation.process_id),
        package_version: PackageVersion::try_new(process.build().package_version())
            .map_err(|_| StartupError::BuildMetadata)?,
        git_revision: GitRevision::try_new(process.build().git_revision())
            .map_err(|_| StartupError::BuildMetadata)?,
        schema_version: SchemaVersion::try_new(4).map_err(|_| StartupError::BuildMetadata)?,
        started_at: runtime_started_at,
    });
    let state_store = Arc::new(state_store);
    let runtime = bootstrap_runtime(
        state_store.as_ref(),
        runtime_evidence,
        orphan_report.orphans.len() as u64,
        clock.as_ref(),
    )
    .await
    .map_err(StartupError::from_runtime)?;
    if let Err(error) = state_store.verify_application_consistency().await {
        if let Ok(wall_time) = clock.utc_now()
            && let Ok(stopped_at) = UtcTimestamp::from_offset_datetime(wall_time)
        {
            let _ = state_store
                .mark_runtime_startup_failure(crate::ports::state_store::FinishRuntimeRequest {
                    runtime_instance_id: runtime.runtime_instance_id,
                    stopped_at,
                })
                .await;
        }
        return Err(StartupError::from_state_store(error));
    }
    if let Err(error) = telemetry.emit_startup_evidence(&process, &health) {
        if let Ok(wall_time) = clock.utc_now()
            && let Ok(stopped_at) = UtcTimestamp::from_offset_datetime(wall_time)
        {
            let _ = state_store
                .mark_runtime_startup_failure(crate::ports::state_store::FinishRuntimeRequest {
                    runtime_instance_id: runtime.runtime_instance_id,
                    stopped_at,
                })
                .await;
        }
        return Err(StartupError::Telemetry(error));
    }
    let (fatal, fatal_receiver) = tokio::sync::watch::channel(false);
    let heartbeat = HeartbeatTask::start(
        Arc::clone(&state_store),
        Arc::clone(&clock),
        health.clone(),
        runtime.runtime_instance_id,
        fatal.clone(),
    );
    let shutdown = Arc::new(ShutdownController::new(
        Arc::clone(&state_store),
        Arc::clone(&clock),
        health.clone(),
        runtime.runtime_instance_id,
        runtime.correlation_id,
        config.shutdown().grace_period_ms(),
        heartbeat,
    ));
    let mutation_admission = MutationAdmission::new();
    let cursors = CursorBroadcaster::new();
    let live_events = Arc::new(LiveEventBroker::new());
    let connections = ConnectionRegistry::default();
    let (ws_shutdown, _) = tokio::sync::watch::channel(false);
    let controlled_shutdown: Arc<dyn crate::application::runtime::ControlledShutdown> =
        shutdown.clone();
    let tool_registry = Arc::new(
        ToolRegistry::v0(ToolSemanticPolicy {
            read_file_default_bytes: config.limits().tools().read_file_default_bytes(),
            read_file_max_bytes: config.limits().tools().read_file_max_bytes(),
            run_shell_command_max_bytes: config.limits().tools().run_shell_command_max_bytes(),
            run_shell_default_timeout_ms: config.limits().tools().run_shell_default_timeout_ms(),
            run_shell_max_timeout_ms: config.limits().tools().run_shell_max_timeout_ms(),
        })
        .map_err(|_| StartupError::Configuration)?,
    );
    let authority: Arc<dyn AuthorityEvaluator> = Arc::new(V0AuthorityEvaluator);
    let tool_state_store: Arc<dyn ToolStateStore> = state_store.clone();
    let tool_workstation: Arc<dyn Workstation> = local_workstation.clone();
    let workstation_preparation: Arc<dyn WorkstationPreparation> = local_workstation.clone();
    let tool_artifact_store: Arc<dyn ArtifactStore> = artifact_store.clone();
    let tool_clock: Arc<dyn Clock> = clock.clone();
    let tool_execution_service = Arc::new(
        ToolExecutionService::new(
            Arc::clone(&tool_registry),
            authority,
            tool_state_store,
            tool_workstation,
            workstation_preparation,
            tool_artifact_store,
            tool_clock,
            ToolRuntimeLimits {
                read_file_default_bytes: config.limits().tools().read_file_default_bytes(),
                read_file_max_bytes: config.limits().tools().read_file_max_bytes(),
                run_shell_command_max_bytes: config.limits().tools().run_shell_command_max_bytes(),
                run_shell_default_timeout_ms: config
                    .limits()
                    .tools()
                    .run_shell_default_timeout_ms(),
                run_shell_max_timeout_ms: config.limits().tools().run_shell_max_timeout_ms(),
                stdout_capture_bytes: config.limits().tools().stdout_capture_bytes(),
                stderr_capture_bytes: config.limits().tools().stderr_capture_bytes(),
                inline_model_result_bytes: config.limits().tools().inline_model_result_bytes(),
                per_stream_projection_bytes: config.limits().tools().per_stream_projection_bytes(),
            },
        )
        .map_err(|_| StartupError::WorkstationLifecycle)?,
    );
    let scheduler_notifier = if let Some(model_provider) = model_provider {
        let default_target = model_targets
            .target(model_targets.default_target())
            .ok_or(StartupError::ProviderComposition)?;
        let estimator: Arc<dyn TokenEstimator> = Arc::new(OpenAiConservativeEstimator::new(
            default_target.estimator().clone(),
        ));
        let source_store: Arc<dyn ContextSourceStore> = state_store.clone();
        let context_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let context_clock: Arc<dyn Clock> = clock.clone();
        let context_assembler = Arc::new(ContextAssembler::new(
            source_store,
            Some(context_artifacts),
            estimator,
            Arc::clone(&tool_registry),
            VersionedInstructionSnapshot::v0(),
            context_clock,
        ));
        let model_store: Arc<dyn ModelStateStore> = state_store.clone();
        let model_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let model_clock: Arc<dyn Clock> = clock.clone();
        let model_gateway = Arc::new(
            ModelGateway::new(
                model_store,
                model_artifacts,
                model_provider,
                live_events.clone() as Arc<dyn DraftSink>,
                model_clock,
                Box::new(SystemJitter),
                ModelGatewayLimits::default(),
            )
            .map_err(|_| StartupError::ProviderComposition)?,
        );
        let loop_store: Arc<dyn crate::application::agent_loop::AgentLoopStateStore> =
            state_store.clone();
        let loop_clock: Arc<dyn Clock> = clock.clone();
        let runner = Arc::new(
            AgentLoop::new(
                Arc::clone(&model_selection_policy),
                context_assembler,
                ContextAssemblyVersions::v0(),
                model_gateway,
                Arc::clone(&tool_execution_service),
                loop_store,
                loop_clock,
                RequiredModelCapabilities {
                    text_input: true,
                    text_output: true,
                    custom_tool_calling: true,
                    streaming: true,
                    ordered_output_items: true,
                    structured_output: false,
                    reasoning_continuation: false,
                    required_output_tokens: default_target.requested_output_tokens(),
                },
                AgentLoopRuntimeContext {
                    workstation: snapshot.workstation.clone(),
                    workspace: snapshot.workspace.clone(),
                    authority_constraints: V0AuthorityConstraints::default(),
                },
                AgentLoopLimits::default(),
            )
            .map_err(|_| StartupError::ProviderComposition)?,
        );
        let scheduler = crate::application::scheduler::start_scheduler(
            Arc::clone(&state_store),
            runner,
            Arc::clone(&clock),
            health.clone(),
            fatal.clone(),
            crate::application::scheduler::SchedulerStart {
                runtime_instance_id: runtime.runtime_instance_id,
                conversation_id: snapshot.identity.conversation_id,
                readiness: crate::application::scheduler::SchedulerReadiness::ReadyAfterInitialScan,
            },
        )
        .map_err(|_| StartupError::RuntimeLifecycle)?;
        let notifier = scheduler.notifier();
        shutdown
            .install_scheduler(scheduler)
            .await
            .map_err(|_| StartupError::RuntimeLifecycle)?;
        Some(notifier)
    } else {
        None
    };
    let http_state = HttpState::new(
        Arc::clone(&state_store),
        Arc::clone(&clock),
        health.clone(),
        mutation_admission.clone(),
        cursors,
        Arc::clone(&live_events),
        fatal,
        ws_shutdown,
        connections,
        allowed_hosts(&config)?,
        Some(controlled_shutdown),
        scheduler_notifier,
    );
    let server = ServerHandle::start(listener, http_state);
    Ok(RunningBootstrap {
        application: ApplicationShell::new(process, health, snapshot),
        sqlite_runtime,
        artifact_store,
        orphan_report,
        runtime_instance_id: runtime.runtime_instance_id,
        workstation,
        local_workstation,
        tool_execution_service,
        model_selection_policy,
        shutdown,
        mutation_admission,
        live_events,
        server,
        fatal_receiver,
    })
}

fn allowed_hosts(config: &config::ValidatedConfig) -> Result<Vec<String>, StartupError> {
    let mut hosts = vec![config.server().bind_address().to_string()];
    let public = url::Url::parse(config.server().public_base_url().as_str())
        .map_err(|_| StartupError::Configuration)?;
    let host = public.host_str().ok_or(StartupError::Configuration)?;
    let authority = match public.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    if !hosts.contains(&authority) {
        hosts.push(authority);
    }
    Ok(hosts)
}

fn bootstrap_observation(
    config: &config::ValidatedConfig,
) -> Result<BootstrapObservation, StartupError> {
    let configured_workspace_root = config
        .paths()
        .primary_workspace_root()
        .to_str()
        .ok_or(StartupError::Configuration)?
        .to_owned();
    let workspace_root = std::fs::canonicalize(config.paths().primary_workspace_root())
        .map_err(|_| StartupError::WorkstationLifecycle)?
        .to_str()
        .ok_or(StartupError::Configuration)?
        .to_owned();
    let default_shell = config
        .shell()
        .executable()
        .to_str()
        .ok_or(StartupError::Configuration)?
        .to_owned();
    let generation = i64::try_from(config.workstation().initial_generation())
        .map_err(|_| StartupError::Configuration)?;
    let execution = observe_execution_support(
        config.shell().executable(),
        config.shell().administrative_enabled(),
        config.shell().delegated_cgroup_root(),
    );
    Ok(BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(generation)
            .map_err(|_| StartupError::Configuration)?,
        architecture: std::env::consts::ARCH.to_owned(),
        os_release: std::env::consts::OS.to_owned(),
        default_shell,
        workspace_logical_name: config
            .workstation()
            .primary_workspace_logical_name()
            .to_owned(),
        workspace_logical_root: configured_workspace_root,
        workspace_resolved_root: workspace_root,
        execution_capabilities: crate::ports::state_store::ExecutionCapabilityObservation {
            foreground_execute: execution.foreground,
            privilege_administrative: execution.administrative,
            process_group_cleanup: execution.process_group,
            cgroup_cleanup: execution.cgroup,
        },
    })
}

#[derive(Clone, Copy, Debug)]
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

/// Successful Stage 7 bootstrap ownership.
///
/// This guard keeps the database pool and process lock alive without making the application layer
/// depend on an outward adapter. Its application remains live but deliberately unready.
pub struct RunningBootstrap {
    application: ApplicationShell,
    sqlite_runtime: SqliteRuntimeGuard,
    artifact_store: Arc<LocalArtifactStore>,
    orphan_report: ArtifactOrphanReport,
    runtime_instance_id: RuntimeInstanceId,
    workstation: Arc<dyn Workstation>,
    local_workstation: Arc<LocalWorkstation>,
    tool_execution_service: Arc<ToolExecutionService>,
    model_selection_policy: Arc<ModelSelectionPolicy>,
    shutdown: Arc<ShutdownController<SqliteStateStore, SystemClock>>,
    mutation_admission: MutationAdmission,
    live_events: Arc<LiveEventBroker>,
    server: ServerHandle,
    fatal_receiver: tokio::sync::watch::Receiver<bool>,
}

impl RunningBootstrap {
    #[must_use]
    pub const fn application(&self) -> &ApplicationShell {
        &self.application
    }

    #[must_use]
    pub const fn sqlite_runtime(&self) -> &SqliteRuntimeGuard {
        &self.sqlite_runtime
    }

    #[must_use]
    pub fn artifact_store(&self) -> &LocalArtifactStore {
        self.artifact_store.as_ref()
    }

    #[must_use]
    pub const fn artifact_orphan_report(&self) -> &ArtifactOrphanReport {
        &self.orphan_report
    }

    #[must_use]
    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }

    #[must_use]
    pub fn workstation(&self) -> &Arc<dyn Workstation> {
        &self.workstation
    }

    #[must_use]
    pub fn tool_execution_service(&self) -> &Arc<ToolExecutionService> {
        &self.tool_execution_service
    }

    #[must_use]
    pub fn model_selection_policy(&self) -> &ModelSelectionPolicy {
        self.model_selection_policy.as_ref()
    }

    #[must_use]
    pub fn model_target_snapshot(&self) -> &ModelTargetSnapshot {
        self.model_selection_policy.snapshot().as_ref()
    }

    pub async fn wait_for_shutdown_request(&mut self) -> Result<(), StartupError> {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(|_| StartupError::RuntimeLifecycle)?;
            tokio::select! {
                result = tokio::signal::ctrl_c() => result.map_err(|_| StartupError::RuntimeLifecycle),
                _ = terminate.recv() => Ok(()),
                result = self.fatal_receiver.changed() => {
                    result.map_err(|_| StartupError::RuntimeLifecycle)
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::select! {
                result = tokio::signal::ctrl_c() => result.map_err(|_| StartupError::RuntimeLifecycle),
                result = self.fatal_receiver.changed() => {
                    result.map_err(|_| StartupError::RuntimeLifecycle)
                }
            }
        }
    }

    pub async fn shutdown(self) -> Result<(), StartupError> {
        let mut runtime_cleanup_failed = false;
        let deadline = self.shutdown.latch_shutdown_request();
        match self.application.health().snapshot().state() {
            crate::bootstrap::health::HealthState::LiveUnready
            | crate::bootstrap::health::HealthState::Ready => self
                .application
                .health()
                .mark_draining()
                .unwrap_or_else(|_| runtime_cleanup_failed = true),
            crate::bootstrap::health::HealthState::Draining
            | crate::bootstrap::health::HealthState::Fatal => {}
        }
        self.server.stop_accepting();
        self.mutation_admission.close_and_wait().await;
        if self.shutdown.request().await.is_err() {
            runtime_cleanup_failed = true;
        }
        self.local_workstation.begin_execution_shutdown(deadline);
        if self
            .local_workstation
            .shutdown_executions_before(deadline)
            .await
            .is_err()
        {
            runtime_cleanup_failed = true;
        }
        self.live_events.close_admission();
        self.server.close_websockets();
        if self.shutdown.finish().await.is_err() {
            runtime_cleanup_failed = true;
        }
        let server_result = self.server.join_before(deadline).await;
        self.sqlite_runtime.shutdown().await;
        if let Err(error) = server_result {
            Err(StartupError::ServerLifecycle(error))
        } else if runtime_cleanup_failed {
            Err(StartupError::RuntimeLifecycle)
        } else {
            Ok(())
        }
    }
}

pub fn write_fatal_diagnostic(writer: &mut impl Write, error: &StartupError) -> io::Result<()> {
    writeln!(writer, "craxii fatal: {}", error.code())
}

struct Cli {
    config_path: PathBuf,
}

impl Cli {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, StartupError> {
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        let option = arguments.next().ok_or(StartupError::Cli)?;
        if option != OsStr::new("--config") {
            return Err(StartupError::Cli);
        }
        let value = arguments.next().ok_or(StartupError::Cli)?;
        if value.is_empty() || arguments.next().is_some() {
            return Err(StartupError::Cli);
        }
        Ok(Self {
            config_path: PathBuf::from(value),
        })
    }
}

pub enum StartupError {
    Cli,
    Configuration,
    BuildMetadata,
    Clock,
    DatabaseLifecycle,
    IncompatibleSchema,
    StateRootAlreadyOwned,
    DatabaseIntegrity,
    RuntimeLifecycle,
    WorkstationLifecycle,
    ProviderCredential,
    ProviderComposition,
    Telemetry(TelemetryError),
    ServerBind,
    ServerLifecycle(crate::adapters::http::ServerError),
    #[cfg(all(feature = "test-failpoints", unix))]
    TestControl,
}

impl StartupError {
    const fn from_artifact_initialization(
        _error: crate::ports::artifact_store::ArtifactStoreError,
    ) -> Self {
        Self::DatabaseLifecycle
    }

    const fn from_artifact_integrity(
        error: crate::ports::artifact_store::ArtifactStoreError,
    ) -> Self {
        match error.kind() {
            ArtifactStoreErrorKind::Integrity | ArtifactStoreErrorKind::Collision => {
                Self::DatabaseIntegrity
            }
            ArtifactStoreErrorKind::InvalidRequest
            | ArtifactStoreErrorKind::UnsafeRoot
            | ArtifactStoreErrorKind::UnsupportedFilesystem
            | ArtifactStoreErrorKind::Storage => Self::DatabaseLifecycle,
        }
    }

    const fn from_sqlite(error: crate::adapters::sqlite::SqliteAdapterError) -> Self {
        match error.kind() {
            SqliteFailureKind::AlreadyOwned => Self::StateRootAlreadyOwned,
            SqliteFailureKind::NewerSchema => Self::IncompatibleSchema,
            SqliteFailureKind::Corrupt | SqliteFailureKind::InconsistentSchema => {
                Self::DatabaseIntegrity
            }
            _ => Self::DatabaseLifecycle,
        }
    }

    const fn from_state_store(error: crate::ports::state_store::StateStoreError) -> Self {
        match error.kind() {
            StateStoreErrorKind::InternalInvariant
            | StateStoreErrorKind::StateConflict
            | StateStoreErrorKind::IdempotencyConflict
            | StateStoreErrorKind::TargetNotFound => Self::DatabaseIntegrity,
            StateStoreErrorKind::Storage => Self::DatabaseLifecycle,
        }
    }

    const fn from_runtime(_error: RuntimeControlError) -> Self {
        Self::RuntimeLifecycle
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::Cli => "invalid_cli",
            Self::Configuration => "invalid_configuration",
            Self::BuildMetadata => "invalid_build_metadata",
            Self::Clock => "clock_failure",
            Self::DatabaseLifecycle => "database_lifecycle_failure",
            Self::IncompatibleSchema => "incompatible_database_schema",
            Self::StateRootAlreadyOwned => "state_root_already_owned",
            Self::DatabaseIntegrity => "database_integrity_failure",
            Self::RuntimeLifecycle => "runtime_lifecycle_failure",
            Self::WorkstationLifecycle => "workstation_lifecycle_failure",
            Self::ProviderCredential => "provider_credential_unavailable",
            Self::ProviderComposition => "provider_composition_failure",
            Self::Telemetry(TelemetryError::GlobalSubscriberConflict) => {
                "telemetry_subscriber_conflict"
            }
            Self::Telemetry(TelemetryError::SinkFailure) => "telemetry_sink_failure",
            Self::ServerBind => "server_bind_failure",
            Self::ServerLifecycle(_) => "server_lifecycle_failure",
            #[cfg(all(feature = "test-failpoints", unix))]
            Self::TestControl => "invalid_test_control",
        }
    }
}

impl Display for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Debug for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl PartialEq for StartupError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ServerLifecycle(left), Self::ServerLifecycle(right)) => {
                left.kind() == right.kind()
            }
            _ => self.code() == other.code(),
        }
    }
}

impl Eq for StartupError {}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ServerLifecycle(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_requires_exact_config_pair() {
        assert!(Cli::parse(["server".into(), "--config".into(), "config.toml".into()]).is_ok());
        for arguments in [
            vec!["server".into()],
            vec!["server".into(), "config.toml".into()],
            vec!["server".into(), "--config".into()],
            vec!["server".into(), "--config".into(), "".into()],
            vec![
                "server".into(),
                "--config".into(),
                "config.toml".into(),
                "extra".into(),
            ],
        ] {
            assert!(matches!(Cli::parse(arguments), Err(StartupError::Cli)));
        }
    }

    #[test]
    fn fatal_diagnostics_are_fixed_safe_codes() {
        let mut output = Vec::new();
        write_fatal_diagnostic(&mut output, &StartupError::Configuration).unwrap();
        assert_eq!(output, b"craxii fatal: invalid_configuration\n");
    }

    #[test]
    fn server_lifecycle_wrapper_preserves_the_original_typed_cause() {
        let error = StartupError::ServerLifecycle(
            crate::adapters::http::ServerError::InjectedSharedFailure,
        );
        assert_eq!(error.code(), "server_lifecycle_failure");
        let source = std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<crate::adapters::http::ServerError>()
            .unwrap();
        assert_eq!(
            source.kind(),
            crate::adapters::http::ServerErrorKind::InjectedSharedFailure
        );
    }

    #[test]
    fn execution_ownership_join_precedes_sqlite_close_in_bootstrap_shutdown() {
        let shutdown = include_str!("startup.rs");
        let begin = shutdown
            .find("begin_execution_shutdown(deadline)")
            .expect("Stage 10 deadline is propagated when execution shutdown begins");
        let join = shutdown
            .find("shutdown_executions_before(deadline)")
            .expect("execution supervisors are joined under the original deadline");
        let sqlite_close = shutdown
            .find("self.sqlite_runtime.shutdown().await")
            .expect("SQLite has an explicit final shutdown point");
        assert!(begin < join);
        assert!(join < sqlite_close);
    }

    #[test]
    fn sqlite_failures_map_to_fixed_startup_categories() {
        for (kind, expected) in [
            (
                SqliteFailureKind::AlreadyOwned,
                StartupError::StateRootAlreadyOwned,
            ),
            (
                SqliteFailureKind::NewerSchema,
                StartupError::IncompatibleSchema,
            ),
            (SqliteFailureKind::Corrupt, StartupError::DatabaseIntegrity),
            (
                SqliteFailureKind::InconsistentSchema,
                StartupError::DatabaseIntegrity,
            ),
            (
                SqliteFailureKind::UnsafeStatePath,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::UnsupportedFilesystem,
                StartupError::DatabaseLifecycle,
            ),
            (SqliteFailureKind::Storage, StartupError::DatabaseLifecycle),
            (
                SqliteFailureKind::BusyOrLocked,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::StateConflict,
                StartupError::DatabaseLifecycle,
            ),
            (
                SqliteFailureKind::InternalInvariant,
                StartupError::DatabaseLifecycle,
            ),
        ] {
            let error =
                StartupError::from_sqlite(crate::adapters::sqlite::SqliteAdapterError::new(kind));
            assert_eq!(error, expected);
            assert!(!error.code().contains('/'));
            assert!(std::error::Error::source(&error).is_none());
        }
    }
}
