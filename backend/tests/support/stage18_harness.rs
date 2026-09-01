#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use craxii_server::adapters::artifacts::LocalArtifactStore;
use craxii_server::adapters::http::{ConnectionRegistry, HttpState, ServerHandle};
use craxii_server::adapters::local_workstation::{LocalWorkstation, LocalWorkstationOptions};
use craxii_server::adapters::scripted_provider::{
    ScriptExpectation, ScriptGate, ScriptedProgram, ScriptedProvider, ScriptedStep,
};
use craxii_server::adapters::sqlite::{SqliteRuntimeGuard, SqliteStateStore};
use craxii_server::adapters::system_clock::SystemClock;
use craxii_server::application::agent_loop::{AgentLoop, AgentLoopLimits, AgentLoopRuntimeContext};
use craxii_server::application::authority::{
    AuthorityEvaluator, V0AuthorityConstraints, V0AuthorityEvaluator,
};
use craxii_server::application::context_assembler::{
    ContextAssembler, ContextAssemblyVersions, VersionedInstructionSnapshot,
};
use craxii_server::application::device_provisioning::DeviceProvisioningService;
use craxii_server::application::model_gateway::{ModelGateway, ModelGatewayLimits, NoopDraftSink};
use craxii_server::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
use craxii_server::application::runtime::{ControlledShutdown, HeartbeatTask, ShutdownController};
use craxii_server::application::scheduler::{
    SchedulerNotifier, SchedulerReadiness, SchedulerStart, start_scheduler,
};
use craxii_server::application::tool_execution_service::{ToolExecutionService, ToolRuntimeLimits};
use craxii_server::application::tool_registry::{ToolRegistry, ToolSemanticPolicy};
use craxii_server::application::transport::{CursorBroadcaster, MutationAdmission};
use craxii_server::bootstrap::health::{Health, HealthState};
use craxii_server::domain::model::{ModelUsage, RequiredModelCapabilities};
use craxii_server::domain::*;
use craxii_server::ports::artifact_store::ArtifactStore;
use craxii_server::ports::clock::Clock;
use craxii_server::ports::context_source_store::ContextSourceStore;
use craxii_server::ports::model_provider::{
    ConservativeTokenEstimate, FullJitterSource, ModelProvider, ModelProviderFuture,
    ModelProviderInvocation, ModelProviderStream, ProviderAttempt, ProviderError,
    ProviderErrorKind, ProviderOutcomeCertainty, TokenEstimateUnit, TokenEstimator,
};
use craxii_server::ports::state_store::{
    BootstrapObservation, BootstrapStateStore, ExecutionCapabilityObservation,
    LoadOrBootstrapIdentityRequest, ModelStateStore, V0IdentityReference,
};
use craxii_server::ports::workstation::{HARD_FILE_READ_MAX_BYTES, Workstation};
use craxii_server::ports::workstation_preparation::WorkstationPreparation;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Connection as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

const PROVIDER_ID: &str = "stage18-scripted";
const TARGET_ID: &str = "stage18-primary";
const ESTIMATOR_ID: &str = "stage18_fixed";
const SHELL: &str = "/bin/bash";
const SCHEMA_VERSION: i64 = 4;
const T0: &str = "2026-09-01T00:00:00.000000Z";
const REQUESTED_OUTPUT_TOKENS: i64 = 512;

static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct Stage18Root {
    path: PathBuf,
    cleanup: bool,
}

