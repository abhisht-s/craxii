//! Explicit debug-only signed terminal and bounded-tool qualification programs.
use crate::adapters::{
    artifacts::LocalArtifactStore,
    scripted_provider::{
        ScriptExpectation, ScriptGate, ScriptedProgram, ScriptedProvider, ScriptedStep,
    },
    system_clock::SystemClock,
};
use crate::application::{
    agent_loop::{AgentLoop, AgentLoopLimits, AgentLoopRuntimeContext},
    authority::{V0AuthorityConstraints, V0AuthorityEvaluator},
    context_assembler::{ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot},
    model_gateway::{ModelGateway, ModelGatewayLimits, NoopDraftSink},
    model_selection::{ModelSelectionPolicy, ModelTargetSnapshot},
    scheduler::{WorkCancellation, WorkRunner, WorkRunnerExit},
    tool_execution_service::{ToolExecutionService, ToolRuntimeLimits},
    tool_registry::{ToolRegistry, ToolSemanticPolicy},
};
use crate::domain::model::{ModelUsage, RequiredModelCapabilities};
use crate::domain::*;
use crate::ports::{
    model_provider::*,
    state_store::{BootstrapStateStore, ClaimedWork},
    workstation::*,
    workstation_preparation::*,
};
use std::sync::Arc;
struct Estimate {
    identity: TokenEstimatorIdentity,
}
impl TokenEstimator for Estimate {
    fn identity(&self) -> &TokenEstimatorIdentity {
        &self.identity
    }
    fn estimate(
        &self,
        _: &ModelTarget,
        _: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError> {
        ConservativeTokenEstimate::try_new(self.identity.clone(), 1024)
    }
}
struct Jitter;
impl FullJitterSource for Jitter {
    fn sample_inclusive(&mut self, _: u64) -> u64 {
        0
    }
}
struct Denied;
fn denied<T>() -> WorkstationFuture<'static, T> {
    Box::pin(async {
        Err(WorkstationError::new(
            WorkstationErrorKind::UnsupportedCapability,
        ))
    })
}
impl Workstation for Denied {
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
impl WorkstationPreparation for Denied {
    fn prepare(
        &self,
        _: WorkstationPreparationRequest,
    ) -> WorkstationPreparationFuture<'_, WorkstationPreparationResult> {
        denied()
    }
}
pub(crate) async fn complete(
    store: &crate::adapters::sqlite::SqliteStateStore,
    config: &super::startup::DistributionConfig,
    claimed: ClaimedWork,
    #[cfg(target_os = "linux")] permit: Option<crate::compute::worker::WorkPermit>,
) -> Result<(), &'static str> {
    let snapshot = store
        .load_bootstrap_snapshot()
        .await
        .map_err(|_| "bootstrap")?;
    let artifacts = Arc::new(
        LocalArtifactStore::initialize(std::path::Path::new(&config.artifact_root))
            .map_err(|_| "bootstrap")?,
    );
    let clock = Arc::new(SystemClock::new());
    let store = Arc::new(store.clone());
    #[cfg(target_os = "linux")]
    let permit_monitor = permit.clone();
    #[cfg(target_os = "linux")]
    let metered = if let Some(p) = permit {
        use crate::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
        use crate::guest_boundary::storage;
        Some(Arc::new(
            LocalWorkstation::new_installed(
                &snapshot.workstation,
                &snapshot.workspace,
                LocalWorkstationOptions {
                    default_shell: LogicalPathReference::absolute("/bin/bash")
                        .map_err(|_| "tool_construct")?,
                    configured_workspace_root: storage::WORKSPACE.into(),
                    read_hard_limit: HARD_FILE_READ_MAX_BYTES,
                    artifact_store: artifacts.clone(),
                    administrative_enabled: false,
                    user_switch_launcher: Some(
                        "/opt/craxii/current/craxii-workstation-launcher".into(),
                    ),
                    credential_free_direct_execution: false,
                    delegated_cgroup_root: Some(storage::TOOLS_CGROUP.into()),
                    clock: clock.clone(),
                },
            )
            .and_then(|w| w.with_compute_permit(p))
            .map_err(|_| "tool_construct")?,
        ))
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    let is_tool = metered.is_some();
    #[cfg(not(target_os = "linux"))]
    let is_tool = false;
    let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: false,
        context_window_tokens: TokenCount::try_new(128000).map_err(|_| "construction_23")?,
        max_output_tokens: TokenCount::try_new(4096).map_err(|_| "construction_23")?,
    });
    let target = ModelTarget::try_new(ModelTargetInput {
        reference: ProviderModelReference::new(
            ModelTargetId::try_new("terminal-fixture").map_err(|_| "construction_24")?,
            ProviderId::try_new("fixture").map_err(|_| "construction_24")?,
            ProviderModelId::try_new("terminal-text").map_err(|_| "construction_24")?,
            TargetConfigurationVersion::try_new(1).map_err(|_| "construction_24")?,
            capabilities,
        ),
        enabled: true,
        endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
            .map_err(|_| "construction_24")?,
        account_reference: ModelConfigReference::named("fixture").map_err(|_| "construction_24")?,
        requested_output_tokens: TokenCount::try_new(512).map_err(|_| "construction_24")?,
        estimator: TokenEstimatorIdentity::try_new("terminal_fixed", 1)
            .map_err(|_| "construction_24")?,
        provider_native_options: ProviderNativeOptions::new(false),
    })
    .map_err(|_| "construction_24")?;
    let part = ModelTextPart::try_new(
        "Synthetic terminal result.\nExact Unicode: é e\u{301}; opaque <b>text</b>.\u{0}",
    )
    .map_err(|_| "construction_25")?;
    let usage = ModelUsage::try_new(10, 0, 5, 0, 15).map_err(|_| "construction_25")?;
    let response = ModelResponse::try_new(ModelResponseInput {
        selected_target: target.identity(),
        output_items: vec![
            ModelOutputItem::text(vec![part.clone()]).map_err(|_| "construction_26")?,
        ],
        stop_reason: ModelStopReason::Completed,
        usage: Some(usage),
        provider_request_id: None,
        provider_response_id: None,
        provider_continuation: None,
        provider_metadata: ProviderMetadata::default(),
    })
    .map_err(|_| "construction_26")?;
    let mut program = ScriptedProgram {
        expectation: ScriptExpectation {
            target_id: target.reference().model_target_id().clone(),
            request_sha256: None,
            fixture_key: None,
            required_user_message_sha256: None,
            required_prior_tool_result: None,
            invocation_ordinal: 1,
            attempt: ProviderAttempt::try_new(1).map_err(|_| "construction_27")?,
        },
        steps: vec![
            ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                target: target.identity(),
                provider_request_id: None,
                provider_response_id: None,
            }),
            ScriptedStep::emit(ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: part,
            }),
            ScriptedStep::emit(ModelStreamEvent::Usage(usage)),
            ScriptedStep::emit(ModelStreamEvent::Completed(Box::new(response))),
        ],
    };
    let tool_gate = ScriptGate::new();
    if is_tool {
        // Fixed harmless local fixture: TERM-ignoring detached descendant attempts a late
        // canary. This uses the real launcher/cgroup and cannot select customer commands.
        let command = "trap '' TERM; python3 -c 'import os,time; p=os.fork(); os.setsid() if p==0 else None; time.sleep(20); open(\".stage14-late-effect\",\"w\").write(\"unexpected\")'";
        let call = CanonicalModelToolCall::try_new(
            ModelToolCallId::try_new("bounded-compute-fixture").map_err(|_| "tool_program")?,
            "run_shell",
            serde_json::json!({"command":command,"timeout_seconds":5}).to_string(),
        )
        .map_err(|_| "tool_program")?;
        let response = ModelResponse::try_new(ModelResponseInput {
            selected_target: target.identity(),
            output_items: vec![ModelOutputItem::ToolCall(call.clone())],
            stop_reason: ModelStopReason::ToolContinuation,
            usage: Some(usage),
            provider_request_id: None,
            provider_response_id: None,
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        })
        .map_err(|_| "tool_program")?;
        program.steps = vec![
            ScriptedStep::AwaitRelease(tool_gate.clone()),
            ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
                target: target.identity(),
                provider_request_id: None,
                provider_response_id: None,
            }),
            ScriptedStep::emit(ModelStreamEvent::ToolCallStarted {
                item_ordinal: 0,
                call_id: call.call_id().clone(),
                name: call.name().clone(),
            }),
            ScriptedStep::emit(ModelStreamEvent::ToolArgumentDelta {
                item_ordinal: 0,
                call_id: call.call_id().clone(),
                delta: call.raw_arguments().to_owned(),
            }),
            ScriptedStep::emit(ModelStreamEvent::ToolCallCompleted {
                item_ordinal: 0,
                call,
            }),
            ScriptedStep::emit(ModelStreamEvent::Usage(usage)),
            ScriptedStep::emit(ModelStreamEvent::Completed(Box::new(response))),
        ];
    }
    #[cfg(target_os = "linux")]
    let workstation: Arc<dyn Workstation> = metered
        .as_ref()
        .map(|w| w.clone() as Arc<dyn Workstation>)
        .unwrap_or_else(|| Arc::new(Denied));
    #[cfg(target_os = "linux")]
    let preparation: Arc<dyn WorkstationPreparation> = metered
        .as_ref()
        .map(|w| w.clone() as Arc<dyn WorkstationPreparation>)
        .unwrap_or_else(|| Arc::new(Denied));
    #[cfg(not(target_os = "linux"))]
    let workstation: Arc<dyn Workstation> = Arc::new(Denied);
    #[cfg(not(target_os = "linux"))]
    let preparation: Arc<dyn WorkstationPreparation> = Arc::new(Denied);
    let provider = Arc::new(ScriptedProvider::with_clock(
        ProviderId::try_new("fixture").map_err(|_| "construction_28")?,
        vec![program],
        clock.clone(),
    ));
    let policy = ToolSemanticPolicy {
        read_file_default_bytes: 4096,
        read_file_max_bytes: 65536,
        run_shell_command_max_bytes: 65536,
        run_shell_default_timeout_ms: if is_tool { 5000 } else { 60000 },
        run_shell_max_timeout_ms: if is_tool { 5000 } else { 900000 },
    };
    let registry = Arc::new(ToolRegistry::v0(policy).map_err(|_| "construction_29")?);
    let tools = Arc::new(
        ToolExecutionService::new(
            registry.clone(),
            Arc::new(V0AuthorityEvaluator),
            store.clone(),
            workstation,
            preparation,
            artifacts.clone(),
            clock.clone(),
            ToolRuntimeLimits {
                read_file_default_bytes: 4096,
                read_file_max_bytes: 65536,
                run_shell_command_max_bytes: 65536,
                run_shell_default_timeout_ms: if is_tool { 5000 } else { 60000 },
                run_shell_max_timeout_ms: if is_tool { 5000 } else { 900000 },
                stdout_capture_bytes: 32768,
                stderr_capture_bytes: 32768,
                inline_model_result_bytes: 65536,
                per_stream_projection_bytes: 32768,
            },
        )
        .map_err(|_| "construction_30")?,
    );
    let assembler = Arc::new(ContextAssembler::new(
        store.clone(),
        Some(artifacts.clone()),
        Arc::new(Estimate {
            identity: TokenEstimatorIdentity::try_new("terminal_fixed", 1)
                .map_err(|_| "construction_31")?,
        }),
        registry,
        VersionedInstructionSnapshot::v0(),
        clock.clone(),
    ));
    let gateway = Arc::new(
        ModelGateway::new(
            store.clone(),
            artifacts,
            provider.clone(),
            Arc::new(NoopDraftSink),
            clock.clone(),
            Box::new(Jitter),
            ModelGatewayLimits::default(),
        )
        .map_err(|_| "construction_32")?,
    );
    let selection = Arc::new(ModelSelectionPolicy::new(Arc::new(
        ModelTargetSnapshot::try_new(target.reference().model_target_id().clone(), vec![target])
            .map_err(|_| "construction_33")?,
    )));
    let runner = Arc::new(
        AgentLoop::new(
            selection,
            assembler,
            ContextAssemblyVersions::v0(),
            gateway,
            tools,
            store.clone(),
            clock,
            RequiredModelCapabilities {
                text_input: true,
                text_output: true,
                custom_tool_calling: true,
                streaming: true,
                ordered_output_items: true,
                structured_output: true,
                reasoning_continuation: false,
                required_output_tokens: TokenCount::try_new(512).map_err(|_| "construction_34")?,
            },
            AgentLoopRuntimeContext {
                workstation: snapshot.workstation,
                workspace: snapshot.workspace,
                authority_constraints: V0AuthorityConstraints::default(),
            },
            AgentLoopLimits {
                model_steps_per_work: 1,
                provider_attempts_per_work: 1,
                tool_calls_per_work: 1,
                work_duration: std::time::Duration::from_secs(60),
            },
        )
        .map_err(|_| "construction_34")?,
    );
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let work_id = claimed.work.work_id();
    let runtime_id = claimed.lifecycle.runtime_owner().ok_or("runtime_owner")?;
    #[cfg(target_os = "linux")]
    let barrier = permit_monitor.clone().map(|p| {
        tokio::spawn(async move {
            // Context assembly and model intent commit precede this provider barrier. Release
            // only inside the original finite profile, so a real tool crosses its cutoff.
            while p
                .profile_cutoff
                .saturating_duration_since(std::time::Instant::now())
                > std::time::Duration::from_secs(9)
            {
                if p.prepare().is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            tool_gate.release();
        })
    });
    let task = runner
        .start(claimed, WorkCancellation::isolated_fixture(receiver))
        .map_err(|_| "construction_35")?;
    tokio::pin!(task);
    #[cfg(target_os = "linux")]
    let exit = if let (Some(w), Some(p)) = (metered, permit_monitor) {
        use crate::ports::state_store::SchedulerStateStore;
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|_| "signal")?;
        let stopping = async {
            loop {
                if p.prepare().is_err() {
                    break;
                }
                tokio::select! { _=term.recv()=>break, _=tokio::time::sleep(std::time::Duration::from_millis(50))=>{} }
            }
        };
        let exit = tokio::select! {
            result=&mut task=>result,
            _=stopping=>{
                store.request_owned_work_cancellation(crate::ports::state_store::RequestOwnedCancellationRequest{runtime_id,requested_at:super::timestamp().map_err(|_|"cancel_time")?}).await.map_err(|_|"cancel_commit")?;
                w.begin_execution_shutdown(p.cleanup_cutoff());
                sender.send_replace(true);
                // Original funded cleanup deadline; no restart-relative grace.
                tokio::time::timeout_at(p.cleanup_cutoff(),&mut task).await.map_err(|_|"drain_timeout")?
            }
        };
        w.shutdown_executions_before(p.cleanup_cutoff())
            .await
            .map_err(|_| "cleanup")?;
        let cancellation_pending = store
            .list_current_runtime_cancel_requested(runtime_id)
            .await
            .map_err(|_| "cancel_state")?
            .iter()
            .any(|w| w.work_id == work_id && w.current_attempt == CurrentWorkAttempt::None);
        if exit == WorkRunnerExit::CancellationConfirmed || cancellation_pending {
            store
                .finish_cancellation(crate::ports::state_store::FinishCancellationRequest {
                    work_id,
                    runtime_id,
                    confirmed_at: super::timestamp().map_err(|_| "cancel_time")?,
                    event_id: JournalEventId::generate(),
                })
                .await
                .map_err(|_| "cancel_finish")?;
        }
        exit
    } else {
        task.await
    };
    #[cfg(not(target_os = "linux"))]
    let exit = {
        let _ = (sender, work_id, runtime_id);
        task.await
    };
    #[cfg(target_os = "linux")]
    if let Some(b) = barrier {
        b.abort();
    }
    if !matches!(
        exit,
        WorkRunnerExit::TerminalCommitted | WorkRunnerExit::CancellationConfirmed
    ) || provider.invocation_count() != 1
    {
        return Err("loop_exit");
    }
    Ok(())
}