impl Stage18Root {
    pub fn new(label: &str) -> Self {
        let sequence = ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "craxii-stage18-{label}-{}-{sequence}",
            std::process::id()
        ));
        Self::create_empty(path)
    }

    pub fn recreate(path: PathBuf) -> Self {
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("craxii-stage18-")),
            "Stage 18 root must retain its scoped test prefix"
        );
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove prior Stage 18 root");
        }
        Self::create_empty(path)
    }

    fn create_empty(path: PathBuf) -> Self {
        fs::create_dir(&path).expect("create Stage 18 root");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("secure Stage 18 root");
        for child in ["state", "workspace"] {
            let directory = path.join(child);
            fs::create_dir(&directory).expect("create Stage 18 directory");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("secure Stage 18 directory");
        }
        fs::write(
            path.join("workspace/machine-note.txt"),
            b"stage18 real read_file evidence\n",
        )
        .expect("write Stage 18 fixture file");
        Self {
            path,
            cleanup: true,
        }
    }

    pub fn from_existing(path: PathBuf) -> Self {
        Self {
            path,
            cleanup: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state_root(&self) -> PathBuf {
        self.path.join("state")
    }

    pub fn workspace(&self) -> PathBuf {
        self.path.join("workspace")
    }

    pub fn artifact_root(&self) -> PathBuf {
        self.path.join("artifacts")
    }

    pub fn database(&self) -> PathBuf {
        self.path.join("state/db/craxii.sqlite3")
    }

    pub fn invocation_log(&self) -> PathBuf {
        self.path.join("provider-invocations.jsonl")
    }

    pub fn effect_log(&self) -> PathBuf {
        self.workspace().join("stage18-effect.log")
    }

    pub fn preserve(mut self) -> PathBuf {
        self.cleanup = false;
        self.path.clone()
    }

    pub fn remove(mut self) {
        self.cleanup = false;
        fs::remove_dir_all(&self.path).expect("remove Stage 18 root");
    }
}

impl Drop for Stage18Root {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MachineFacts {
    pub os: String,
    pub architecture: String,
    pub cwd: String,
    pub git_version: String,
}

impl MachineFacts {
    pub fn capture(workspace: &Path) -> Self {
        let output = std::process::Command::new(SHELL)
            .arg("-lc")
            .arg("uname -s; uname -m; pwd; git --version")
            .current_dir(workspace)
            .output()
            .expect("capture independent machine facts");
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("machine facts are UTF-8");
        let mut lines = stdout.lines();
        let facts = Self {
            os: lines.next().expect("OS line").to_owned(),
            architecture: lines.next().expect("architecture line").to_owned(),
            cwd: lines.next().expect("cwd line").to_owned(),
            git_version: lines.next().expect("git line").to_owned(),
        };
        assert!(lines.next().is_none());
        facts
    }

    pub fn answer(&self) -> String {
        format!(
            "OS: {}\nCPU architecture: {}\nCurrent directory: {}\nGit version: {}",
            self.os, self.architecture, self.cwd, self.git_version
        )
    }
}

#[derive(Clone, Debug)]
pub enum ProgramPlan {
    Tools(Vec<ToolPlan>),
    Answer {
        text: String,
        require_tool_result: Option<ModelToolCallId>,
    },
    Fail(ProviderError),
}

#[derive(Clone, Debug)]
pub struct ToolPlan {
    pub call_id: ModelToolCallId,
    pub name: &'static str,
    pub arguments: String,
}

impl ToolPlan {
    pub fn new(call_id: &str, name: &'static str, arguments: Value) -> Self {
        Self {
            call_id: ModelToolCallId::try_new(call_id).expect("valid fixture call ID"),
            name,
            arguments: serde_json::to_string(&arguments).expect("canonical fixture arguments"),
        }
    }
}

pub fn machine_programs(facts: &MachineFacts) -> (Vec<ScriptedProgram>, ModelToolCallId) {
    let (plans, shell_call) = machine_plans(facts);
    (programs(&plans), shell_call)
}

pub fn machine_plans(facts: &MachineFacts) -> (Vec<ProgramPlan>, ModelToolCallId) {
    let shell_call = ModelToolCallId::try_new("machine-shell").unwrap();
    let read_call = ModelToolCallId::try_new("machine-read").unwrap();
    let plans = vec![
        ProgramPlan::Tools(vec![
            ToolPlan {
                call_id: shell_call.clone(),
                name: "run_shell",
                arguments: serde_json::to_string(&json!({
                    "command": "uname -s; uname -m; pwd; git --version"
                }))
                .unwrap(),
            },
            ToolPlan {
                call_id: read_call,
                name: "read_file",
                arguments: serde_json::to_string(&json!({"path": "machine-note.txt"})).unwrap(),
            },
        ]),
        ProgramPlan::Answer {
            text: facts.answer(),
            require_tool_result: Some(shell_call.clone()),
        },
    ];
    (plans, shell_call)
}

pub fn programs(plans: &[ProgramPlan]) -> Vec<ScriptedProgram> {
    let target = target();
    plans
        .iter()
        .enumerate()
        .map(|(index, plan)| {
            let ordinal = u64::try_from(index + 1).unwrap();
            let attempt = match plan {
                ProgramPlan::Fail(_) => ProviderAttempt::try_new(u32::try_from(index + 1).unwrap())
                    .unwrap_or_else(|_| ProviderAttempt::try_new(1).unwrap()),
                ProgramPlan::Tools(_) | ProgramPlan::Answer { .. } => {
                    ProviderAttempt::try_new(1).unwrap()
                }
            };
            program(&target, ordinal, attempt, plan.clone())
        })
        .collect()
}

pub fn retry_programs(final_text: &str, attempts: u32) -> Vec<ScriptedProgram> {
    assert!((2..=3).contains(&attempts));
    let target = target();
    let mut result = Vec::new();
    for attempt in 1..attempts {
        result.push(program(
            &target,
            u64::from(attempt),
            ProviderAttempt::try_new(attempt).unwrap(),
            ProgramPlan::Fail(ProviderError::new(
                ProviderErrorKind::TemporarilyUnavailable,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
            )),
        ));
    }
    result.push(program(
        &target,
        u64::from(attempts),
        ProviderAttempt::try_new(attempts).unwrap(),
        ProgramPlan::Answer {
            text: final_text.to_owned(),
            require_tool_result: None,
        },
    ));
    result
}

pub fn gated_answer_program(
    final_text: &str,
    gate: ScriptGate,
    after_response_started: bool,
) -> Vec<ScriptedProgram> {
    let target = target();
    let mut scripted = program(
        &target,
        1,
        ProviderAttempt::try_new(1).unwrap(),
        ProgramPlan::Answer {
            text: final_text.to_owned(),
            require_tool_result: None,
        },
    );
    scripted.steps.insert(
        usize::from(after_response_started),
        ScriptedStep::AwaitRelease(gate),
    );
    vec![scripted]
}

fn program(
    target: &ModelTarget,
    invocation_ordinal: u64,
    attempt: ProviderAttempt,
    plan: ProgramPlan,
) -> ScriptedProgram {
    let request_id =
        ProviderEvidenceId::try_new(format!("stage18-req-{invocation_ordinal}")).unwrap();
    let response_id =
        ProviderEvidenceId::try_new(format!("stage18-resp-{invocation_ordinal}")).unwrap();
    let usage = ModelUsage::try_new(20, 0, 10, 0, 30).unwrap();
    let mut steps = vec![ScriptedStep::emit(ModelStreamEvent::ResponseStarted {
        target: target.identity(),
        provider_request_id: Some(request_id.clone()),
        provider_response_id: Some(response_id.clone()),
    })];
    let (items, stop_reason, required_prior_tool_result) = match plan {
        ProgramPlan::Tools(tools) => {
            let mut items = Vec::new();
            for (index, tool) in tools.into_iter().enumerate() {
                let item_ordinal = u32::try_from(index).unwrap();
                let call = CanonicalModelToolCall::try_new(
                    tool.call_id.clone(),
                    tool.name,
                    tool.arguments,
                )
                .unwrap();
                steps.extend([
                    ScriptedStep::emit(ModelStreamEvent::ToolCallStarted {
                        item_ordinal,
                        call_id: call.call_id().clone(),
                        name: call.name().clone(),
                    }),
                    ScriptedStep::emit(ModelStreamEvent::ToolArgumentDelta {
                        item_ordinal,
                        call_id: call.call_id().clone(),
                        delta: call.raw_arguments().to_owned(),
                    }),
                    ScriptedStep::emit(ModelStreamEvent::ToolCallCompleted {
                        item_ordinal,
                        call: call.clone(),
                    }),
                ]);
                items.push(ModelOutputItem::ToolCall(call));
            }
            (items, ModelStopReason::ToolContinuation, None)
        }
        ProgramPlan::Answer {
            text,
            require_tool_result,
        } => {
            let part = ModelTextPart::try_new(text).unwrap();
            steps.push(ScriptedStep::emit(ModelStreamEvent::TextDelta {
                item_ordinal: 0,
                delta: part.clone(),
            }));
            (
                vec![ModelOutputItem::text(vec![part]).unwrap()],
                ModelStopReason::Completed,
                require_tool_result,
            )
        }
        ProgramPlan::Fail(error) => {
            steps.clear();
            steps.push(ScriptedStep::Fail(error));
            return ScriptedProgram {
                expectation: ScriptExpectation {
                    target_id: target.reference().model_target_id().clone(),
                    request_sha256: None,
                    fixture_key: None,
                    required_prior_tool_result: None,
                    invocation_ordinal,
                    attempt,
                },
                steps,
            };
        }
    };
    let response = ModelResponse::try_new(ModelResponseInput {
        selected_target: target.identity(),
        output_items: items,
        stop_reason,
        usage,
        provider_request_id: Some(request_id),
        provider_response_id: Some(response_id),
        provider_continuation: None,
        provider_metadata: ProviderMetadata::default(),
    })
    .unwrap();
    steps.push(ScriptedStep::emit(ModelStreamEvent::Usage(usage)));
    steps.push(ScriptedStep::emit(ModelStreamEvent::Completed(response)));
    ScriptedProgram {
        expectation: ScriptExpectation {
            target_id: target.reference().model_target_id().clone(),
            request_sha256: None,
            fixture_key: None,
            required_prior_tool_result,
            invocation_ordinal,
            attempt,
        },
        steps,
    }
}

fn target() -> ModelTarget {
    let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: false,
        context_window_tokens: TokenCount::try_new(128_000).unwrap(),
        max_output_tokens: TokenCount::try_new(4_096).unwrap(),
    });
    ModelTarget::try_new(ModelTargetInput {
        reference: ProviderModelReference::new(
            ModelTargetId::try_new(TARGET_ID).unwrap(),
            ProviderId::try_new(PROVIDER_ID).unwrap(),
            ProviderModelId::try_new("fixture-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities,
        ),
        enabled: true,
        endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1").unwrap(),
        account_reference: ModelConfigReference::named("fixture-account").unwrap(),
        requested_output_tokens: TokenCount::try_new(REQUESTED_OUTPUT_TOKENS).unwrap(),
        estimator: TokenEstimatorIdentity::try_new(ESTIMATOR_ID, 1).unwrap(),
        provider_native_options: ProviderNativeOptions::new(false),
    })
    .unwrap()
}

#[derive(Debug)]
struct FixedEstimator {
    identity: TokenEstimatorIdentity,
    tokens: u64,
    call_count: AtomicU64,
    pause_on_second: Option<PathBuf>,
}

impl FixedEstimator {
    fn normal() -> Self {
        Self {
            identity: TokenEstimatorIdentity::try_new(ESTIMATOR_ID, 1).unwrap(),
            tokens: 512,
            call_count: AtomicU64::new(0),
            pause_on_second: None,
        }
    }

    fn over_limit() -> Self {
        Self {
            identity: TokenEstimatorIdentity::try_new(ESTIMATOR_ID, 1).unwrap(),
            tokens: 200_000,
            call_count: AtomicU64::new(0),
            pause_on_second: None,
        }
    }

    fn pause_on_second(marker: PathBuf) -> Self {
        Self {
            identity: TokenEstimatorIdentity::try_new(ESTIMATOR_ID, 1).unwrap(),
            tokens: 512,
            call_count: AtomicU64::new(0),
            pause_on_second: Some(marker),
        }
    }
}

impl TokenEstimator for FixedEstimator {
    fn identity(&self) -> &TokenEstimatorIdentity {
        &self.identity
    }

    fn estimate(
        &self,
        _: &ModelTarget,
        _: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError> {
        let call = self.call_count.fetch_add(1, Ordering::AcqRel) + 1;
        if call == 2
            && let Some(marker) = &self.pause_on_second
        {
            let file = std::fs::File::create(marker).map_err(|_| recorder_error())?;
            file.sync_all().map_err(|_| recorder_error())?;
            if let Some(parent) = marker.parent() {
                std::fs::File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|_| recorder_error())?;
            }
            loop {
                std::thread::park();
            }
        }
        ConservativeTokenEstimate::try_new(self.identity.clone(), self.tokens)
    }
}

#[derive(Debug)]
struct MinimumJitter;

impl FullJitterSource for MinimumJitter {
    fn sample_inclusive(&mut self, _: u64) -> u64 {
        0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InvocationRecord {
    pub work_id: String,
    pub logical_invocation_id: String,
    pub physical_attempt: u32,
    pub request_sha256: String,
}

#[derive(Debug)]
struct RecordingProvider {
    inner: Arc<ScriptedProvider>,
    database: PathBuf,
    ledger: PathBuf,
}

impl RecordingProvider {
    fn new(inner: Arc<ScriptedProvider>, database: PathBuf, ledger: PathBuf) -> Self {
        Self {
            inner,
            database,
            ledger,
        }
    }

    async fn record(&self, invocation: &ModelProviderInvocation) -> Result<(), ProviderError> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&self.database)
            .read_only(true);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .map_err(|_| recorder_error())?;
        let work_id: String = sqlx::query_scalar(
            "SELECT work_id FROM model_invocations WHERE logical_invocation_id = ? \
             AND attempt_no = ?",
        )
        .bind(invocation.request.logical_invocation_id().to_string())
        .bind(i64::from(invocation.attempt.get()))
        .fetch_one(&mut connection)
        .await
        .map_err(|_| recorder_error())?;
        connection.close().await.map_err(|_| recorder_error())?;
        let record = InvocationRecord {
            work_id,
            logical_invocation_id: invocation.request.logical_invocation_id().to_string(),
            physical_attempt: invocation.attempt.get(),
            request_sha256: invocation.request.canonical_sha256().to_string(),
        };
        let encoded = serde_json::to_vec(&record).map_err(|_| recorder_error())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.ledger)
            .map_err(|_| recorder_error())?;
        file.write_all(&encoded)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| recorder_error())
    }
}

impl ModelProvider for RecordingProvider {
    fn provider_id(&self) -> &ProviderId {
        self.inner.provider_id()
    }

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        self.inner.capabilities(target)
    }

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            self.record(&invocation).await?;
            self.inner.invoke_stream(invocation).await
        })
    }
}

fn recorder_error() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::InternalProviderError,
        ProviderOutcomeCertainty::DefinitelyNotSent,
    )
}

pub fn read_invocation_records(path: &Path) -> Vec<InvocationRecord> {
    match fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid invocation ledger line"))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read invocation ledger: {error}"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EstimatorMode {
    Normal,
    ContextLimit,
    PauseOnSecond(PathBuf),
}

pub struct Stage18Harness {
    pub root: Stage18Root,
    pub identity: V0IdentityReference,
    pub runtime_id: RuntimeInstanceId,
    pub health: Health,
    pub authority: String,
    pub bearer: String,
    pub provider: Arc<ScriptedProvider>,
    pub store: Arc<SqliteStateStore>,
    pub artifact_store: Arc<LocalArtifactStore>,
    guard: Option<SqliteRuntimeGuard>,
    server: Option<ServerHandle>,
    shutdown: Arc<ShutdownController<SqliteStateStore, SystemClock>>,
    scheduler_notifier: SchedulerNotifier,
    admission: MutationAdmission,
    local_workstation: Arc<LocalWorkstation>,
}

impl Stage18Harness {
    pub async fn start(
        root: Stage18Root,
        provider_programs: Vec<ScriptedProgram>,
        estimator_mode: EstimatorMode,
    ) -> Result<Self, String> {
        let state_root = root.state_root();
        let workspace = root.workspace();
        let guard = SqliteRuntimeGuard::start(&state_root, 4)
            .await
            .map_err(|error| error.to_string())?;
        let store = Arc::new(SqliteStateStore::new(guard.runtime().clone()));
        let artifact_store = Arc::new(
            LocalArtifactStore::initialize(&root.artifact_root())
                .map_err(|error| error.to_string())?,
        );
        let clock = Arc::new(SystemClock::new());
        let identity = store
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
                created_at: T0.parse().unwrap(),
                observation: observation(&workspace),
            })
            .await
            .map_err(|error| error.to_string())?
            .identity;
        store
            .verify_application_consistency()
            .await
            .map_err(|error| error.to_string())?;
        let referenced_artifacts = store
            .load_referenced_artifacts()
            .await
            .map_err(|error| error.to_string())?;
        let mut referenced_keys = BTreeSet::new();
        for artifact in &referenced_artifacts {
            artifact_store
                .verify(artifact)
                .map_err(|error| error.to_string())?;
            referenced_keys.insert(artifact.storage_key().clone());
        }
        let orphan_report = artifact_store
            .scan_orphans(&referenced_keys, now(clock.as_ref())?)
            .map_err(|error| error.to_string())?;
        let runtime_id = RuntimeInstanceId::generate();
        let runtime_started_at = now(clock.as_ref())?;
        let runtime = craxii_server::application::runtime::bootstrap_runtime(
            store.as_ref(),
            RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
                runtime_instance_id: runtime_id,
                craxii_id: identity.craxii_id,
                workstation_id: identity.workstation_id,
                workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
                linux_boot_id: Some(LinuxBootId::try_new("stage18-local-boot").unwrap()),
                diagnostic_pid: DiagnosticPid::try_new(std::process::id().into()).ok(),
                package_version: PackageVersion::try_new("0.0.1").unwrap(),
                git_revision: GitRevision::try_new("stage18-harness").unwrap(),
                schema_version: SchemaVersion::try_new(SCHEMA_VERSION).unwrap(),
                started_at: runtime_started_at,
            }),
            orphan_report.orphans.len() as u64,
            clock.as_ref(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let provisioned = DeviceProvisioningService::new(store.as_ref())
            .provision(
                DeviceDisplayName::try_new("Stage 18 harness".to_owned()).unwrap(),
                now(clock.as_ref())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut bearer_bytes = Vec::new();
        provisioned
            .write_bearer_once(&mut bearer_bytes)
            .map_err(|error| error.to_string())?;
        let bearer = String::from_utf8(bearer_bytes)
            .map_err(|error| error.to_string())?
            .trim()
            .to_owned();
        let snapshot = store
            .load_bootstrap_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let workstation_clock: Arc<dyn Clock> = clock.clone();
        let workstation_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let local_workstation = Arc::new(
            LocalWorkstation::new(
                &snapshot.workstation,
                &snapshot.workspace,
                LocalWorkstationOptions {
                    default_shell: LogicalPathReference::absolute(SHELL).unwrap(),
                    configured_workspace_root: workspace,
                    read_hard_limit: HARD_FILE_READ_MAX_BYTES,
                    artifact_store: workstation_artifacts,
                    administrative_enabled: false,
                    delegated_cgroup_root: None,
                    clock: workstation_clock,
                },
            )
            .map_err(|error| error.to_string())?,
        );
        if local_workstation.capabilities_snapshot() != &snapshot.workstation_capabilities {
            return Err("workstation capability mismatch".to_owned());
        }

        let policy = ToolSemanticPolicy {
            read_file_default_bytes: 65_536,
            read_file_max_bytes: 1_048_576,
            run_shell_command_max_bytes: 65_536,
            run_shell_default_timeout_ms: 120_000,
            run_shell_max_timeout_ms: 900_000,
        };
        let registry =
            Arc::new(ToolRegistry::v0(policy).map_err(|_| "invalid tool registry".to_owned())?);
        let tool_store: Arc<dyn craxii_server::ports::state_store::ToolStateStore> = store.clone();
        let workstation: Arc<dyn Workstation> = local_workstation.clone();
        let preparation: Arc<dyn WorkstationPreparation> = local_workstation.clone();
        let tool_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let tool_clock: Arc<dyn Clock> = clock.clone();
        let tool_execution = Arc::new(
            ToolExecutionService::new(
                Arc::clone(&registry),
                Arc::new(V0AuthorityEvaluator) as Arc<dyn AuthorityEvaluator>,
                tool_store,
                workstation,
                preparation,
                tool_artifacts,
                tool_clock,
                ToolRuntimeLimits {
                    read_file_default_bytes: policy.read_file_default_bytes,
                    read_file_max_bytes: policy.read_file_max_bytes,
                    run_shell_command_max_bytes: policy.run_shell_command_max_bytes,
                    run_shell_default_timeout_ms: policy.run_shell_default_timeout_ms,
                    run_shell_max_timeout_ms: policy.run_shell_max_timeout_ms,
                    stdout_capture_bytes: 8_388_608,
                    stderr_capture_bytes: 8_388_608,
                    inline_model_result_bytes: 65_536,
                    per_stream_projection_bytes: 32_768,
                },
            )
            .map_err(|error| error.to_string())?,
        );

        let target = target();
        let selection = Arc::new(ModelSelectionPolicy::new(Arc::new(
            ModelTargetSnapshot::try_new(
                target.reference().model_target_id().clone(),
                vec![target],
            )
            .map_err(|error| error.to_string())?,
        )));
        let source_store: Arc<dyn ContextSourceStore> = store.clone();
        let context_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let estimator: Arc<dyn TokenEstimator> = match estimator_mode {
            EstimatorMode::Normal => Arc::new(FixedEstimator::normal()),
            EstimatorMode::ContextLimit => Arc::new(FixedEstimator::over_limit()),
            EstimatorMode::PauseOnSecond(marker) => {
                Arc::new(FixedEstimator::pause_on_second(marker))
            }
        };
        let context_clock: Arc<dyn Clock> = clock.clone();
        let assembler = Arc::new(ContextAssembler::new(
            source_store,
            Some(context_artifacts),
            estimator,
            Arc::clone(&registry),
            VersionedInstructionSnapshot::v0(),
            context_clock,
        ));
        let scripted = Arc::new(ScriptedProvider::with_clock(
            ProviderId::try_new(PROVIDER_ID).unwrap(),
            provider_programs,
            clock.clone(),
        ));
        let gateway_provider: Arc<dyn ModelProvider> = Arc::new(RecordingProvider::new(
            Arc::clone(&scripted),
            root.database(),
            root.invocation_log(),
        ));
        let gateway_store: Arc<dyn ModelStateStore> = store.clone();
        let gateway_artifacts: Arc<dyn ArtifactStore> = artifact_store.clone();
        let gateway_clock: Arc<dyn Clock> = clock.clone();
        let gateway = Arc::new(
            ModelGateway::new(
                gateway_store,
                gateway_artifacts,
                gateway_provider,
                Arc::new(NoopDraftSink),
                gateway_clock,
                Box::new(MinimumJitter),
                ModelGatewayLimits::default(),
            )
            .map_err(|error| error.to_string())?,
        );
        let loop_store: Arc<dyn craxii_server::application::agent_loop::AgentLoopStateStore> =
            store.clone();
        let loop_clock: Arc<dyn Clock> = clock.clone();
        let runner = Arc::new(
            AgentLoop::new(
                selection,
                assembler,
                ContextAssemblyVersions::v0(),
                gateway,
                tool_execution,
                loop_store,
                loop_clock,
                required_capabilities(),
                AgentLoopRuntimeContext {
                    workstation: snapshot.workstation,
                    workspace: snapshot.workspace,
                    authority_constraints: V0AuthorityConstraints::default(),
                },
                AgentLoopLimits::default(),
            )
            .map_err(|error| error.to_string())?,
        );
        let health = Health::new();
        if health.snapshot().state() != HealthState::LiveUnready {
            return Err("new health was not live_unready".to_owned());
        }
        let (fatal, _) = tokio::sync::watch::channel(false);
        let heartbeat = HeartbeatTask::start(
            Arc::clone(&store),
            Arc::clone(&clock),
            health.clone(),
            runtime_id,
            fatal.clone(),
        );
        let shutdown = Arc::new(ShutdownController::new(
            Arc::clone(&store),
            Arc::clone(&clock),
            health.clone(),
            runtime_id,
            runtime.correlation_id,
            5_000,
            heartbeat,
        ));
        let scheduler = start_scheduler(
            Arc::clone(&store),
            runner,
            Arc::clone(&clock),
            health.clone(),
            fatal.clone(),
            SchedulerStart {
                runtime_instance_id: runtime_id,
                conversation_id: identity.conversation_id,
                readiness: SchedulerReadiness::ReadyAfterInitialScan,
            },
        )
        .map_err(|error| error.to_string())?;
        let scheduler_notifier = scheduler.notifier();
        shutdown
            .install_scheduler(scheduler)
            .await
            .map_err(|error| error.to_string())?;
        wait_ready(&health).await?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let authority = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .to_string();
        let admission = MutationAdmission::new();
        let cursors = CursorBroadcaster::new();
        let connections = ConnectionRegistry::default();
        let (ws_shutdown, _) = tokio::sync::watch::channel(false);
        let controlled_shutdown: Arc<dyn ControlledShutdown> = shutdown.clone();
        let http_state = HttpState::new(
            Arc::clone(&store),
            Arc::clone(&clock),
            health.clone(),
            admission.clone(),
            cursors,
            fatal,
            ws_shutdown,
            connections,
            vec![authority.clone()],
            Some(controlled_shutdown),
            Some(scheduler_notifier.clone()),
        );
        let server = ServerHandle::start(listener, http_state);
        Ok(Self {
            root,
            identity,
            runtime_id,
            health,
            authority,
            bearer,
            provider: scripted,
            store,
            artifact_store,
            guard: Some(guard),
            server: Some(server),
            shutdown,
            scheduler_notifier,
            admission,
            local_workstation,
        })
    }

    pub async fn submit_message(
        &self,
        text: &str,
        client_message_id: ClientMessageId,
    ) -> HttpResponse {
        let body = serde_json::to_vec(&json!({
            "protocol_version": 1,
            "client_message_id": client_message_id.to_string(),
            "content": [{"type": "text", "text": text}],
        }))
        .unwrap();
        self.http(
            "POST",
            &format!(
                "/v1/conversations/{}/messages",
                self.identity.conversation_id
            ),
            Some(&body),
            Some(&client_message_id.to_string()),
        )
        .await
    }

    pub async fn submit_message_losing_response(
        &self,
        text: &str,
        client_message_id: ClientMessageId,
    ) {
        let body = serde_json::to_vec(&json!({
            "protocol_version": 1,
            "client_message_id": client_message_id.to_string(),
            "content": [{"type": "text", "text": text}],
        }))
        .unwrap();
        let path = format!(
            "/v1/conversations/{}/messages",
            self.identity.conversation_id
        );
        let mut stream = tokio::net::TcpStream::connect(&self.authority)
            .await
            .expect("connect lost-response request");
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\nIdempotency-Key: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            self.authority,
            self.bearer,
            client_message_id,
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        for _ in 0..1_000 {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(self.root.database())
                .read_only(true);
            let mut connection = sqlx::SqliteConnection::connect_with(&options)
                .await
                .unwrap();
            let committed: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM client_commands WHERE idempotency_key = ? \
                 AND command_type = 'message'",
            )
            .bind(client_message_id.to_string())
            .fetch_one(&mut connection)
            .await
            .unwrap();
            connection.close().await.unwrap();
            if committed == 1 {
                drop(stream);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("lost-response command did not durably commit")
    }

    pub async fn cancel_work(&self, work_id: WorkId, command_id: ClientCommandId) -> HttpResponse {
        let body = serde_json::to_vec(&json!({
            "protocol_version": 1,
            "client_command_id": command_id.to_string(),
        }))
        .unwrap();
        self.http(
            "POST",
            &format!("/v1/work-items/{work_id}/cancel"),
            Some(&body),
            Some(&command_id.to_string()),
        )
        .await
    }

    pub async fn begin_graceful_shutdown(&self) {
        let deadline = self.shutdown.latch_shutdown_request();
        match self.health.snapshot().state() {
            HealthState::LiveUnready | HealthState::Ready => {
                self.health.mark_draining().expect("mark harness draining");
            }
            HealthState::Draining | HealthState::Fatal => {}
        }
        if let Some(server) = self.server.as_ref() {
            server.stop_accepting();
        }
        self.admission.close_and_wait().await;
        self.shutdown
            .request()
            .await
            .expect("begin harness shutdown");
        self.local_workstation.begin_execution_shutdown(deadline);
    }

    pub async fn bootstrap(&self) -> HttpResponse {
        self.http("GET", "/v1/bootstrap", None, None).await
    }

    async fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        idempotency_key: Option<&str>,
    ) -> HttpResponse {
        let mut stream = tokio::net::TcpStream::connect(&self.authority)
            .await
            .expect("connect Stage 18 HTTP");
        let body = body.unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n",
            self.authority, self.bearer
        );
        if let Some(key) = idempotency_key {
            request.push_str(&format!("Idempotency-Key: {key}\r\n"));
        }
        if !body.is_empty() {
            request.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        HttpResponse::parse(&bytes)
    }

    pub async fn replay_from_zero(&self) -> Vec<Value> {
        let url = format!("ws://{}/v1/events?after=0", self.authority);
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.bearer).parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let mut frames = Vec::new();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("replay frame timeout")
                .expect("replay socket closed")
                .expect("replay frame");
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                let complete =
                    value.get("event_type").and_then(Value::as_str) == Some("sync.complete");
                frames.push(value);
                if complete {
                    break;
                }
            }
        }
        socket.close(None).await.unwrap();
        frames
    }

    pub async fn wait_terminal(&self, work_id: WorkId) -> String {
        let mut last = None;
        for _ in 0..1_000 {
            let state = query_string(
                &self.root.database(),
                "SELECT state FROM work_items WHERE work_id = ?",
                work_id.to_string(),
            )
            .await;
            if matches!(
                state.as_deref(),
                Some("completed" | "failed" | "cancelled" | "interrupted")
            ) {
                return state.unwrap();
            }
            last = state;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "work did not terminalize; last_state={last:?}; health={}",
            self.health.snapshot().state().as_str()
        );
    }

    pub async fn shutdown(mut self) -> Stage18Root {
        let deadline = self.shutdown.latch_shutdown_request();
        match self.health.snapshot().state() {
            HealthState::LiveUnready | HealthState::Ready => {
                self.health.mark_draining().expect("mark harness draining");
            }
            HealthState::Draining | HealthState::Fatal => {}
        }
        if let Some(server) = self.server.as_ref() {
            server.stop_accepting();
        }
        self.admission.close_and_wait().await;
        self.shutdown
            .request()
            .await
            .expect("request harness shutdown");
        self.local_workstation.begin_execution_shutdown(deadline);
        self.local_workstation
            .shutdown_executions_before(deadline)
            .await
            .expect("shutdown harness executions");
        if let Some(server) = self.server.take() {
            server.close_websockets();
            self.shutdown
                .finish()
                .await
                .expect("finish harness shutdown");
            server
                .join_before(deadline)
                .await
                .expect("join Stage 18 HTTP server");
        }
        self.store
            .verify_application_consistency()
            .await
            .expect("Stage 18 shutdown consistency");
        if let Some(guard) = self.guard.take() {
            guard.shutdown().await;
        }
        self.root
    }

    pub async fn induce_storage_failure(&self) {
        self.guard
            .as_ref()
            .expect("live SQLite guard")
            .runtime()
            .close()
            .await;
        self.scheduler_notifier.wake();
    }

    pub async fn teardown_after_fatal(mut self) -> Stage18Root {
        let deadline = self.shutdown.latch_shutdown_request();
        if let Some(server) = self.server.as_ref() {
            server.stop_accepting();
        }
        self.admission.close_and_wait().await;
        let _ = self.shutdown.request().await;
        self.local_workstation.begin_execution_shutdown(deadline);
        let _ = self
            .local_workstation
            .shutdown_executions_before(deadline)
            .await;
        if let Some(server) = self.server.take() {
            server.close_websockets();
            let _ = self.shutdown.finish().await;
            let _ = server.join_before(deadline).await;
        }
        if let Some(guard) = self.guard.take() {
            guard.shutdown().await;
        }
        self.root
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn parse(bytes: &[u8]) -> Self {
        let marker = b"\r\n\r\n";
        let split = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("HTTP header terminator");
        let headers = std::str::from_utf8(&bytes[..split]).unwrap();
        let status = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        Self {
            status,
            body: bytes[split + marker.len()..].to_vec(),
        }
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("JSON HTTP response")
    }
}

fn observation(workspace: &Path) -> BootstrapObservation {
    let workspace = fs::canonicalize(workspace).unwrap();
    let workspace = workspace.to_str().unwrap().to_owned();
    BootstrapObservation {
        initial_generation: WorkstationGeneration::try_new(1).unwrap(),
        architecture: std::env::consts::ARCH.to_owned(),
        os_release: std::env::consts::OS.to_owned(),
        default_shell: SHELL.to_owned(),
        workspace_logical_name: "primary".to_owned(),
        workspace_logical_root: workspace.clone(),
        workspace_resolved_root: workspace,
        execution_capabilities: ExecutionCapabilityObservation {
            foreground_execute: true,
            privilege_administrative: false,
            process_group_cleanup: true,
            cgroup_cleanup: false,
        },
    }
}

fn required_capabilities() -> RequiredModelCapabilities {
    RequiredModelCapabilities {
        text_input: true,
        text_output: true,
        custom_tool_calling: true,
        streaming: true,
        ordered_output_items: true,
        structured_output: true,
        reasoning_continuation: false,
        required_output_tokens: TokenCount::try_new(REQUESTED_OUTPUT_TOKENS).unwrap(),
    }
}

fn now(clock: &dyn Clock) -> Result<UtcTimestamp, String> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

async fn wait_ready(health: &Health) -> Result<(), String> {
    for _ in 0..1_000 {
        match health.snapshot().state() {
            HealthState::Ready => return Ok(()),
            HealthState::Fatal => return Err("scheduler became fatal before ready".to_owned()),
            HealthState::LiveUnready | HealthState::Draining => {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }
    Err("scheduler did not become ready".to_owned())
}

pub async fn query_string(database: &Path, sql: &'static str, binding: String) -> Option<String> {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let value = sqlx::query_scalar(sql)
        .bind(binding)
        .fetch_optional(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

pub async fn query_count(database: &Path, sql: &'static str) -> i64 {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let value = sqlx::query_scalar(sql)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    value
}

pub fn effect_count(path: &Path) -> usize {
    match fs::read_to_string(path) {
        Ok(content) => content.lines().count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read effect log: {error}"),
    }
}

pub async fn sqlite_integrity(database: &Path) -> (String, i64) {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .foreign_keys(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let quick: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    (quick, foreign_keys)
}
