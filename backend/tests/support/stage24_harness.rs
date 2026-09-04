#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use craxii_server::adapters::scripted_provider::ScriptedProvider;
use craxii_server::adapters::telemetry;
use craxii_server::bootstrap::config;
use craxii_server::domain::model::ModelUsage;
use craxii_server::domain::{
    CanonicalModelToolCall, ModelCapabilitySnapshot, ModelInputItem, ModelInputRole,
    ModelOutputItem, ModelRequest, ModelResponse, ModelResponseInput, ModelStopReason,
    ModelStreamEvent, ModelTarget, ModelTextPart, ModelToolCallId, ProviderEvidenceId, ProviderId,
    ProviderMetadata, Sha256Digest,
};
use craxii_server::ports::model_provider::{
    ModelProvider, ModelProviderFuture, ModelProviderInvocation, ModelProviderStream,
    ProviderError, ProviderErrorKind, ProviderOutcomeCertainty,
};
use craxii_server::ports::state_store::BootstrapStateStore as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection as _, Row as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::stage18_harness::{
    EstimatorMode, MachineFacts, ProgramPlan, Stage18Harness, Stage18Root, ToolPlan, programs,
};

pub const CANONICAL_REQUEST: &str = "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.";
pub const FOLLOW_UP: &str = "What Git version did you just tell me I have?";

const FIRST_CLIENT_ID: &str = "01890f6c-7b3a-7cc0-98f1-02e6f7a82401";
const FOLLOW_CLIENT_ID: &str = "01890f6c-7b3a-7cc0-98f1-02e6f7a82402";
const SHELL_CALL_ID: &str = "stage24-machine-shell";
const READ_CALL_ID: &str = "stage24-machine-read";
const CREDENTIAL_CANARY_NAME: &str = "CRAXII_STAGE24_PROVIDER_CREDENTIAL_CANARY";
const CREDENTIAL_CANARY_VALUE: &str = "stage24-credential-canary-must-not-inherit";
const FIXTURE_CONTENT: &str = "stage24-fixture-content-canary-real-workspace-read\n";
const RAW_SHELL_COMMAND: &str = concat!(
    "uname -s; uname -m; pwd; git --version; ",
    "if [ -z \"${CRAXII_STAGE24_PROVIDER_CREDENTIAL_CANARY+x}\" ]; ",
    "then printf 'credential_canary=absent\\n'; ",
    "else printf 'credential_canary=present\\n'; fi"
);
const CHILD_ROOT_ENV: &str = "CRAXII_STAGE24_CHILD_ROOT";
const CHILD_PHASE_ENV: &str = "CRAXII_STAGE24_CHILD_PHASE";
const FILE_WAIT: Duration = Duration::from_secs(30);
const FAILURE_CLEANUP_WAIT: Duration = Duration::from_secs(8);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn expected_first_answer(facts: &MachineFacts) -> String {
    format!(
        "{}\nWorkspace file SHA-256: {}",
        facts.answer(),
        Sha256Digest::hash_bytes(FIXTURE_CONTENT.as_bytes())
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReadyRecord {
    configuration_version: u64,
    protocol_version: u64,
    authority: String,
    bearer: String,
    runtime_id: String,
    craxii_id: String,
    conversation_id: String,
    workstation_id: String,
    workspace_id: String,
    workspace_root: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ObservedFacts {
    os: String,
    architecture: String,
    cwd: String,
    git_version: String,
}

impl From<&MachineFacts> for ObservedFacts {
    fn from(value: &MachineFacts) -> Self {
        Self {
            os: value.os.clone(),
            architecture: value.architecture.clone(),
            cwd: value.cwd.clone(),
            git_version: value.git_version.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ToolResultProof {
    call_id: String,
    tool_execution_id: String,
    provider_call_id: String,
    result_kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProviderProof {
    phase: String,
    invocation_count: u64,
    canonical_prompt_seen: bool,
    follow_up_seen: bool,
    durable_prior_assistant_seen: bool,
    durable_tool_values_verified: bool,
    credential_canary_absent: bool,
    fixture_content_verified: bool,
    answer_source: String,
    answer_context_verified: bool,
    facts: ObservedFacts,
    tools: Vec<ToolResultProof>,
}

#[derive(Clone)]
struct Stage24AnswerGate {
    sender: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Stage24AnswerGate {
    fn new() -> Self {
        let (sender, _) = tokio::sync::watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }

    fn release(&self) {
        self.sender.send_replace(true);
    }

    async fn wait(&self) {
        if *self.sender.borrow() {
            return;
        }
        let mut receiver = self.sender.subscribe();
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Stage24ProviderPhase {
    FirstWork,
    FollowUp,
}

#[derive(Clone)]
struct Stage24ProviderCapture {
    request: ModelRequest,
    answer: Option<String>,
    answer_source: Option<&'static str>,
}

#[derive(Default)]
struct Stage24ProviderState {
    invocation_count: u64,
    captures: Vec<Stage24ProviderCapture>,
}

struct ContextAnsweringProvider {
    phase: Stage24ProviderPhase,
    scripted_tools: Arc<ScriptedProvider>,
    answer_gate: Stage24AnswerGate,
    state: Mutex<Stage24ProviderState>,
}

impl ContextAnsweringProvider {
    fn first_work(answer_gate: Stage24AnswerGate) -> Self {
        let programs = programs(&[ProgramPlan::Tools(vec![
            ToolPlan::new(
                SHELL_CALL_ID,
                "run_shell",
                json!({"command": RAW_SHELL_COMMAND}),
            ),
            ToolPlan::new(
                READ_CALL_ID,
                "read_file",
                json!({"path": "machine-note.txt"}),
            ),
        ])]);
        Self {
            phase: Stage24ProviderPhase::FirstWork,
            scripted_tools: Arc::new(ScriptedProvider::new(stage24_provider_id(), programs)),
            answer_gate,
            state: Mutex::new(Stage24ProviderState::default()),
        }
    }

    fn follow_up(answer_gate: Stage24AnswerGate) -> Self {
        Self {
            phase: Stage24ProviderPhase::FollowUp,
            scripted_tools: Arc::new(ScriptedProvider::new(stage24_provider_id(), Vec::new())),
            answer_gate,
            state: Mutex::new(Stage24ProviderState::default()),
        }
    }

    fn invocation_count(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invocation_count
    }

    fn captures(&self) -> Vec<Stage24ProviderCapture> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .captures
            .clone()
    }

    fn capture_invocation(
        &self,
        request: ModelRequest,
        answer: Option<String>,
        answer_source: Option<&'static str>,
    ) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.invocation_count = state.invocation_count.saturating_add(1);
        let ordinal = state.invocation_count;
        state.captures.push(Stage24ProviderCapture {
            request,
            answer,
            answer_source,
        });
        ordinal
    }
}

impl ModelProvider for ContextAnsweringProvider {
    fn provider_id(&self) -> &ProviderId {
        self.scripted_tools.provider_id()
    }

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        self.scripted_tools.capabilities(target)
    }

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            let next_ordinal = self.invocation_count().saturating_add(1);
            match (self.phase, next_ordinal) {
                (Stage24ProviderPhase::FirstWork, 1) => {
                    self.capture_invocation(invocation.request.clone(), None, None);
                    self.scripted_tools.invoke_stream(invocation).await
                }
                (Stage24ProviderPhase::FirstWork, 2) => {
                    let derived = derive_first_answer(invocation.request.ordered_input_items())
                        .map_err(|_| invalid_context_program())?;
                    let answer = derived.answer();
                    let ordinal = self.capture_invocation(
                        invocation.request.clone(),
                        Some(answer.clone()),
                        Some("model_visible_tool_results"),
                    );
                    Ok(Box::new(Stage24AnswerStream::new(
                        &invocation,
                        answer,
                        ordinal,
                        self.answer_gate.clone(),
                    )?) as Box<dyn ModelProviderStream>)
                }
                (Stage24ProviderPhase::FollowUp, 1) => {
                    let derived = derive_follow_up_answer(invocation.request.ordered_input_items())
                        .map_err(|_| invalid_context_program())?;
                    let answer = format!("You have {}.", derived.git_version);
                    let ordinal = self.capture_invocation(
                        invocation.request.clone(),
                        Some(answer.clone()),
                        Some("durable_prior_assistant_and_tool_context"),
                    );
                    Ok(Box::new(Stage24AnswerStream::new(
                        &invocation,
                        answer,
                        ordinal,
                        self.answer_gate.clone(),
                    )?) as Box<dyn ModelProviderStream>)
                }
                _ => {
                    self.capture_invocation(invocation.request, None, None);
                    Err(invalid_context_program())
                }
            }
        })
    }
}

enum Stage24AnswerStep {
    Emit(Box<ModelStreamEvent>),
    AwaitRelease(Stage24AnswerGate),
}

struct Stage24AnswerStream {
    steps: VecDeque<Stage24AnswerStep>,
    cancellation: craxii_server::ports::model_provider::ProviderCancellationToken,
    semantic_output_observed: bool,
    terminal: bool,
}

impl Stage24AnswerStream {
    fn new(
        invocation: &ModelProviderInvocation,
        answer: String,
        ordinal: u64,
        gate: Stage24AnswerGate,
    ) -> Result<Self, ProviderError> {
        let request_id = ProviderEvidenceId::try_new(format!("stage24-derived-req-{ordinal}"))
            .map_err(|_| invalid_context_program())?;
        let response_id = ProviderEvidenceId::try_new(format!("stage24-derived-resp-{ordinal}"))
            .map_err(|_| invalid_context_program())?;
        let usage = ModelUsage::try_new(20, 0, 10, 0, 30).map_err(|_| invalid_context_program())?;
        let text = ModelTextPart::try_new(answer).map_err(|_| invalid_context_program())?;
        let response = ModelResponse::try_new(ModelResponseInput {
            selected_target: invocation.request.target().identity(),
            output_items: vec![
                ModelOutputItem::text(vec![text.clone()]).map_err(|_| invalid_context_program())?,
            ],
            stop_reason: ModelStopReason::Completed,
            usage: Some(usage),
            provider_request_id: Some(request_id.clone()),
            provider_response_id: Some(response_id.clone()),
            provider_continuation: None,
            provider_metadata: ProviderMetadata::default(),
        })
        .map_err(|_| invalid_context_program())?;
        Ok(Self {
            steps: [
                Stage24AnswerStep::Emit(Box::new(ModelStreamEvent::ResponseStarted {
                    target: invocation.request.target().identity(),
                    provider_request_id: Some(request_id),
                    provider_response_id: Some(response_id),
                })),
                Stage24AnswerStep::AwaitRelease(gate),
                Stage24AnswerStep::Emit(Box::new(ModelStreamEvent::TextDelta {
                    item_ordinal: 0,
                    delta: text,
                })),
                Stage24AnswerStep::Emit(Box::new(ModelStreamEvent::Usage(usage))),
                Stage24AnswerStep::Emit(Box::new(ModelStreamEvent::Completed(Box::new(response)))),
            ]
            .into(),
            cancellation: invocation.control.cancellation().clone(),
            semantic_output_observed: false,
            terminal: false,
        })
    }
}

impl ModelProviderStream for Stage24AnswerStream {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>> {
        Box::pin(async move {
            if self.terminal {
                return Ok(None);
            }
            loop {
                if self.cancellation.is_cancelled() {
                    self.terminal = true;
                    return Err(cancelled_context_program(self.semantic_output_observed));
                }
                match self.steps.pop_front() {
                    Some(Stage24AnswerStep::Emit(event)) => {
                        let event = *event;
                        self.semantic_output_observed |= event.is_semantic_output();
                        self.terminal = event.is_terminal();
                        return Ok(Some(event));
                    }
                    Some(Stage24AnswerStep::AwaitRelease(gate)) => {
                        tokio::select! {
                            () = gate.wait() => {}
                            () = self.cancellation.cancelled() => {
                                self.terminal = true;
                                return Err(cancelled_context_program(self.semantic_output_observed));
                            }
                        }
                    }
                    None => {
                        self.terminal = true;
                        return Ok(None);
                    }
                }
            }
        })
    }
}

fn stage24_provider_id() -> ProviderId {
    ProviderId::try_new("stage18-scripted").unwrap()
}

fn invalid_context_program() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::InvalidScriptProgram,
        ProviderOutcomeCertainty::DefinitelyNotSent,
    )
}

fn cancelled_context_program(semantic_output_observed: bool) -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::Cancelled,
        if semantic_output_observed {
            ProviderOutcomeCertainty::SemanticOutputObserved
        } else {
            ProviderOutcomeCertainty::ProviderOutcomeUnknown
        },
    )
}

#[derive(Clone, Debug)]
struct PhaseResult {
    ready: ReadyRecord,
    proof: ProviderProof,
    telemetry: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedStage24Evidence {
    pub contract_version: &'static str,
    pub schema_version: u64,
    pub protocol_version: u64,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub completed_work: u64,
    pub model_attempts: u64,
    pub tool_executions: u64,
    pub runtime_count: u64,
    pub first_work_model_steps: Vec<u64>,
    pub first_work_tools: Vec<String>,
    pub follow_up_model_steps: Vec<u64>,
    pub work_results: Vec<String>,
    pub model_results: Vec<String>,
    pub tool_results: Vec<String>,
    pub first_live_event_types: Vec<String>,
    pub reconnect_event_types: Vec<String>,
    pub replay_event_types: Vec<String>,
    pub message_roles: Vec<String>,
    pub work_ordinals: Vec<u64>,
    pub works: Vec<NormalizedWorkEvidence>,
    pub runtimes: Vec<NormalizedRuntimeEvidence>,
    pub provider_dependencies: Vec<NormalizedProviderDependency>,
    pub reconnect: NormalizedReconnectEvidence,
    pub telemetry_chains: Vec<NormalizedTelemetryChain>,
    pub satisfied_relationships: Vec<String>,
    pub retransmission_deduplicated: bool,
    pub provider_tool_values_verified: bool,
    pub durable_history_verified: bool,
    pub restart_identity_contract_verified: bool,
    pub persistence_and_artifact_integrity_verified: bool,
    pub operator_evidence_verified: bool,
    pub telemetry_composition_and_redaction_verified: bool,
    pub portable_host_result: &'static str,
    pub ubuntu_target_result: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedWorkEvidence {
    pub alias: String,
    pub ordinal: u64,
    pub state: String,
    pub terminal_reason: Option<String>,
    pub correlation_matches_work: bool,
    pub model_attempts: Vec<NormalizedModelEvidence>,
    pub tools: Vec<NormalizedToolEvidence>,
    pub artifacts: Vec<NormalizedArtifactEvidence>,
    pub journal: Vec<NormalizedJournalEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedModelEvidence {
    pub alias: String,
    pub logical_invocation_alias: String,
    pub context_alias: String,
    pub runtime_alias: String,
    pub agent_step: u64,
    pub attempt: u64,
    pub retry_of: Option<String>,
    pub state: String,
    pub stop_reason: Option<String>,
    pub certainty: Option<String>,
    pub tool_call_count: Option<u64>,
    pub usage_status: String,
    pub draft_exposed: bool,
    pub request_digest_alias: String,
    pub response_digest_alias: Option<String>,
    pub context_request_digest_matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedToolEvidence {
    pub alias: String,
    pub workstation_execution_alias: String,
    pub source_model_alias: String,
    pub provider_call_id: String,
    pub agent_step: u64,
    pub ordinal: u64,
    pub name: String,
    pub state: String,
    pub result_class: Option<String>,
    pub effective_privilege: Option<String>,
    pub timed_out: Option<bool>,
    pub cancelled: Option<bool>,
    pub cleanup_confirmed: Option<bool>,
    pub artifact_aliases: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedArtifactEvidence {
    pub alias: String,
    pub producer_tool_alias: Option<String>,
    pub digest_alias: String,
    pub storage_key_matches_digest: bool,
    pub retention_class: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedJournalEvidence {
    pub alias: String,
    pub stream_sequence: u64,
    pub event_type: String,
    pub cause: Option<String>,
    pub runtime_alias: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedRuntimeEvidence {
    pub alias: String,
    pub state: String,
    pub stop_reason: Option<String>,
    pub owned_work_count: u64,
    pub model_attempt_count: u64,
    pub tool_execution_count: u64,
    pub journal_event_types: Vec<String>,
    pub recovery: Vec<NormalizedRecoveryEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedRecoveryEvidence {
    pub stale_runtime_count: Option<u64>,
    pub queued_work_retained: Option<u64>,
    pub work_interrupted: Option<u64>,
    pub model_attempts_marked_unknown: Option<u64>,
    pub tool_attempts_marked_unknown: Option<u64>,
    pub cleanup_checks_performed: Option<u64>,
    pub cleanup_unconfirmed: Option<u64>,
    pub orphan_count: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedProviderDependency {
    pub phase: String,
    pub invocation_count: u64,
    pub answer_source: String,
    pub answer_context_verified: bool,
    pub tool_call_ids: Vec<String>,
    pub tool_execution_aliases: Vec<String>,
    pub prior_assistant_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedReconnectEvidence {
    pub replayed_durable_count: u64,
    pub every_replayed_cursor_after_saved: bool,
    pub no_pre_saved_event_replayed: bool,
    pub live_handoff_at_or_after_saved: bool,
    pub final_cursor_after_saved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedTelemetryChain {
    pub request_alias: String,
    pub command_kind: String,
    pub work_alias: String,
    pub model_alias: String,
    pub tool_alias: String,
    pub workstation_execution_alias: String,
    pub workstation_result_class: String,
    pub workstation_span_present: bool,
}

const REQUIRED_RELATIONSHIPS: [&str; 8] = [
    "retransmission_preserves_message_and_work_identity",
    "tool_results_pair_by_provider_call_and_tool_execution_identity",
    "tool_executions_link_to_source_model_and_workstation_execution",
    "assistant_commit_precedes_atomic_work_completion",
    "restart_preserves_product_and_conversation_identity",
    "follow_up_context_contains_durable_prior_history",
    "telemetry_links_request_command_work_model_tool_and_workstation",
    "replay_and_bootstrap_converge_to_the_same_durable_cursor",
];

const PORTABLE_HOST_REQUIREMENTS: [&str; 9] = [
    "real_production_local_workstation_bash_with_profiles_disabled_and_pipefail_enabled",
    "real_os",
    "real_architecture",
    "configured_working_directory",
    "installed_git_version",
    "workspace_file_read",
    "credential_canary_absent",
    "captured_output_metadata",
    "owned_process_cleanup",
];

const NORMALIZED_NONDETERMINISM: [&str; 10] = [
    "uuidv7_values",
    "request_ids",
    "runtime_ids",
    "process_ids",
    "ports",
    "timestamps",
    "durations",
    "temporary_roots",
    "artifact_locations",
    "machine_specific_values",
];

const PRESERVED_SEMANTICS: [&str; 8] = [
    "ordering",
    "counts",
    "result_classes",
    "identity_equalities",
    "causal_relationships",
    "artifact_integrity",
    "provider_context_dependencies",
    "live_and_replay_event_semantics",
];

pub fn validate_frozen_contract(
    contract: &Value,
    evidence: &NormalizedStage24Evidence,
) -> Result<(), String> {
    let encoded = serde_json::to_string(contract).map_err(|error| error.to_string())?;
    if encoded.contains("real_bash_lc") {
        return Err("frozen contract falsely claims Bash -lc".to_owned());
    }
    let expected_relationships = serde_json::to_value(REQUIRED_RELATIONSHIPS).unwrap();
    if contract["required_relationships"] != expected_relationships {
        return Err("frozen relationship names/content changed".to_owned());
    }
    if evidence.satisfied_relationships != REQUIRED_RELATIONSHIPS.map(str::to_owned).to_vec() {
        return Err("actual evidence does not satisfy every frozen relationship".to_owned());
    }
    let expected_portable = serde_json::to_value(PORTABLE_HOST_REQUIREMENTS).unwrap();
    if contract["portable_host"]["required"] != expected_portable {
        return Err("portable LocalWorkstation contract content changed".to_owned());
    }
    if contract["normalization"]["removed"]
        != serde_json::to_value(NORMALIZED_NONDETERMINISM).unwrap()
        || contract["normalization"]["preserved"]
            != serde_json::to_value(PRESERVED_SEMANTICS).unwrap()
    {
        return Err("frozen normalization contract content changed".to_owned());
    }
    let expected = &contract["expected"];
    let comparisons = [
        ("user_messages", json!(evidence.user_messages)),
        ("assistant_messages", json!(evidence.assistant_messages)),
        ("completed_work", json!(evidence.completed_work)),
        ("model_attempts", json!(evidence.model_attempts)),
        ("tool_executions", json!(evidence.tool_executions)),
        ("runtime_count", json!(evidence.runtime_count)),
        (
            "first_work_model_steps",
            serde_json::to_value(&evidence.first_work_model_steps).unwrap(),
        ),
        (
            "first_work_tools",
            serde_json::to_value(&evidence.first_work_tools).unwrap(),
        ),
        (
            "follow_up_model_steps",
            serde_json::to_value(&evidence.follow_up_model_steps).unwrap(),
        ),
        (
            "message_roles",
            serde_json::to_value(&evidence.message_roles).unwrap(),
        ),
        (
            "work_ordinals",
            serde_json::to_value(&evidence.work_ordinals).unwrap(),
        ),
    ];
    for (field, actual) in comparisons {
        if expected[field] != actual {
            return Err(format!("frozen expected value differs for {field}"));
        }
    }
    for (field, actual) in [
        (
            "works",
            serde_json::to_value(&evidence.work_results).unwrap(),
        ),
        (
            "models",
            serde_json::to_value(&evidence.model_results).unwrap(),
        ),
        (
            "tools",
            serde_json::to_value(&evidence.tool_results).unwrap(),
        ),
    ] {
        if contract["required_result_classes"][field] != actual {
            return Err(format!("frozen result classes differ for {field}"));
        }
    }
    if contract["contract_version"] != evidence.contract_version
        || contract["schema_version"] != evidence.schema_version
        || contract["protocol_version"] != evidence.protocol_version
        || contract["portable_host"]["result"] != evidence.portable_host_result
        || contract["ubuntu_target"]["result"] != evidence.ubuntu_target_result
    {
        return Err("frozen version/host result differs from actual evidence".to_owned());
    }
    if !evidence.retransmission_deduplicated
        || !evidence.provider_tool_values_verified
        || !evidence.durable_history_verified
        || !evidence.restart_identity_contract_verified
        || !evidence.persistence_and_artifact_integrity_verified
        || !evidence.operator_evidence_verified
        || evidence.provider_dependencies.len() != 2
        || evidence.provider_dependencies[0].phase != "first"
        || evidence.provider_dependencies[0].invocation_count != 2
        || evidence.provider_dependencies[0].answer_source != "model_visible_tool_results"
        || !evidence.provider_dependencies[0].answer_context_verified
        || evidence.provider_dependencies[0]
            .tool_execution_aliases
            .len()
            != 2
        || evidence.provider_dependencies[0].prior_assistant_required
        || evidence.provider_dependencies[1].phase != "second"
        || evidence.provider_dependencies[1].invocation_count != 1
        || evidence.provider_dependencies[1].answer_source
            != "durable_prior_assistant_and_tool_context"
        || !evidence.provider_dependencies[1].answer_context_verified
        || evidence.provider_dependencies[1]
            .tool_execution_aliases
            .len()
            != 2
        || !evidence.provider_dependencies[1].prior_assistant_required
    {
        return Err("required provenance and persistence semantics absent".to_owned());
    }
    if evidence.telemetry_chains.len() != 1
        || !evidence.telemetry_composition_and_redaction_verified
        || !evidence.reconnect.every_replayed_cursor_after_saved
        || !evidence.reconnect.no_pre_saved_event_replayed
        || !evidence.reconnect.live_handoff_at_or_after_saved
        || !evidence.reconnect.final_cursor_after_saved
    {
        return Err("required telemetry/reconnect semantic evidence absent".to_owned());
    }
    let chain = &evidence.telemetry_chains[0];
    let first_work = evidence
        .works
        .first()
        .ok_or_else(|| "first normalized Work absent".to_owned())?;
    let shell = first_work
        .tools
        .iter()
        .find(|tool| tool.name == "run_shell")
        .ok_or_else(|| "normalized shell execution absent".to_owned())?;
    if chain.request_alias.is_empty()
        || chain.command_kind.is_empty()
        || chain.work_alias != first_work.alias
        || chain.model_alias != shell.source_model_alias
        || chain.tool_alias != shell.alias
        || chain.workstation_execution_alias != shell.workstation_execution_alias
        || chain.workstation_result_class != "exited"
        || !chain.workstation_span_present
    {
        return Err("workstation telemetry correlation chain differs".to_owned());
    }
    Ok(())
}

struct HttpResponse {
    status: u16,
    body: Value,
}

struct Stage24Client {
    authority: String,
    bearer: String,
    conversation_id: String,
}

struct Stage24ScenarioGuard {
    root: PathBuf,
    child: Option<Child>,
    child_phase: Option<String>,
    cleanup_root: bool,
}

impl Stage24ScenarioGuard {
    fn new(root: PathBuf) -> Self {
        assert!(
            root.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with("craxii-stage18-stage24-"))
        );
        Self {
            root,
            child: None,
            child_phase: None,
            cleanup_root: true,
        }
    }

    fn spawn(&mut self, phase: &str) {
        assert!(self.child.is_none(), "Stage 24 child ownership overlapped");
        self.child = Some(spawn_child(&self.root, phase));
        self.child_phase = Some(phase.to_owned());
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("active Stage 24 child")
    }

    fn take_child(&mut self) -> Child {
        self.child_phase = None;
        self.child.take().expect("active Stage 24 child")
    }

    fn kill_active(&mut self) -> Output {
        kill_and_reap(self.take_child())
    }

    async fn wait_for_active_exit(&mut self) -> Output {
        let deadline = Instant::now() + FILE_WAIT;
        loop {
            match self.child_mut().try_wait().expect("poll Stage 24 child") {
                Some(_) => {
                    return self
                        .take_child()
                        .wait_with_output()
                        .expect("collect Stage 24 child output");
                }
                None if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                None => {
                    let _ = signal_owned_process_group(self.child_mut(), 9);
                    let output = self.take_child().wait_with_output().unwrap();
                    panic!(
                        "Stage 24 child did not exit: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
        }
    }

    fn finish(mut self) {
        assert!(
            self.child.is_none(),
            "Stage 24 child remained live at finish"
        );
        remove_stage24_root(&self.root);
        self.cleanup_root = false;
    }
}

impl Drop for Stage24ScenarioGuard {
    fn drop(&mut self) {
        if let Some(phase) = &self.child_phase {
            let path = self.root.join(format!("cleanup-{phase}.request"));
            if let Ok(file) = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(path)
            {
                let _ = file.sync_all();
            }
        }
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + FAILURE_CLEANUP_WAIT;
            let exited = loop {
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) | Err(_) => break false,
                }
            };
            if !exited {
                let _ = signal_owned_process_group(&child, 9);
            }
            let _ = child.wait();
        }
        if self.cleanup_root {
            remove_stage24_root(&self.root);
        }
    }
}

fn remove_stage24_root(root: &Path) {
    assert!(
        root.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("craxii-stage18-stage24-"))
    );
    match fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if std::thread::panicking() => {
            eprintln!("failed to remove owned Stage 24 root after panic: {error}");
        }
        Err(error) => panic!("failed to remove owned Stage 24 root: {error}"),
    }
}

impl Stage24Client {
    fn new(ready: &ReadyRecord) -> Self {
        Self {
            authority: ready.authority.clone(),
            bearer: ready.bearer.clone(),
            conversation_id: ready.conversation_id.clone(),
        }
    }

    async fn submit(&self, text: &str, client_message_id: &str) -> HttpResponse {
        let body = serde_json::to_vec(&json!({
            "protocol_version": 1,
            "client_message_id": client_message_id,
            "content": [{"type": "text", "text": text}],
        }))
        .unwrap();
        self.http(
            "POST",
            &format!("/v1/conversations/{}/messages", self.conversation_id),
            Some(&body),
            Some(client_message_id),
        )
        .await
    }

    async fn bootstrap(&self) -> HttpResponse {
        self.http("GET", "/v1/bootstrap", None, None).await
    }

    async fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        idempotency_key: Option<&str>,
    ) -> HttpResponse {
        let mut stream = TcpStream::connect(&self.authority)
            .await
            .expect("connect Stage 24 HTTP client");
        let body = body.unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n",
            self.authority, self.bearer
        );
        if let Some(value) = idempotency_key {
            request.push_str(&format!("Idempotency-Key: {value}\r\n"));
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
        parse_http_response(&bytes)
    }

    async fn websocket(&self, after: u64) -> Socket {
        let url = format!("ws://{}/v1/events?after={after}", self.authority);
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.bearer).parse().unwrap(),
        );
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
}

fn parse_http_response(bytes: &[u8]) -> HttpResponse {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
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
    HttpResponse {
        status,
        body: serde_json::from_slice(&bytes[split + 4..]).expect("JSON HTTP response"),
    }
}

pub async fn run_canonical_scenario(label: &str) -> NormalizedStage24Evidence {
    let root_path = Stage18Root::new(&format!("stage24-{label}")).preserve();
    let root = Stage18Root::from_existing(root_path.clone());
    fs::write(root.workspace().join("machine-note.txt"), FIXTURE_CONTENT)
        .expect("write Stage 24 workspace fixture");
    fs::create_dir(root.path().join("credentials")).expect("create credential config directory");
    fs::set_permissions(
        root.path().join("credentials"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let configuration_path = write_configuration(&root);
    let expected = MachineFacts::capture(&root.workspace());
    let canonical_workspace = root.workspace().canonicalize().unwrap();
    assert_eq!(PathBuf::from(&expected.cwd), canonical_workspace);
    assert!(expected.git_version.starts_with("git version "));
    drop(root);

    let mut lifecycle = Stage24ScenarioGuard::new(root_path.clone());
    lifecycle.spawn("first");
    let first_ready: ReadyRecord =
        wait_json(&root_path.join("ready-first.json"), lifecycle.child_mut()).await;
    assert_eq!(first_ready.configuration_version, 1);
    assert_eq!(first_ready.protocol_version, 1);
    assert_eq!(
        PathBuf::from(&first_ready.workspace_root),
        canonical_workspace
    );
    let first_client = Stage24Client::new(&first_ready);
    let mut first_socket = first_client.websocket(0).await;
    let initial_sync = through_sync(&mut first_socket).await;
    assert!(
        initial_sync
            .iter()
            .all(|frame| frame["event_type"] == "sync.complete")
    );

    let accepted = first_client
        .submit(CANONICAL_REQUEST, FIRST_CLIENT_ID)
        .await;
    assert_eq!(accepted.status, 202);
    assert_eq!(accepted.body["protocol_version"], 1);
    assert_eq!(accepted.body["duplicate"], false);
    let first_message_id = accepted.body["message_id"].as_str().unwrap().to_owned();
    let first_work_id = accepted.body["work_id"].as_str().unwrap().to_owned();
    let first_frames = through_terminal_work(&mut first_socket, &first_work_id).await;
    let expected_first_answer = expected_first_answer(&expected);
    assert_live_contract(&first_frames, &first_work_id, &expected_first_answer, true);

    let duplicate = first_client
        .submit(CANONICAL_REQUEST, FIRST_CLIENT_ID)
        .await;
    assert_eq!(duplicate.status, 202);
    assert_eq!(duplicate.body["duplicate"], true);
    assert_eq!(duplicate.body["message_id"], first_message_id);
    assert_eq!(duplicate.body["work_id"], first_work_id);
    let retransmission_deduplicated = duplicate.body["duplicate"] == true
        && duplicate.body["message_id"] == first_message_id
        && duplicate.body["work_id"] == first_work_id;

    let first_bootstrap = first_client.bootstrap().await;
    assert_eq!(first_bootstrap.status, 200);
    assert_bootstrap(
        &first_bootstrap.body,
        1,
        1,
        &[CANONICAL_REQUEST, expected_first_answer.as_str()],
    );
    let saved_cursor = first_bootstrap.body["snapshot_cursor"].as_u64().unwrap();
    assert!(first_frames.iter().any(|frame| {
        frame["event_type"] == "work.completed"
            && frame["cursor"]
                .as_u64()
                .is_some_and(|value| value <= saved_cursor)
    }));
    let pre_saved_event_ids: BTreeSet<String> = first_frames
        .iter()
        .filter(|frame| {
            frame["delivery_kind"] == "durable"
                && frame["cursor"]
                    .as_u64()
                    .is_some_and(|cursor| cursor <= saved_cursor)
        })
        .map(|frame| frame["event_id"].as_str().unwrap().to_owned())
        .collect();
    first_socket.close(None).await.unwrap();

    request_snapshot(&root_path, "first");
    wait_path(
        &root_path.join("snapshot-first.ready"),
        lifecycle.child_mut(),
    )
    .await;
    let first_proof: ProviderProof = read_json(&root_path.join("provider-proof-first.json"));
    assert_eq!(first_proof.invocation_count, 2);
    assert!(first_proof.canonical_prompt_seen);
    assert!(first_proof.durable_tool_values_verified);
    assert!(first_proof.credential_canary_absent);
    assert!(first_proof.fixture_content_verified);
    assert_eq!(first_proof.tools.len(), 2);
    let first_telemetry = fs::read_to_string(root_path.join("telemetry-first.jsonl")).unwrap();
    let first_output = lifecycle.kill_active();
    assert_eq!(
        first_output.status.signal(),
        Some(9),
        "child was not SIGKILLed"
    );

    lifecycle.spawn("second");
    let second_ready: ReadyRecord =
        wait_json(&root_path.join("ready-second.json"), lifecycle.child_mut()).await;
    assert_ne!(first_ready.runtime_id, second_ready.runtime_id);
    assert_eq!(first_ready.craxii_id, second_ready.craxii_id);
    assert_eq!(first_ready.conversation_id, second_ready.conversation_id);
    assert_eq!(first_ready.workstation_id, second_ready.workstation_id);
    assert_eq!(first_ready.workspace_id, second_ready.workspace_id);
    assert_eq!(first_ready.workspace_root, second_ready.workspace_root);

    let second_client = Stage24Client::new(&second_ready);
    let mut second_socket = second_client.websocket(saved_cursor).await;
    let reconnect_frames = through_sync(&mut second_socket).await;
    assert_reconnect_cursor_contract(&reconnect_frames, saved_cursor, &pre_saved_event_ids);
    assert!(reconnect_frames.iter().all(|frame| {
        !frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
    }));
    assert!(
        reconnect_frames.last().unwrap()["through_cursor"]
            .as_u64()
            .is_some_and(|cursor| cursor >= saved_cursor)
    );
    let recovered_bootstrap = second_client.bootstrap().await;
    assert_eq!(recovered_bootstrap.status, 200);
    assert_bootstrap(
        &recovered_bootstrap.body,
        1,
        1,
        &[CANONICAL_REQUEST, expected_first_answer.as_str()],
    );
    assert_eq!(
        recovered_bootstrap.body["craxii"]["craxii_id"],
        first_ready.craxii_id
    );
    assert_eq!(
        recovered_bootstrap.body["primary_conversation"]["conversation_id"],
        first_ready.conversation_id
    );

    let follow = second_client.submit(FOLLOW_UP, FOLLOW_CLIENT_ID).await;
    assert_eq!(follow.status, 202);
    assert_eq!(follow.body["duplicate"], false);
    let follow_work_id = follow.body["work_id"].as_str().unwrap().to_owned();
    assert_ne!(first_work_id, follow_work_id);
    let follow_answer = format!("You have {}.", expected.git_version);
    let follow_frames = through_terminal_work(&mut second_socket, &follow_work_id).await;
    assert_live_contract(&follow_frames, &follow_work_id, &follow_answer, false);
    second_socket.close(None).await.unwrap();

    let final_bootstrap = second_client.bootstrap().await;
    assert_eq!(final_bootstrap.status, 200);
    assert_bootstrap(
        &final_bootstrap.body,
        2,
        2,
        &[
            CANONICAL_REQUEST,
            expected_first_answer.as_str(),
            FOLLOW_UP,
            follow_answer.as_str(),
        ],
    );
    let final_cursor = final_bootstrap.body["snapshot_cursor"].as_u64().unwrap();
    assert!(final_cursor > saved_cursor);

    let mut replay_socket = second_client.websocket(0).await;
    let replay_frames = through_sync(&mut replay_socket).await;
    replay_socket.close(None).await.unwrap();
    assert_replay_contract(
        &replay_frames,
        final_cursor,
        &first_work_id,
        &follow_work_id,
    );

    request_snapshot(&root_path, "second");
    wait_path(
        &root_path.join("snapshot-second.ready"),
        lifecycle.child_mut(),
    )
    .await;
    let second_proof: ProviderProof = read_json(&root_path.join("provider-proof-second.json"));
    assert_eq!(second_proof.invocation_count, 1);
    assert!(second_proof.canonical_prompt_seen);
    assert!(second_proof.follow_up_seen);
    assert!(second_proof.durable_prior_assistant_seen);
    assert!(second_proof.durable_tool_values_verified);
    assert_eq!(second_proof.facts.git_version, expected.git_version);
    let second_telemetry = fs::read_to_string(root_path.join("telemetry-second.jsonl")).unwrap();
    touch(&root_path.join("stop-second.request"));
    let second_output = lifecycle.wait_for_active_exit().await;
    assert!(
        second_output.status.success(),
        "second child failed: {}",
        String::from_utf8_lossy(&second_output.stderr)
    );

    let operator = run_operator_evidence(
        &root_path,
        &configuration_path,
        &first_ready,
        &second_ready,
        &first_work_id,
        &follow_work_id,
    );
    let _persisted = assert_persistence_contract(
        &root_path,
        &operator.export,
        &final_bootstrap.body,
        &first_ready,
        &second_ready,
        &first_proof,
        &expected,
        &first_work_id,
        &follow_work_id,
    )
    .await;
    let telemetry_chains = assert_telemetry_contract(
        &[first_telemetry.as_str(), second_telemetry.as_str()].concat(),
        &operator.export,
        &first_ready.bearer,
        &second_ready.bearer,
        &first_work_id,
        &follow_work_id,
        &first_proof,
        &expected,
    );

    let normalized = normalize_stage24_evidence(
        &operator.export,
        &final_bootstrap.body,
        &first_ready,
        &second_ready,
        &first_proof,
        &second_proof,
        &first_frames,
        &reconnect_frames,
        &replay_frames,
        saved_cursor,
        final_cursor,
        &pre_saved_event_ids,
        retransmission_deduplicated,
        telemetry_chains,
    )
    .expect("normalize actual Stage 24 causal evidence");
    lifecycle.finish();
    normalized
}

fn spawn_child(root: &Path, phase: &str) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "stage24_runtime_child", "--nocapture"])
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_PHASE_ENV, phase)
        .env(CREDENTIAL_CANARY_NAME, CREDENTIAL_CANARY_VALUE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    command
        .spawn()
        .expect("spawn Stage 24 runtime child in its owned process group")
}

fn signal_owned_process_group(child: &Child, signal: i32) -> std::io::Result<()> {
    let process_group = i32::try_from(child.id()).unwrap();
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: spawn_child makes the child's PID the ID of a dedicated process group. A negative
    // target therefore reaches only the Stage 24 runtime and descendants that it owns.
    let result = unsafe { kill(-process_group, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn kill_and_reap(child: Child) -> Output {
    signal_owned_process_group(&child, 9).expect("kill owned Stage 24 process group");
    child
        .wait_with_output()
        .expect("reap killed Stage 24 child")
}

async fn wait_path(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + FILE_WAIT;
    loop {
        if path.is_file() {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll Stage 24 child") {
            panic!("Stage 24 child exited before {}: {status}", path.display());
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_json<T: for<'de> Deserialize<'de>>(path: &Path, child: &mut Child) -> T {
    wait_path(path, child).await;
    read_json(path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    serde_json::from_slice(&fs::read(path).unwrap()).expect("valid Stage 24 coordination JSON")
}

fn write_json(path: &Path, value: &impl Serialize) {
    let temporary = path.with_extension("partial");
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    fs::rename(temporary, path).unwrap();
}

fn touch(path: &Path) {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .sync_all()
        .unwrap();
}

fn request_snapshot(root: &Path, phase: &str) {
    touch(&root.join(format!("snapshot-{phase}.request")));
}

fn write_configuration(root: &Stage18Root) -> PathBuf {
    let state_root = root.state_root();
    let artifact_root = root.artifact_root();
    let workspace_root = root.workspace();
    let state = state_root.to_str().unwrap();
    let artifacts = artifact_root.to_str().unwrap();
    let workspace = workspace_root.to_str().unwrap();
    let credentials = root.path().join("credentials");
    let input = include_str!("../fixtures/config/valid/local.toml")
        .replace("/tmp/craxii-dev/state/artifacts", artifacts)
        .replace("/tmp/craxii-dev/workspaces/primary", workspace)
        .replace("/tmp/craxii-dev/credentials", credentials.to_str().unwrap())
        .replace("/tmp/craxii-dev/state", state)
        .replace("format = \"pretty\"", "format = \"json\"");
    let path = root.path().join("stage24.toml");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(input.as_bytes()).unwrap();
    file.sync_all().unwrap();
    path
}

async fn next_json(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(20), socket.next())
            .await
            .expect("Stage 24 WebSocket frame timeout")
            .expect("Stage 24 WebSocket closed")
            .expect("Stage 24 WebSocket frame");
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                return serde_json::from_str(&text).unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Pong(bytes))
                    .await
                    .unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Pong(_) => {}
            other => panic!("unexpected Stage 24 WebSocket frame: {other:?}"),
        }
    }
}

async fn through_sync(socket: &mut Socket) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_json(socket).await;
        let complete = frame["event_type"] == "sync.complete";
        frames.push(frame);
        if complete {
            return frames;
        }
    }
}

async fn through_terminal_work(socket: &mut Socket, work_id: &str) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_json(socket).await;
        let terminal = frame["event_type"] == "work.completed" && frame["work_id"] == work_id;
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
}

fn assert_live_contract(frames: &[Value], work_id: &str, final_answer: &str, tools: bool) {
    assert_durable_cursor_contract(frames);
    for frame in frames {
        if frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
        {
            assert_eq!(frame["delivery_kind"], "ephemeral");
            assert!(frame["cursor"].is_null());
            assert_eq!(frame["work_id"], work_id);
        }
    }
    assert!(
        frames
            .iter()
            .any(|frame| frame["event_type"] == "assistant.draft_started")
    );
    assert!(frames.iter().any(|frame| {
        frame["event_type"] == "assistant.draft_delta" && frame["payload"]["text"] == final_answer
    }));
    assert!(frames.iter().any(|frame| {
        frame["event_type"] == "assistant.message_committed"
            && frame["work_id"] == work_id
            && frame["payload"]["content"][0]["text"] == final_answer
    }));
    assert!(
        frames.iter().any(|frame| {
            frame["event_type"] == "work.completed" && frame["work_id"] == work_id
        })
    );
    let started = frames
        .iter()
        .filter(|frame| frame["event_type"] == "tool.execution_started")
        .count();
    let finished = frames
        .iter()
        .filter(|frame| frame["event_type"] == "tool.execution_finished")
        .count();
    assert_eq!((started, finished), if tools { (2, 2) } else { (0, 0) });
    let encoded = serde_json::to_string(frames).unwrap();
    assert!(!encoded.contains(FIXTURE_CONTENT.trim()));
    assert!(!encoded.contains("credential_canary=absent"));
}

fn assert_durable_cursor_contract(frames: &[Value]) {
    let durable: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame["delivery_kind"] == "durable")
        .collect();
    let cursors: Vec<u64> = durable
        .iter()
        .map(|frame| frame["cursor"].as_u64().unwrap())
        .collect();
    assert!(cursors.windows(2).all(|pair| pair[0] < pair[1]));
    let event_ids: BTreeSet<&str> = durable
        .iter()
        .map(|frame| frame["event_id"].as_str().unwrap())
        .collect();
    assert_eq!(event_ids.len(), durable.len());
}

fn assert_reconnect_cursor_contract(
    frames: &[Value],
    saved_cursor: u64,
    pre_saved_event_ids: &BTreeSet<String>,
) {
    let replayed: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame["delivery_kind"] == "durable")
        .collect();
    assert!(
        replayed.iter().all(|frame| {
            frame["cursor"]
                .as_u64()
                .is_some_and(|cursor| cursor > saved_cursor)
        }),
        "restart replay included a durable cursor at or before the saved cursor"
    );
    assert!(
        replayed.iter().all(|frame| {
            frame["event_id"]
                .as_str()
                .is_some_and(|event_id| !pre_saved_event_ids.contains(event_id))
        }),
        "restart replay repeated a pre-saved durable event"
    );
    let through = frames
        .last()
        .and_then(|frame| frame["through_cursor"].as_u64())
        .expect("restart replay sync cursor");
    assert!(through >= saved_cursor);
    assert!(replayed.iter().all(|frame| {
        frame["cursor"]
            .as_u64()
            .is_some_and(|cursor| cursor <= through)
    }));
}

fn assert_replay_contract(
    frames: &[Value],
    final_cursor: u64,
    first_work: &str,
    second_work: &str,
) {
    assert_eq!(frames.last().unwrap()["event_type"], "sync.complete");
    assert_eq!(frames.last().unwrap()["through_cursor"], final_cursor);
    assert!(frames.iter().all(|frame| {
        !frame["event_type"]
            .as_str()
            .unwrap_or_default()
            .starts_with("assistant.draft_")
    }));
    assert_durable_cursor_contract(frames);
    for work in [first_work, second_work] {
        assert!(frames.iter().any(|frame| {
            frame["event_type"] == "assistant.message_committed" && frame["work_id"] == work
        }));
        assert!(
            frames.iter().any(|frame| {
                frame["event_type"] == "work.completed" && frame["work_id"] == work
            })
        );
    }
}

fn assert_bootstrap(body: &Value, users: usize, assistants: usize, expected_texts: &[&str]) {
    assert_eq!(body["protocol_version"], 1);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["role"] == "user")
            .count(),
        users
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["role"] == "assistant")
            .count(),
        assistants
    );
    assert_eq!(body["work_items"].as_array().unwrap().len(), users);
    assert!(
        body["work_items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| { work["state"] == "completed" && work["terminal_reason"] == "answered" })
    );
    let text: Vec<&str> = messages
        .iter()
        .map(|message| message["content"][0]["text"].as_str().unwrap())
        .collect();
    assert_eq!(text, expected_texts);
}

fn event_types(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| frame["event_type"].as_str().unwrap().to_owned())
        .collect()
}

pub fn run_runtime_child_from_environment() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV) else {
        return;
    };
    let phase = std::env::var(CHILD_PHASE_ENV).expect("Stage 24 child phase");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(run_runtime_child(PathBuf::from(root), phase));
}

async fn run_runtime_child(root_path: PathBuf, phase: String) {
    let configuration_path = root_path.join("stage24.toml");
    let configuration = config::load(&configuration_path).expect("load Stage 24 startup config");
    assert_eq!(configuration.configuration_version(), 1);
    assert_eq!(configuration.paths().state_root(), root_path.join("state"));
    assert_eq!(
        configuration.paths().primary_workspace_root(),
        root_path.join("workspace")
    );
    let (dispatch, capture) = telemetry::production_test_dispatch(configuration.tracing());
    tracing::dispatcher::set_global_default(dispatch)
        .expect("Stage 24 child owns its process-global telemetry subscriber");

    let root = Stage18Root::from_existing(root_path.clone());
    let facts = MachineFacts::capture(&root.workspace());
    let gate = Stage24AnswerGate::new();
    let provider = Arc::new(match phase.as_str() {
        "first" => ContextAnsweringProvider::first_work(gate.clone()),
        "second" => ContextAnsweringProvider::follow_up(gate.clone()),
        other => panic!("unknown Stage 24 child phase {other}"),
    });
    let harness = Stage18Harness::start_with_provider(
        root,
        provider.clone() as Arc<dyn ModelProvider>,
        EstimatorMode::Normal,
    )
    .await
    .expect("start Stage 24 production composition");
    assert!(harness.health.snapshot().is_ready());
    let ready = ReadyRecord {
        configuration_version: configuration.configuration_version(),
        protocol_version: 1,
        authority: harness.authority.clone(),
        bearer: harness.bearer.clone(),
        runtime_id: harness.runtime_id.to_string(),
        craxii_id: harness.identity.craxii_id.to_string(),
        conversation_id: harness.identity.conversation_id.to_string(),
        workstation_id: harness.identity.workstation_id.to_string(),
        workspace_id: harness.identity.workspace_id.to_string(),
        workspace_root: harness
            .root
            .workspace()
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
    };
    write_json(&root_path.join(format!("ready-{phase}.json")), &ready);

    let expected_invocations = if phase == "first" { 2 } else { 1 };
    let cleanup_request = root_path.join(format!("cleanup-{phase}.request"));
    if wait_provider_or_cleanup(provider.as_ref(), expected_invocations, &cleanup_request).await {
        gate.release();
        shutdown_for_failure_cleanup(harness).await;
        return;
    }
    let proof = if phase == "first" {
        prove_first_provider_request(provider.as_ref(), &facts)
    } else {
        prove_follow_up_provider_request(provider.as_ref(), &facts)
    };
    write_json(
        &root_path.join(format!("provider-proof-{phase}.json")),
        &proof,
    );
    // The scripted final answer is unavailable until the captured assembled request proves that
    // real, correlated tool results (or the durable prior history) reached the provider boundary.
    gate.release();

    let snapshot_request = root_path.join(format!("snapshot-{phase}.request"));
    if wait_for_file_or_cleanup(&snapshot_request, &cleanup_request).await {
        shutdown_for_failure_cleanup(harness).await;
        return;
    }
    assert_eq!(provider.invocation_count(), expected_invocations);
    harness
        .store
        .verify_application_consistency()
        .await
        .expect("live Stage 24 application consistency");
    fs::write(
        root_path.join(format!("telemetry-{phase}.jsonl")),
        capture.output(),
    )
    .unwrap();
    touch(&root_path.join(format!("snapshot-{phase}.ready")));

    if phase == "first" {
        wait_for_file_in_child(&cleanup_request).await;
        shutdown_for_failure_cleanup(harness).await;
        return;
    }
    let stop_request = root_path.join("stop-second.request");
    let _cleanup_requested = wait_for_file_or_cleanup(&stop_request, &cleanup_request).await;
    let root = harness.shutdown().await;
    let _ = root.preserve();
}

async fn shutdown_for_failure_cleanup(harness: Stage18Harness) {
    let root = harness.shutdown().await;
    let _ = root.preserve();
}

async fn wait_provider_or_cleanup(
    provider: &ContextAnsweringProvider,
    expected: u64,
    cleanup_request: &Path,
) -> bool {
    let deadline = Instant::now() + FILE_WAIT;
    loop {
        if cleanup_request.is_file() {
            return true;
        }
        if provider.invocation_count() == expected {
            return false;
        }
        assert!(
            provider.invocation_count() < expected,
            "unexpected extra Stage 24 model invocation"
        );
        assert!(
            Instant::now() < deadline,
            "Stage 24 provider was not invoked"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_file_or_cleanup(path: &Path, cleanup_request: &Path) -> bool {
    let deadline = Instant::now() + FILE_WAIT;
    loop {
        if cleanup_request.is_file() {
            return true;
        }
        if path.is_file() {
            return false;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_file_in_child(path: &Path) {
    let deadline = Instant::now() + FILE_WAIT;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn prove_first_provider_request(
    provider: &ContextAnsweringProvider,
    facts: &MachineFacts,
) -> ProviderProof {
    let captures = provider.captures();
    assert_eq!(captures.len(), 2);
    let canonical_prompt_seen = request_has_message(
        captures[0].request.ordered_input_items(),
        ModelInputRole::User,
        CANONICAL_REQUEST,
    );
    assert!(canonical_prompt_seen);
    let items = captures[1].request.ordered_input_items();
    let derived = derive_first_answer(items).expect("derive first answer from provider context");
    let observed = &derived.facts;
    assert_eq!(observed.os, facts.os);
    assert_eq!(observed.architecture, facts.architecture);
    assert_eq!(PathBuf::from(&observed.cwd), PathBuf::from(&facts.cwd));
    assert_eq!(observed.git_version, facts.git_version);
    assert_eq!(derived.file_text, FIXTURE_CONTENT);
    assert_eq!(
        captures[1].answer.as_deref(),
        Some(derived.answer().as_str())
    );
    assert_eq!(
        captures[1].answer_source,
        Some("model_visible_tool_results")
    );
    ProviderProof {
        phase: "first".to_owned(),
        invocation_count: 2,
        canonical_prompt_seen,
        follow_up_seen: false,
        durable_prior_assistant_seen: false,
        durable_tool_values_verified: true,
        credential_canary_absent: observed.credential_canary_absent,
        fixture_content_verified: true,
        answer_source: captures[1].answer_source.unwrap().to_owned(),
        answer_context_verified: true,
        facts: ObservedFacts {
            os: observed.os.clone(),
            architecture: observed.architecture.clone(),
            cwd: observed.cwd.clone(),
            git_version: observed.git_version.clone(),
        },
        tools: derived.tools,
    }
}

fn prove_follow_up_provider_request(
    provider: &ContextAnsweringProvider,
    facts: &MachineFacts,
) -> ProviderProof {
    let captures = provider.captures();
    assert_eq!(captures.len(), 1);
    let items = captures[0].request.ordered_input_items();
    let canonical_prompt_seen = request_has_message(items, ModelInputRole::User, CANONICAL_REQUEST);
    let follow_up_seen = request_has_message(items, ModelInputRole::User, FOLLOW_UP);
    let durable_prior_assistant_seen = items.iter().any(|item| {
        matches!(item, ModelInputItem::PriorAssistant { content_parts }
            if content_parts.len() == 1 && parse_first_answer(content_parts[0].as_str()).is_ok())
    });
    assert!(canonical_prompt_seen && follow_up_seen && durable_prior_assistant_seen);
    let derived = derive_follow_up_answer(items).expect("derive follow-up from durable context");
    assert_eq!(derived.git_version, facts.git_version);
    assert_eq!(derived.facts.os, facts.os);
    assert_eq!(derived.facts.architecture, facts.architecture);
    assert_eq!(PathBuf::from(&derived.facts.cwd), PathBuf::from(&facts.cwd));
    assert_eq!(derived.file_text, FIXTURE_CONTENT);
    let expected_answer = format!("You have {}.", derived.git_version);
    assert_eq!(
        captures[0].answer.as_deref(),
        Some(expected_answer.as_str())
    );
    assert_eq!(
        captures[0].answer_source,
        Some("durable_prior_assistant_and_tool_context")
    );
    ProviderProof {
        phase: "second".to_owned(),
        invocation_count: 1,
        canonical_prompt_seen,
        follow_up_seen,
        durable_prior_assistant_seen,
        durable_tool_values_verified: true,
        credential_canary_absent: derived.facts.credential_canary_absent,
        fixture_content_verified: derived.file_text == FIXTURE_CONTENT,
        answer_source: captures[0].answer_source.unwrap().to_owned(),
        answer_context_verified: true,
        facts: ObservedFacts {
            os: derived.facts.os,
            architecture: derived.facts.architecture,
            cwd: derived.facts.cwd,
            git_version: derived.git_version,
        },
        tools: derived.tools,
    }
}

fn request_has_message(items: &[ModelInputItem], role: ModelInputRole, expected: &str) -> bool {
    items.iter().any(|item| {
        matches!(item, ModelInputItem::Message { role: actual, content_parts }
            if *actual == role && content_parts.iter().any(|part| part.as_str() == expected))
    })
}

#[derive(Clone)]
struct CorrelatedToolProjection {
    item_index: usize,
    name: String,
    result: Value,
    proof: ToolResultProof,
}

#[derive(Clone)]
struct DerivedFirstAnswer {
    facts: ParsedShellFacts,
    file_text: String,
    file_sha256: String,
    tools: Vec<ToolResultProof>,
}

impl DerivedFirstAnswer {
    fn answer(&self) -> String {
        format!(
            "OS: {}\nCPU architecture: {}\nCurrent directory: {}\nGit version: {}\nWorkspace file SHA-256: {}",
            self.facts.os,
            self.facts.architecture,
            self.facts.cwd,
            self.facts.git_version,
            self.file_sha256
        )
    }
}

struct DerivedFollowUpAnswer {
    git_version: String,
    facts: ParsedShellFacts,
    file_text: String,
    tools: Vec<ToolResultProof>,
}

fn derive_first_answer(items: &[ModelInputItem]) -> Result<DerivedFirstAnswer, String> {
    if !request_has_message(items, ModelInputRole::User, CANONICAL_REQUEST) {
        return Err("canonical user request absent".to_owned());
    }
    let tools = correlated_tool_results(items)?;
    if tools.len() != 2 {
        return Err("canonical work requires exactly two correlated tool results".to_owned());
    }
    let shell = tools
        .iter()
        .find(|tool| tool.name == "run_shell" && tool.proof.call_id == SHELL_CALL_ID)
        .ok_or_else(|| "correlated run_shell result absent".to_owned())?;
    let read = tools
        .iter()
        .find(|tool| tool.name == "read_file" && tool.proof.call_id == READ_CALL_ID)
        .ok_or_else(|| "correlated read_file result absent".to_owned())?;
    let facts = parse_shell_facts_checked(&shell.result)?;
    if !facts.credential_canary_absent {
        return Err("credential canary reached the model-visible shell result".to_owned());
    }
    let file_text = field_projection_checked(&read.result, "text")?;
    let file_sha256 = Sha256Digest::hash_bytes(file_text.as_bytes()).to_string();
    Ok(DerivedFirstAnswer {
        facts,
        file_text,
        file_sha256,
        tools: tools.into_iter().map(|tool| tool.proof).collect(),
    })
}

fn derive_follow_up_answer(items: &[ModelInputItem]) -> Result<DerivedFollowUpAnswer, String> {
    let current_trigger = items
        .iter()
        .position(|item| {
            matches!(item, ModelInputItem::Message { role: ModelInputRole::User, content_parts }
                if content_parts.len() == 1 && content_parts[0].as_str() == FOLLOW_UP)
        })
        .ok_or_else(|| "follow-up trigger absent".to_owned())?;
    let canonical_trigger = items
        .iter()
        .position(|item| {
            matches!(item, ModelInputItem::Message { role: ModelInputRole::User, content_parts }
                if content_parts.len() == 1 && content_parts[0].as_str() == CANONICAL_REQUEST)
        })
        .ok_or_else(|| "durable canonical trigger absent".to_owned())?;
    let prior_assistants: Vec<(usize, &str)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            ModelInputItem::PriorAssistant { content_parts } if content_parts.len() == 1 => {
                Some((index, content_parts[0].as_str()))
            }
            _ => None,
        })
        .collect();
    if prior_assistants.len() != 1 {
        return Err("exact durable prior assistant context absent".to_owned());
    }
    let (assistant_index, assistant_text) = prior_assistants[0];
    let prior_answer = parse_first_answer(assistant_text)?;
    let tool_answer = derive_first_answer(items)?;
    let first_tool_index = correlated_tool_results(items)?
        .into_iter()
        .map(|tool| tool.item_index)
        .min()
        .ok_or_else(|| "durable tool context absent".to_owned())?;
    if !(canonical_trigger < first_tool_index
        && first_tool_index < assistant_index
        && assistant_index < current_trigger)
    {
        return Err("durable context order is not canonical".to_owned());
    }
    if prior_answer.facts.os != tool_answer.facts.os
        || prior_answer.facts.architecture != tool_answer.facts.architecture
        || prior_answer.facts.cwd != tool_answer.facts.cwd
        || prior_answer.facts.git_version != tool_answer.facts.git_version
        || prior_answer.file_sha256 != tool_answer.file_sha256
    {
        return Err("durable prior assistant disagrees with correlated tool context".to_owned());
    }
    Ok(DerivedFollowUpAnswer {
        git_version: prior_answer.facts.git_version,
        facts: tool_answer.facts,
        file_text: tool_answer.file_text,
        tools: tool_answer.tools,
    })
}

fn parse_first_answer(value: &str) -> Result<DerivedFirstAnswer, String> {
    let mut lines = value.lines();
    let os = answer_line(&mut lines, "OS: ")?;
    let architecture = answer_line(&mut lines, "CPU architecture: ")?;
    let cwd = answer_line(&mut lines, "Current directory: ")?;
    let git_version = answer_line(&mut lines, "Git version: ")?;
    let file_sha256 = answer_line(&mut lines, "Workspace file SHA-256: ")?;
    if file_sha256.len() != 64 || !file_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("prior assistant file digest is invalid".to_owned());
    }
    if lines.next().is_some() {
        return Err("prior assistant answer has unexpected extra content".to_owned());
    }
    Ok(DerivedFirstAnswer {
        facts: ParsedShellFacts {
            os,
            architecture,
            cwd,
            git_version,
            credential_canary_absent: true,
        },
        file_text: String::new(),
        file_sha256,
        tools: Vec::new(),
    })
}

fn answer_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    prefix: &str,
) -> Result<String, String> {
    lines
        .next()
        .and_then(|line| line.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("prior assistant answer missing {prefix}"))
}

fn correlated_tool_results(
    items: &[ModelInputItem],
) -> Result<Vec<CorrelatedToolProjection>, String> {
    let mut projections = Vec::new();
    let mut result_count = 0_usize;
    let mut call_ids = BTreeSet::new();
    let mut execution_ids = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        if matches!(item, ModelInputItem::ToolResult { .. }) {
            result_count += 1;
        }
        let ModelInputItem::ToolCall(call) = item else {
            continue;
        };
        if !call_ids.insert(call.call_id().as_str().to_owned()) {
            return Err("duplicate model-visible tool call identity".to_owned());
        }
        let Some(ModelInputItem::ToolResult { call_id, result }) = items.get(index + 1) else {
            return Err("tool call is not immediately paired with its result".to_owned());
        };
        if call_id != call.call_id() {
            return Err("tool call/result canonical identities differ".to_owned());
        }
        let provider_call_id = result["provider_tool_call_id"]
            .as_str()
            .ok_or_else(|| "tool result provider call identity absent".to_owned())?;
        let tool_execution_id = result["tool_execution_id"]
            .as_str()
            .ok_or_else(|| "tool execution identity absent".to_owned())?;
        let tool_name = result["tool_name"]
            .as_str()
            .ok_or_else(|| "tool result name absent".to_owned())?;
        let result_kind = result["result"]["result_kind"]
            .as_str()
            .ok_or_else(|| "tool result class absent".to_owned())?;
        if provider_call_id != call.call_id().as_str()
            || tool_name != call.name().as_str()
            || result_kind != "success"
            || uuid::Uuid::parse_str(tool_execution_id).is_err()
            || !execution_ids.insert(tool_execution_id.to_owned())
        {
            return Err("tool result correlation/evidence is invalid".to_owned());
        }
        projections.push(CorrelatedToolProjection {
            item_index: index,
            name: tool_name.to_owned(),
            result: result.clone(),
            proof: ToolResultProof {
                call_id: call_id.as_str().to_owned(),
                tool_execution_id: tool_execution_id.to_owned(),
                provider_call_id: provider_call_id.to_owned(),
                result_kind: result_kind.to_owned(),
            },
        });
    }
    if result_count != projections.len() {
        return Err("orphan or duplicate model-visible tool result".to_owned());
    }
    Ok(projections)
}

fn tool_projection(items: &[ModelInputItem], expected_call: &str) -> (Value, ToolResultProof) {
    let (call_id, result) = items
        .iter()
        .find_map(|item| match item {
            ModelInputItem::ToolResult { call_id, result } if call_id.as_str() == expected_call => {
                Some((call_id, result))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing correlated tool result {expected_call}"));
    let provider_call_id = result["provider_tool_call_id"].as_str().unwrap();
    assert_eq!(provider_call_id, call_id.as_str());
    let tool_execution_id = result["tool_execution_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(tool_execution_id).is_ok());
    let result_kind = result["result"]["result_kind"].as_str().unwrap();
    assert_eq!(result_kind, "success");
    (
        result.clone(),
        ToolResultProof {
            call_id: call_id.as_str().to_owned(),
            tool_execution_id: tool_execution_id.to_owned(),
            provider_call_id: provider_call_id.to_owned(),
            result_kind: result_kind.to_owned(),
        },
    )
}

#[derive(Clone)]
struct ParsedShellFacts {
    os: String,
    architecture: String,
    cwd: String,
    git_version: String,
    credential_canary_absent: bool,
}

fn parse_shell_facts(projection: &Value) -> ParsedShellFacts {
    parse_shell_facts_checked(projection).expect("valid Stage 24 shell fact projection")
}

fn parse_shell_facts_checked(projection: &Value) -> Result<ParsedShellFacts, String> {
    let stdout = field_projection_checked(projection, "stdout")?;
    let mut lines = stdout.lines();
    let result = ParsedShellFacts {
        os: lines
            .next()
            .ok_or_else(|| "tool OS line absent".to_owned())?
            .to_owned(),
        architecture: lines
            .next()
            .ok_or_else(|| "tool architecture line absent".to_owned())?
            .to_owned(),
        cwd: lines
            .next()
            .ok_or_else(|| "tool cwd line absent".to_owned())?
            .to_owned(),
        git_version: lines
            .next()
            .ok_or_else(|| "tool Git line absent".to_owned())?
            .to_owned(),
        credential_canary_absent: lines.next() == Some("credential_canary=absent"),
    };
    if lines.next().is_some() {
        return Err("tool shell projection has unexpected extra lines".to_owned());
    }
    Ok(result)
}

fn field_projection(projection: &Value, prefix: &str) -> String {
    field_projection_checked(projection, prefix)
        .unwrap_or_else(|error| panic!("invalid {prefix} projection fields: {error}"))
}

fn field_projection_checked(projection: &Value, prefix: &str) -> Result<String, String> {
    let fields = projection["result"]["fields"]
        .as_array()
        .ok_or_else(|| format!("missing {prefix} projection fields"))?;
    let mut parts = BTreeMap::new();
    for pair in fields {
        let pair = pair
            .as_array()
            .ok_or_else(|| format!("invalid {prefix} projection pair"))?;
        let key = pair
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| format!("invalid {prefix} projection key"))?;
        if key.starts_with(&format!("{prefix}_")) && !key.contains("omitted") {
            let value = pair
                .get(1)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("invalid {prefix} projection value"))?;
            parts.insert(key.to_owned(), value.to_owned());
        }
    }
    if parts.is_empty() {
        return Err(format!("missing {prefix} projection fields"));
    }
    Ok(parts.into_values().collect())
}

struct OperatorEvidence {
    export: Value,
}

fn run_operator_evidence(
    root: &Path,
    configuration: &Path,
    first_ready: &ReadyRecord,
    second_ready: &ReadyRecord,
    first_work: &str,
    second_work: &str,
) -> OperatorEvidence {
    let database = root.join("state/db/craxii.sqlite3");
    let database_before = Sha256Digest::hash_bytes(&fs::read(&database).unwrap());
    let verify = admin(configuration, &["verify-state", "--format", "json"]);
    assert_versioned_operator(&verify, "verify_state");
    assert_eq!(verify["data"]["consistent"], true);

    for work in [first_work, second_work] {
        let inspected = admin(configuration, &["inspect-work", work, "--format", "json"]);
        assert_versioned_operator(&inspected, "inspect_work");
        assert_eq!(inspected["data"]["work_id"], work);
        assert_eq!(inspected["data"]["state"], "completed");
    }
    for runtime in [&first_ready.runtime_id, &second_ready.runtime_id] {
        let inspected = admin(
            configuration,
            &["inspect-runtime", runtime, "--format", "json"],
        );
        assert_versioned_operator(&inspected, "inspect_runtime");
        assert_eq!(inspected["data"]["runtime_instance_id"], runtime.as_str());
    }
    let export = admin(configuration, &["evidence-export", "--format", "json"]);
    assert_versioned_operator(&export, "evidence_export");
    assert_eq!(export["data"]["verification"]["consistent"], true);
    let database_after = Sha256Digest::hash_bytes(&fs::read(&database).unwrap());
    assert_eq!(
        database_before, database_after,
        "operator evidence mutated SQLite"
    );

    let encoded = serde_json::to_string(&export).unwrap();
    for forbidden in [
        CANONICAL_REQUEST,
        FOLLOW_UP,
        FIXTURE_CONTENT.trim(),
        RAW_SHELL_COMMAND,
        CREDENTIAL_CANARY_VALUE,
        first_ready.bearer.as_str(),
        second_ready.bearer.as_str(),
        "stage18-req-1",
        "stage18-resp-1",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "operator evidence leaked {forbidden}"
        );
    }
    OperatorEvidence { export }
}

fn admin(configuration: &Path, arguments: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_craxii-admin"))
        .arg("--config")
        .arg(configuration)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("run Stage 23 read-only operator command");
    assert!(
        output.status.success(),
        "operator command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("versioned operator JSON")
}

fn assert_versioned_operator(value: &Value, kind: &str) {
    assert_eq!(value["format_version"], "craxii.operator-evidence/v1");
    assert_eq!(value["artifact_kind"], kind);
    assert_eq!(value["evidence_role"], "read_only_noncanonical");
}

struct PersistedSummary {
    first_work_tools: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn normalize_stage24_evidence(
    export: &Value,
    bootstrap: &Value,
    first_ready: &ReadyRecord,
    second_ready: &ReadyRecord,
    first_proof: &ProviderProof,
    second_proof: &ProviderProof,
    first_frames: &[Value],
    reconnect_frames: &[Value],
    replay_frames: &[Value],
    saved_cursor: u64,
    final_cursor: u64,
    pre_saved_event_ids: &BTreeSet<String>,
    retransmission_deduplicated: bool,
    telemetry_chains: Vec<NormalizedTelemetryChain>,
) -> Result<NormalizedStage24Evidence, String> {
    let data = &export["data"];
    let preflight = &data["preflight"];
    let mut works: Vec<&Value> = data["works"]
        .as_array()
        .ok_or_else(|| "operator work evidence absent".to_owned())?
        .iter()
        .collect();
    works.sort_by_key(|work| {
        work["conversation_work_ordinal"]
            .as_u64()
            .unwrap_or(u64::MAX)
    });
    let runtimes = data["runtimes"]
        .as_array()
        .ok_or_else(|| "operator runtime evidence absent".to_owned())?;

    let work_aliases: BTreeMap<String, String> = works
        .iter()
        .enumerate()
        .map(|(index, work)| {
            Ok((
                required_string(work, "work_id")?.to_owned(),
                format!("WORK_{}", index + 1),
            ))
        })
        .collect::<Result<_, String>>()?;
    let runtime_aliases = BTreeMap::from([
        (first_ready.runtime_id.clone(), "RUNTIME_1".to_owned()),
        (second_ready.runtime_id.clone(), "RUNTIME_2".to_owned()),
    ]);

    let mut model_aliases = BTreeMap::new();
    let mut logical_aliases = BTreeMap::new();
    let mut context_aliases = BTreeMap::new();
    let mut tool_aliases = BTreeMap::new();
    let mut execution_aliases = BTreeMap::new();
    let mut artifact_aliases = BTreeMap::new();
    let mut event_aliases = BTreeMap::new();
    for work in &works {
        let work_id = required_string(work, "work_id")?;
        let work_alias = required_alias(&work_aliases, work_id, "work")?;
        for model in required_array(work, "model_attempts")? {
            let step = required_u64(model, "agent_step")?;
            let attempt = required_u64(model, "attempt")?;
            let model_alias = format!("{work_alias}_MODEL_STEP_{step}_ATTEMPT_{attempt}");
            model_aliases.insert(
                required_string(model, "model_invocation_id")?.to_owned(),
                model_alias,
            );
            logical_aliases
                .entry(required_string(model, "logical_invocation_id")?.to_owned())
                .or_insert_with(|| format!("{work_alias}_LOGICAL_STEP_{step}"));
            context_aliases.insert(
                required_string(model, "context_manifest_id")?.to_owned(),
                format!("{work_alias}_CONTEXT_STEP_{step}"),
            );
        }
        for tool in required_array(work, "tool_executions")? {
            let ordinal = required_u64(tool, "tool_ordinal")?;
            tool_aliases.insert(
                required_string(tool, "tool_execution_id")?.to_owned(),
                format!("{work_alias}_TOOL_{ordinal}"),
            );
            execution_aliases.insert(
                required_string(tool, "workstation_execution_id")?.to_owned(),
                format!("{work_alias}_WS_EXEC_{ordinal}"),
            );
        }
        for (index, artifact) in required_array(work, "artifacts")?.iter().enumerate() {
            artifact_aliases.insert(
                required_string(artifact, "artifact_id")?.to_owned(),
                format!("{work_alias}_ARTIFACT_{}", index + 1),
            );
        }
        for (index, event) in required_array(work, "journal")?.iter().enumerate() {
            event_aliases.insert(
                required_string(event, "event_id")?.to_owned(),
                format!("{work_alias}_EVENT_{}", index + 1),
            );
        }
    }
    for (runtime_index, runtime) in runtimes.iter().enumerate() {
        let runtime_alias = runtime_aliases
            .get(required_string(runtime, "runtime_instance_id")?)
            .cloned()
            .unwrap_or_else(|| format!("RUNTIME_{}", runtime_index + 1));
        for (event_index, event) in required_array(runtime, "journal")?.iter().enumerate() {
            event_aliases.insert(
                required_string(event, "event_id")?.to_owned(),
                format!("{runtime_alias}_EVENT_{}", event_index + 1),
            );
        }
    }

    let mut digest_aliases = BTreeMap::new();
    let mut external_cause_aliases = BTreeMap::new();
    let mut normalized_works = Vec::new();
    for work in &works {
        let work_id = required_string(work, "work_id")?;
        let work_alias = required_alias(&work_aliases, work_id, "work")?.to_owned();
        let contexts = required_array(work, "contexts")?;
        let mut normalized_models = Vec::new();
        for model in required_array(work, "model_attempts")? {
            let model_id = required_string(model, "model_invocation_id")?;
            let context_id = required_string(model, "context_manifest_id")?;
            let context = find_id_result(contexts, "context_manifest_id", context_id)?;
            normalized_models.push(NormalizedModelEvidence {
                alias: required_alias(&model_aliases, model_id, "model")?.to_owned(),
                logical_invocation_alias: required_alias(
                    &logical_aliases,
                    required_string(model, "logical_invocation_id")?,
                    "logical invocation",
                )?
                .to_owned(),
                context_alias: required_alias(&context_aliases, context_id, "context")?.to_owned(),
                runtime_alias: required_alias(
                    &runtime_aliases,
                    required_string(model, "runtime_instance_id")?,
                    "runtime",
                )?
                .to_owned(),
                agent_step: required_u64(model, "agent_step")?,
                attempt: required_u64(model, "attempt")?,
                retry_of: optional_string(model, "retry_of_invocation_id")
                    .map(|id| required_alias(&model_aliases, id, "retry model").map(str::to_owned))
                    .transpose()?,
                state: required_string(model, "state")?.to_owned(),
                stop_reason: optional_string(model, "stop_reason").map(str::to_owned),
                certainty: optional_string(model, "provider_outcome_certainty").map(str::to_owned),
                tool_call_count: model["tool_call_count"].as_u64(),
                usage_status: required_string(model, "usage_status")?.to_owned(),
                draft_exposed: model["draft_exposed"]
                    .as_bool()
                    .ok_or_else(|| "model draft evidence absent".to_owned())?,
                request_digest_alias: alias_value(
                    &mut digest_aliases,
                    required_string(model, "request_sha256")?,
                    "REQUEST_DIGEST",
                ),
                response_digest_alias: optional_string(model, "response_sha256")
                    .map(|digest| alias_value(&mut digest_aliases, digest, "RESPONSE_DIGEST")),
                context_request_digest_matches: context["rendered_request_sha256"]
                    == model["request_sha256"],
            });
        }

        let proof_tools = first_proof
            .tools
            .iter()
            .chain(second_proof.tools.iter())
            .collect::<Vec<_>>();
        let mut normalized_tools = Vec::new();
        for tool in required_array(work, "tool_executions")? {
            let tool_id = required_string(tool, "tool_execution_id")?;
            let provider_call_id = proof_tools
                .iter()
                .find(|proof| proof.tool_execution_id == tool_id)
                .ok_or_else(|| "provider/tool durable identity relationship absent".to_owned())?
                .provider_call_id
                .clone();
            let mut linked_artifacts = Vec::new();
            for field in ["stdout_artifact_id", "stderr_artifact_id"] {
                if let Some(artifact_id) = optional_string(tool, field) {
                    linked_artifacts.push(
                        required_alias(&artifact_aliases, artifact_id, "tool artifact")?.to_owned(),
                    );
                }
            }
            for artifact in required_array(work, "artifacts")? {
                if optional_string(artifact, "producer_id") == Some(tool_id) {
                    let alias = required_alias(
                        &artifact_aliases,
                        required_string(artifact, "artifact_id")?,
                        "producer artifact",
                    )?
                    .to_owned();
                    if !linked_artifacts.contains(&alias) {
                        linked_artifacts.push(alias);
                    }
                }
            }
            normalized_tools.push(NormalizedToolEvidence {
                alias: required_alias(&tool_aliases, tool_id, "tool")?.to_owned(),
                workstation_execution_alias: required_alias(
                    &execution_aliases,
                    required_string(tool, "workstation_execution_id")?,
                    "workstation execution",
                )?
                .to_owned(),
                source_model_alias: normalized_tool_source_alias(tool, &model_aliases)?,
                provider_call_id,
                agent_step: required_u64(tool, "agent_step")?,
                ordinal: required_u64(tool, "tool_ordinal")?,
                name: required_string(tool, "tool_name")?.to_owned(),
                state: required_string(tool, "state")?.to_owned(),
                result_class: optional_string(tool, "result_class").map(str::to_owned),
                effective_privilege: optional_string(tool, "effective_privilege")
                    .map(str::to_owned),
                timed_out: tool["timed_out"].as_bool(),
                cancelled: tool["cancelled"].as_bool(),
                cleanup_confirmed: tool["cleanup_confirmed"].as_bool(),
                artifact_aliases: linked_artifacts,
            });
        }

        let normalized_artifacts = required_array(work, "artifacts")?
            .iter()
            .map(|artifact| {
                let artifact_id = required_string(artifact, "artifact_id")?;
                let digest = required_string(artifact, "sha256")?;
                let producer_tool_alias = optional_string(artifact, "producer_id")
                    .map(|id| {
                        required_alias(&tool_aliases, id, "artifact producer").map(str::to_owned)
                    })
                    .transpose()?;
                Ok(NormalizedArtifactEvidence {
                    alias: required_alias(&artifact_aliases, artifact_id, "artifact")?.to_owned(),
                    producer_tool_alias,
                    digest_alias: alias_value(&mut digest_aliases, digest, "ARTIFACT_DIGEST"),
                    storage_key_matches_digest: required_string(artifact, "storage_key")?
                        .starts_with("sha256/")
                        && required_string(artifact, "storage_key")?
                            .rsplit('/')
                            .next()
                            .is_some_and(|suffix| suffix == digest),
                    retention_class: required_string(artifact, "retention_class")?.to_owned(),
                    truncated: artifact["truncated"]
                        .as_bool()
                        .ok_or_else(|| "artifact truncation evidence absent".to_owned())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let normalized_journal = required_array(work, "journal")?
            .iter()
            .map(|event| {
                let cause = optional_string(event, "causation_event_id").map(|cause| {
                    event_aliases.get(cause).cloned().unwrap_or_else(|| {
                        alias_value(&mut external_cause_aliases, cause, "EXTERNAL_EVENT")
                    })
                });
                Ok(NormalizedJournalEvidence {
                    alias: required_alias(
                        &event_aliases,
                        required_string(event, "event_id")?,
                        "journal event",
                    )?
                    .to_owned(),
                    stream_sequence: required_u64(event, "stream_sequence")?,
                    event_type: required_string(event, "event_type")?.to_owned(),
                    cause,
                    runtime_alias: optional_string(event, "runtime_instance_id")
                        .map(|id| {
                            required_alias(&runtime_aliases, id, "event runtime").map(str::to_owned)
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        normalized_works.push(NormalizedWorkEvidence {
            alias: work_alias,
            ordinal: required_u64(work, "conversation_work_ordinal")?,
            state: required_string(work, "state")?.to_owned(),
            terminal_reason: optional_string(work, "terminal_reason_code").map(str::to_owned),
            correlation_matches_work: required_string(work, "correlation_id")? == work_id,
            model_attempts: normalized_models,
            tools: normalized_tools,
            artifacts: normalized_artifacts,
            journal: normalized_journal,
        });
    }

    let normalized_runtimes = runtimes
        .iter()
        .map(|runtime| {
            let runtime_id = required_string(runtime, "runtime_instance_id")?;
            Ok(NormalizedRuntimeEvidence {
                alias: required_alias(&runtime_aliases, runtime_id, "runtime")?.to_owned(),
                state: required_string(runtime, "state")?.to_owned(),
                stop_reason: optional_string(runtime, "stop_reason").map(str::to_owned),
                owned_work_count: required_u64(runtime, "owned_work_count")?,
                model_attempt_count: required_u64(runtime, "model_attempt_count")?,
                tool_execution_count: required_u64(runtime, "tool_execution_count")?,
                journal_event_types: required_array(runtime, "journal")?
                    .iter()
                    .map(|event| required_string(event, "event_type").map(str::to_owned))
                    .collect::<Result<Vec<_>, String>>()?,
                recovery: required_array(runtime, "recovery")?
                    .iter()
                    .map(|recovery| {
                        Ok(NormalizedRecoveryEvidence {
                            stale_runtime_count: recovery["stale_runtime_count"].as_u64(),
                            queued_work_retained: recovery["queued_work_retained"].as_u64(),
                            work_interrupted: recovery["work_interrupted"].as_u64(),
                            model_attempts_marked_unknown:
                                recovery["model_attempts_marked_unknown"].as_u64(),
                            tool_attempts_marked_unknown: recovery["tool_attempts_marked_unknown"]
                                .as_u64(),
                            cleanup_checks_performed: recovery["cleanup_checks_performed"].as_u64(),
                            cleanup_unconfirmed: recovery["cleanup_unconfirmed"].as_u64(),
                            orphan_count: recovery["orphan_count"].as_u64(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let provider_dependencies = [first_proof, second_proof]
        .into_iter()
        .map(|proof| {
            Ok(NormalizedProviderDependency {
                phase: proof.phase.clone(),
                invocation_count: proof.invocation_count,
                answer_source: proof.answer_source.clone(),
                answer_context_verified: proof.answer_context_verified,
                tool_call_ids: proof
                    .tools
                    .iter()
                    .map(|tool| tool.call_id.clone())
                    .collect(),
                tool_execution_aliases: proof
                    .tools
                    .iter()
                    .map(|tool| {
                        required_alias(&tool_aliases, &tool.tool_execution_id, "provider tool")
                            .map(str::to_owned)
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                prior_assistant_required: proof.durable_prior_assistant_seen,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let replayed: Vec<&Value> = reconnect_frames
        .iter()
        .filter(|frame| frame["delivery_kind"] == "durable")
        .collect();
    let reconnect = NormalizedReconnectEvidence {
        replayed_durable_count: u64::try_from(replayed.len()).unwrap_or(u64::MAX),
        every_replayed_cursor_after_saved: replayed.iter().all(|frame| {
            frame["cursor"]
                .as_u64()
                .is_some_and(|cursor| cursor > saved_cursor)
        }),
        no_pre_saved_event_replayed: replayed.iter().all(|frame| {
            frame["event_id"]
                .as_str()
                .is_some_and(|event_id| !pre_saved_event_ids.contains(event_id))
        }),
        live_handoff_at_or_after_saved: reconnect_frames
            .last()
            .and_then(|frame| frame["through_cursor"].as_u64())
            .is_some_and(|cursor| cursor >= saved_cursor),
        final_cursor_after_saved: final_cursor > saved_cursor,
    };

    let restart_identity_contract_verified = first_ready.runtime_id != second_ready.runtime_id
        && first_ready.craxii_id == second_ready.craxii_id
        && first_ready.conversation_id == second_ready.conversation_id
        && first_ready.workstation_id == second_ready.workstation_id
        && first_ready.workspace_id == second_ready.workspace_id;
    let persistence_and_artifact_integrity_verified = data["verification"]["consistent"] == true
        && data["verification"]["referenced_artifact_count"]
            == data["verification"]["verified_artifact_count"];
    let durable_history_verified = second_proof.durable_prior_assistant_seen
        && second_proof.answer_context_verified
        && second_proof.answer_source == "durable_prior_assistant_and_tool_context";
    let provider_tool_values_verified = first_proof.durable_tool_values_verified
        && first_proof.answer_context_verified
        && first_proof.answer_source == "model_visible_tool_results";

    let assistant_precedes_completion = normalized_works.iter().all(|work| {
        let assistant = work
            .journal
            .iter()
            .position(|event| event.event_type == "assistant.message_committed");
        let completed = work
            .journal
            .iter()
            .position(|event| event.event_type == "work.completed");
        matches!((assistant, completed), (Some(a), Some(c)) if a < c)
    });
    let tool_relationships_hold =
        normalized_works
            .iter()
            .flat_map(|work| &work.tools)
            .all(|tool| {
                !tool.source_model_alias.is_empty() && !tool.workstation_execution_alias.is_empty()
            });
    let provider_pairing_holds = first_proof.tools.iter().all(|tool| {
        tool.call_id == tool.provider_call_id && tool_aliases.contains_key(&tool.tool_execution_id)
    });
    let telemetry_verified = telemetry_chains
        .iter()
        .any(|chain| chain.workstation_span_present && chain.workstation_result_class == "exited");
    let mut satisfied_relationships = Vec::new();
    for (satisfied, name) in [
        (
            retransmission_deduplicated,
            "retransmission_preserves_message_and_work_identity",
        ),
        (
            provider_pairing_holds,
            "tool_results_pair_by_provider_call_and_tool_execution_identity",
        ),
        (
            tool_relationships_hold,
            "tool_executions_link_to_source_model_and_workstation_execution",
        ),
        (
            assistant_precedes_completion,
            "assistant_commit_precedes_atomic_work_completion",
        ),
        (
            restart_identity_contract_verified,
            "restart_preserves_product_and_conversation_identity",
        ),
        (
            durable_history_verified,
            "follow_up_context_contains_durable_prior_history",
        ),
        (
            telemetry_verified,
            "telemetry_links_request_command_work_model_tool_and_workstation",
        ),
        (
            reconnect.every_replayed_cursor_after_saved
                && reconnect.no_pre_saved_event_replayed
                && reconnect.live_handoff_at_or_after_saved,
            "replay_and_bootstrap_converge_to_the_same_durable_cursor",
        ),
    ] {
        if satisfied {
            satisfied_relationships.push(name.to_owned());
        }
    }

    let message_roles = bootstrap["messages"]
        .as_array()
        .ok_or_else(|| "bootstrap messages absent".to_owned())?
        .iter()
        .map(|message| required_string(message, "role").map(str::to_owned))
        .collect::<Result<Vec<_>, String>>()?;
    let first_work_model_steps = normalized_works
        .first()
        .ok_or_else(|| "first Work absent".to_owned())?
        .model_attempts
        .iter()
        .map(|model| model.agent_step)
        .collect();
    let first_work_tools = normalized_works[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    let follow_up_model_steps = normalized_works
        .get(1)
        .ok_or_else(|| "follow-up Work absent".to_owned())?
        .model_attempts
        .iter()
        .map(|model| model.agent_step)
        .collect();
    let work_results = normalized_works
        .iter()
        .map(|work| {
            format!(
                "{}:{}",
                work.state,
                work.terminal_reason.as_deref().unwrap_or("none")
            )
        })
        .collect();
    let model_results = normalized_works
        .iter()
        .flat_map(|work| work.model_attempts.iter().map(|model| model.state.clone()))
        .collect();
    let tool_results = normalized_works
        .iter()
        .flat_map(|work| {
            work.tools.iter().map(|tool| {
                tool.result_class
                    .clone()
                    .unwrap_or_else(|| "none".to_owned())
            })
        })
        .collect();

    Ok(NormalizedStage24Evidence {
        contract_version: "craxii.stage24.evidence/v1",
        schema_version: required_u64(preflight, "schema_version")?,
        protocol_version: bootstrap["protocol_version"]
            .as_u64()
            .ok_or_else(|| "protocol version absent".to_owned())?,
        user_messages: u64::try_from(
            message_roles
                .iter()
                .filter(|role| role.as_str() == "user")
                .count(),
        )
        .unwrap_or(u64::MAX),
        assistant_messages: u64::try_from(
            message_roles
                .iter()
                .filter(|role| role.as_str() == "assistant")
                .count(),
        )
        .unwrap_or(u64::MAX),
        completed_work: u64::try_from(
            normalized_works
                .iter()
                .filter(|work| work.state == "completed")
                .count(),
        )
        .unwrap_or(u64::MAX),
        model_attempts: normalized_works
            .iter()
            .map(|work| u64::try_from(work.model_attempts.len()).unwrap_or(u64::MAX))
            .sum(),
        tool_executions: normalized_works
            .iter()
            .map(|work| u64::try_from(work.tools.len()).unwrap_or(u64::MAX))
            .sum(),
        runtime_count: u64::try_from(normalized_runtimes.len()).unwrap_or(u64::MAX),
        first_work_model_steps,
        first_work_tools,
        follow_up_model_steps,
        work_results,
        model_results,
        tool_results,
        first_live_event_types: event_types(first_frames),
        reconnect_event_types: event_types(reconnect_frames),
        replay_event_types: event_types(replay_frames),
        message_roles,
        work_ordinals: normalized_works.iter().map(|work| work.ordinal).collect(),
        works: normalized_works,
        runtimes: normalized_runtimes,
        provider_dependencies,
        reconnect,
        telemetry_chains,
        satisfied_relationships,
        retransmission_deduplicated,
        provider_tool_values_verified,
        durable_history_verified,
        restart_identity_contract_verified,
        persistence_and_artifact_integrity_verified,
        operator_evidence_verified: export["artifact_kind"] == "evidence_export"
            && export["evidence_role"] == "read_only_noncanonical",
        telemetry_composition_and_redaction_verified: telemetry_verified,
        portable_host_result: "passed",
        ubuntu_target_result: "deferred_by_user_to_stage_27",
    })
}

fn required_array<'a>(value: &'a Value, field: &str) -> Result<&'a [Value], String> {
    value[field]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing evidence array {field}"))
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing evidence string {field}"))
}

fn optional_string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value[field].as_str()
}

fn required_u64(value: &Value, field: &str) -> Result<u64, String> {
    value[field]
        .as_u64()
        .ok_or_else(|| format!("missing evidence integer {field}"))
}

fn required_alias<'a>(
    aliases: &'a BTreeMap<String, String>,
    identity: &str,
    kind: &str,
) -> Result<&'a str, String> {
    aliases
        .get(identity)
        .map(String::as_str)
        .ok_or_else(|| format!("unresolved {kind} relationship"))
}

fn alias_value(aliases: &mut BTreeMap<String, String>, value: &str, prefix: &str) -> String {
    let next = aliases.len() + 1;
    aliases
        .entry(value.to_owned())
        .or_insert_with(|| format!("{prefix}_{next}"))
        .clone()
}

fn find_id_result<'a>(
    values: &'a [Value],
    field: &str,
    expected: &str,
) -> Result<&'a Value, String> {
    values
        .iter()
        .find(|value| value[field] == expected)
        .ok_or_else(|| format!("missing {field} relationship"))
}

fn normalized_tool_source_alias(
    tool: &Value,
    model_aliases: &BTreeMap<String, String>,
) -> Result<String, String> {
    required_alias(
        model_aliases,
        required_string(tool, "source_model_invocation_id")?,
        "source model",
    )
    .map(str::to_owned)
}

#[allow(clippy::too_many_arguments)]
async fn assert_persistence_contract(
    root: &Path,
    export: &Value,
    bootstrap: &Value,
    first_ready: &ReadyRecord,
    second_ready: &ReadyRecord,
    first_proof: &ProviderProof,
    facts: &MachineFacts,
    first_work_id: &str,
    follow_work_id: &str,
) -> PersistedSummary {
    let data = &export["data"];
    let preflight = &data["preflight"];
    assert_eq!(preflight["schema_version"], 4);
    assert_eq!(preflight["database_disposition"], "current");
    assert_eq!(preflight["work_count"], 2);
    assert_eq!(preflight["runtime_count"], 2);
    assert_eq!(preflight["model_attempt_count"], 3);
    assert_eq!(preflight["tool_execution_count"], 2);
    assert_eq!(preflight["artifact_count"], 1);
    assert_eq!(data["verification"]["consistent"], true);
    assert_eq!(
        data["verification"]["referenced_artifact_count"],
        preflight["artifact_count"]
    );
    assert_eq!(
        data["verification"]["verified_artifact_count"],
        preflight["artifact_count"]
    );

    let messages = bootstrap["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|value| value["role"] == "user")
            .count(),
        2
    );
    assert_eq!(
        messages
            .iter()
            .filter(|value| value["role"] == "assistant")
            .count(),
        2
    );
    assert_eq!(
        messages
            .iter()
            .filter(|value| value["client_message_id"] == FIRST_CLIENT_ID)
            .count(),
        1
    );

    let works = data["works"].as_array().unwrap();
    assert_eq!(works.len(), 2);
    let first = find_id(works, "work_id", first_work_id);
    let follow = find_id(works, "work_id", follow_work_id);
    for work in [first, follow] {
        assert_eq!(work["state"], "completed");
        assert_eq!(work["terminal_reason_code"], "answered");
        assert!(work["runtime_instance_id"].is_null());
        assert!(work["current_model_invocation_id"].is_null());
        assert!(work["current_tool_execution_id"].is_null());
        assert_eq!(work["craxii_id"], first_ready.craxii_id);
        assert_eq!(work["conversation_id"], first_ready.conversation_id);
        assert_eq!(work["workspace_id"], first_ready.workspace_id);
        assert_journal_order(work);
    }
    assert_eq!(first["conversation_work_ordinal"], 1);
    assert_eq!(follow["conversation_work_ordinal"], 2);

    let first_models = first["model_attempts"].as_array().unwrap();
    let first_contexts = first["contexts"].as_array().unwrap();
    assert_eq!(first_models.len(), 2);
    assert_eq!(first_contexts.len(), 2);
    assert_eq!(
        first_models
            .iter()
            .map(|model| model["agent_step"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(first_models.iter().all(assert_model_record));
    assert_eq!(first_models[0]["tool_call_count"], 2);
    assert_eq!(first_models[1]["tool_call_count"], 0);
    assert_eq!(
        first_models[0]["runtime_instance_id"],
        first_ready.runtime_id
    );
    assert_eq!(
        first_models[1]["runtime_instance_id"],
        first_ready.runtime_id
    );
    assert_context_links(first_contexts, first_models);

    let follow_models = follow["model_attempts"].as_array().unwrap();
    let follow_contexts = follow["contexts"].as_array().unwrap();
    assert_eq!(follow_models.len(), 1);
    assert_eq!(follow_contexts.len(), 1);
    assert!(assert_model_record(&follow_models[0]));
    assert_eq!(follow_models[0]["agent_step"], 1);
    assert_eq!(follow_models[0]["tool_call_count"], 0);
    assert_eq!(
        follow_models[0]["runtime_instance_id"],
        second_ready.runtime_id
    );
    assert_context_links(follow_contexts, follow_models);
    assert!(follow["tool_executions"].as_array().unwrap().is_empty());

    let tools = first["tool_executions"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool["tool_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["run_shell", "read_file"]);
    for (index, tool) in tools.iter().enumerate() {
        assert_eq!(tool["state"], "completed");
        assert_eq!(tool["result_class"], "success");
        assert_eq!(tool["agent_step"], 1);
        assert_eq!(tool["tool_ordinal"], u64::try_from(index + 1).unwrap());
        assert_eq!(tool["runtime_instance_id"], first_ready.runtime_id);
        assert_eq!(tool["workstation_id"], first_ready.workstation_id);
        assert_eq!(tool["workspace_id"], first_ready.workspace_id);
        assert_eq!(tool["workstation_generation"], 1);
        assert_eq!(
            tool["source_model_invocation_id"],
            first_models[0]["model_invocation_id"]
        );
        assert_eq!(tool["effective_privilege"], "user");
        assert_eq!(tool["timed_out"], false);
        assert_eq!(tool["cancelled"], false);
        assert!(ordered_times(tool));
        assert_eq!(
            tool["tool_execution_id"],
            first_proof.tools[index].tool_execution_id
        );
        assert!(uuid::Uuid::parse_str(tool["workstation_execution_id"].as_str().unwrap()).is_ok());
    }
    assert_eq!(tools[0]["cleanup_confirmed"], true);

    let artifact_values = first["artifacts"].as_array().unwrap();
    assert_eq!(artifact_values.len(), 1);
    verify_artifact(root, &artifact_values[0], facts);
    assert!(follow["artifacts"].as_array().unwrap().is_empty());

    let first_journal = first["journal"].as_array().unwrap();
    let lifecycle: Vec<&str> = first_journal
        .iter()
        .filter_map(|event| event["event_type"].as_str())
        .filter(|event| event.starts_with("tool.execution_"))
        .collect();
    assert_eq!(
        lifecycle,
        [
            "tool.execution_requested",
            "tool.execution_dispatching",
            "tool.execution_completed",
            "tool.execution_requested",
            "tool.execution_dispatching",
            "tool.execution_completed",
        ]
    );
    for work in [first, follow] {
        let journal = work["journal"].as_array().unwrap();
        let assistant = journal
            .iter()
            .find(|event| event["event_type"] == "assistant.message_committed")
            .unwrap()["journal_offset"]
            .as_u64()
            .unwrap();
        let completed = journal
            .iter()
            .find(|event| event["event_type"] == "work.completed")
            .unwrap()["journal_offset"]
            .as_u64()
            .unwrap();
        assert!(assistant < completed);
    }

    let runtimes = data["runtimes"].as_array().unwrap();
    assert_eq!(runtimes.len(), 2);
    let first_runtime = find_id(runtimes, "runtime_instance_id", &first_ready.runtime_id);
    let second_runtime = find_id(runtimes, "runtime_instance_id", &second_ready.runtime_id);
    assert_eq!(first_runtime["owned_work_count"], 0);
    assert_eq!(second_runtime["owned_work_count"], 0);
    assert_eq!(first_runtime["model_attempt_count"], 2);
    assert_eq!(first_runtime["tool_execution_count"], 2);
    assert_eq!(second_runtime["model_attempt_count"], 1);
    assert_eq!(second_runtime["tool_execution_count"], 0);
    assert!(
        second_runtime["recovery"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["stale_runtime_count"]
                    .as_u64()
                    .is_some_and(|count| count >= 1)
                    && item["work_interrupted"] == 0
                    && item["model_attempts_marked_unknown"] == 0
                    && item["tool_attempts_marked_unknown"] == 0
            })
    );

    let database = root.join("state/db/craxii.sqlite3");
    assert_sqlite_contract(&database).await;
    PersistedSummary {
        first_work_tools: names,
    }
}

fn assert_model_record(model: &Value) -> bool {
    assert_eq!(model["state"], "completed");
    assert_eq!(model["attempt"], 1);
    assert_eq!(model["target"], "stage18-primary");
    assert_eq!(model["provider"], "stage18-scripted");
    assert_eq!(model["model"], "fixture-model");
    assert_eq!(model["selection_reason"], "configured_default");
    assert_eq!(model["usage_status"], "reported");
    assert_eq!(model["provider_outcome_certainty"], "definitely_completed");
    assert!(
        model["request_sha256"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    assert!(
        model["response_sha256"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    assert!(model["context_manifest_id"].is_string());
    true
}

fn assert_context_links(contexts: &[Value], models: &[Value]) {
    for model in models {
        let context = find_id(
            contexts,
            "context_manifest_id",
            model["context_manifest_id"].as_str().unwrap(),
        );
        assert_eq!(
            context["logical_invocation_id"],
            model["logical_invocation_id"]
        );
        assert_eq!(context["target"], model["target"]);
        assert_eq!(context["provider"], model["provider"]);
        assert_eq!(context["model"], model["model"]);
        assert_eq!(context["target_configuration_version"], 1);
        assert_eq!(context["rendered_request_sha256"], model["request_sha256"]);
        assert!(
            context["source_count"]
                .as_u64()
                .is_some_and(|value| value > 0)
        );
        assert!(
            context["canonical_byte_count"]
                .as_u64()
                .is_some_and(|value| value > 0)
        );
        assert!(
            context["rendered_request_byte_count"]
                .as_u64()
                .is_some_and(|value| value > 0)
        );
        assert!(
            context["manifest_sha256"]
                .as_str()
                .is_some_and(|value| value.len() == 64)
        );
    }
}

fn ordered_times(tool: &Value) -> bool {
    let requested = tool["requested_at"].as_str().unwrap();
    let dispatch = tool["dispatch_intent_at"].as_str().unwrap();
    let started = tool["started_at"].as_str().unwrap();
    let completed = tool["completed_at"].as_str().unwrap();
    requested <= dispatch && dispatch <= started && started <= completed
}

fn find_id<'a>(values: &'a [Value], field: &str, expected: &str) -> &'a Value {
    values
        .iter()
        .find(|value| value[field] == expected)
        .unwrap_or_else(|| panic!("missing {field}={expected}"))
}

fn assert_journal_order(work: &Value) {
    let journal = work["journal"].as_array().unwrap();
    let offsets: Vec<u64> = journal
        .iter()
        .map(|event| event["journal_offset"].as_u64().unwrap())
        .collect();
    assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
    let ids: BTreeSet<&str> = journal
        .iter()
        .map(|event| event["event_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), journal.len());
    let id_to_offset: BTreeMap<&str, u64> = journal
        .iter()
        .map(|event| {
            (
                event["event_id"].as_str().unwrap(),
                event["journal_offset"].as_u64().unwrap(),
            )
        })
        .collect();
    let causal_links = journal
        .iter()
        .filter_map(|event| {
            let cause = event["causation_event_id"].as_str()?;
            let prior = id_to_offset.get(cause)?;
            Some((*prior, event["journal_offset"].as_u64().unwrap()))
        })
        .collect::<Vec<_>>();
    assert!(!causal_links.is_empty());
    assert!(causal_links.iter().all(|(cause, effect)| cause < effect));
}

fn verify_artifact(root: &Path, artifact: &Value, facts: &MachineFacts) {
    let key = artifact["storage_key"].as_str().unwrap();
    assert!(key.starts_with("sha256/"));
    let digest = artifact["sha256"].as_str().unwrap();
    assert_eq!(&key[10..], digest);
    let path = root
        .join("artifacts/sha256")
        .join(&key[7..9])
        .join(&key[10..]);
    let bytes = fs::read(path).expect("read persisted Stage 24 artifact");
    assert_eq!(
        u64::try_from(bytes.len()).unwrap(),
        artifact["captured_byte_count"].as_u64().unwrap()
    );
    assert_eq!(Sha256Digest::hash_bytes(&bytes).to_string(), digest);
    let stdout = String::from_utf8(bytes).unwrap();
    assert!(stdout.contains(&facts.os));
    assert!(stdout.contains(&facts.architecture));
    assert!(stdout.contains(&facts.cwd));
    assert!(stdout.contains(&facts.git_version));
    assert!(stdout.contains("credential_canary=absent"));
    assert!(!stdout.contains(CREDENTIAL_CANARY_VALUE));
}

async fn assert_sqlite_contract(database: &Path) {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .foreign_keys(true);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .unwrap();
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let integrity: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let migrations = sqlx::query("SELECT version, success FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(journal_mode, "wal");
    assert_eq!(foreign_keys, 1);
    assert_eq!(integrity, "ok");
    assert_eq!(
        migrations
            .iter()
            .map(|row| (row.get::<i64, _>("version"), row.get::<bool, _>("success")))
            .collect::<Vec<_>>(),
        [(1, true), (2, true), (3, true), (4, true)]
    );
    let mut migration_files: Vec<String> =
        fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    migration_files.sort();
    assert_eq!(
        migration_files,
        [
            "0001_core_durable_schema.sql",
            "0002_journal_and_work_inputs.sql",
            "0003_context_model_tool_artifacts.sql",
            "0004_model_attempt_outcome_evidence.sql",
        ]
    );
}

#[allow(clippy::too_many_arguments)]
fn assert_telemetry_contract(
    output: &str,
    export: &Value,
    first_bearer: &str,
    second_bearer: &str,
    first_work_id: &str,
    follow_work_id: &str,
    first_proof: &ProviderProof,
    facts: &MachineFacts,
) -> Vec<NormalizedTelemetryChain> {
    let records: Vec<Value> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("production JSON telemetry record"))
        .collect();
    let accepted = find_event(&records, "client_command_terminal", |record| {
        record["client_message_id"] == FIRST_CLIENT_ID && record["result_class"] == "accepted"
    });
    assert_eq!(accepted["work_id"], first_work_id);
    let first_message_id = accepted["message_id"].as_str().unwrap();
    let request_id = accepted["request_id"].as_str().unwrap();
    assert_span_path(accepted, &["http_request", "client_command"]);
    let request = find_event(&records, "http_request_terminal", |record| {
        record["request_id"] == request_id && record["status"] == 202
    });
    assert_eq!(request["result_class"], "success");
    let retransmission = find_event(&records, "client_command_terminal", |record| {
        record["client_message_id"] == FIRST_CLIENT_ID && record["result_class"] == "retransmission"
    });
    assert_eq!(retransmission["work_id"], first_work_id);
    assert_eq!(retransmission["message_id"], first_message_id);

    let follow = find_event(&records, "client_command_terminal", |record| {
        record["client_message_id"] == FOLLOW_CLIENT_ID && record["result_class"] == "accepted"
    });
    assert_eq!(follow["work_id"], follow_work_id);
    assert_span_path(follow, &["http_request", "client_command"]);

    for work in [first_work_id, follow_work_id] {
        let terminal = find_event(&records, "work_terminal", |record| {
            record["work_id"] == work
        });
        assert_span_path(terminal, &["work_execution"]);
    }
    let model_events: Vec<&Value> = records
        .iter()
        .filter(|record| record["event_name"] == "model_attempt_terminal")
        .collect();
    assert_eq!(model_events.len(), 3);
    assert_eq!(
        model_events
            .iter()
            .filter(|record| record["work_id"] == first_work_id)
            .count(),
        2
    );
    assert_eq!(
        model_events
            .iter()
            .filter(|record| record["work_id"] == follow_work_id)
            .count(),
        1
    );
    for event in model_events {
        assert_eq!(event["result_class"], "completed");
        assert_eq!(event["attempt_ordinal"], 1);
        assert_eq!(event["target"], "stage18-primary");
        assert_eq!(event["provider"], "stage18-scripted");
        assert_eq!(event["model"], "fixture-model");
        assert!(event["logical_invocation_id"].is_string());
        assert!(event["model_invocation_id"].is_string());
        assert!(event["request_sha256"].is_string());
        assert_span_path(event, &["work_execution", "model_invocation_attempt"]);
    }

    let first_work = find_id(
        export["data"]["works"].as_array().unwrap(),
        "work_id",
        first_work_id,
    );
    let work_ordinal = first_work["conversation_work_ordinal"].as_u64().unwrap();
    let work_alias = format!("WORK_{work_ordinal}");
    let accepted_record_index = records
        .iter()
        .position(|record| std::ptr::eq(record, accepted))
        .unwrap();
    let accepted_ordinal = records[..=accepted_record_index]
        .iter()
        .filter(|record| {
            record["event_name"] == "client_command_terminal"
                && record["result_class"] == "accepted"
        })
        .count();
    let request_alias = format!("REQUEST_{accepted_ordinal}");
    let durable_tools = first_work["tool_executions"].as_array().unwrap();
    let mut telemetry_chains = Vec::new();
    for (index, proof) in first_proof.tools.iter().enumerate() {
        let durable = find_id(durable_tools, "tool_execution_id", &proof.tool_execution_id);
        let terminal = find_event(&records, "tool_execution_terminal", |record| {
            record["tool_execution_id"] == proof.tool_execution_id
        });
        assert_eq!(terminal["result_class"], "success");
        assert_eq!(
            terminal["execution_id"],
            durable["workstation_execution_id"]
        );
        assert_span_path(terminal, &["work_execution"]);
        let work_span = find_span(terminal, "work_execution").unwrap();
        assert_eq!(work_span["work_id"], first_work_id);
        assert_eq!(
            durable["source_model_invocation_id"],
            first_work["model_attempts"][0]["model_invocation_id"]
        );
        let source_model = find_event(&records, "model_attempt_terminal", |record| {
            record["model_invocation_id"] == durable["source_model_invocation_id"]
                && record["work_id"] == first_work_id
        });
        assert_eq!(source_model["result_class"], "completed");
        assert!(durable["tool_name"] == "run_shell" || durable["tool_name"] == "read_file");
        assert_eq!(
            index + 1,
            durable["tool_ordinal"].as_u64().unwrap() as usize
        );
        if durable["tool_name"] == "run_shell" {
            let workstation = find_event(&records, "workstation_execution_terminal", |record| {
                record["execution_id"] == durable["workstation_execution_id"]
            });
            assert_eq!(workstation["result_class"], "exited");
            assert_eq!(workstation["cleanup_confirmed"], true);
            assert_span_path(
                workstation,
                &[
                    "work_execution",
                    "tool_execution_service",
                    "workstation_execute",
                ],
            );
            let tool_span = find_span(workstation, "tool_execution_service").unwrap();
            assert_eq!(tool_span["tool_execution_id"], proof.tool_execution_id);
            assert_eq!(
                tool_span["workstation_execution_id"],
                durable["workstation_execution_id"]
            );
            let workstation_span = find_span(workstation, "workstation_execute").unwrap();
            assert_eq!(
                workstation_span["execution_id"],
                durable["workstation_execution_id"]
            );
            assert_eq!(workstation_span["work_id"], first_work_id);
            let durable_model = find_id(
                first_work["model_attempts"].as_array().unwrap(),
                "model_invocation_id",
                durable["source_model_invocation_id"].as_str().unwrap(),
            );
            let model_step = durable_model["agent_step"].as_u64().unwrap();
            let model_attempt = durable_model["attempt"].as_u64().unwrap();
            let tool_ordinal = durable["tool_ordinal"].as_u64().unwrap();
            telemetry_chains.push(NormalizedTelemetryChain {
                request_alias: request_alias.clone(),
                command_kind: accepted["command_kind"]
                    .as_str()
                    .unwrap_or("message")
                    .to_owned(),
                work_alias: work_alias.clone(),
                model_alias: format!(
                    "{work_alias}_MODEL_STEP_{model_step}_ATTEMPT_{model_attempt}"
                ),
                tool_alias: format!("{work_alias}_TOOL_{tool_ordinal}"),
                workstation_execution_alias: format!("{work_alias}_WS_EXEC_{tool_ordinal}"),
                workstation_result_class: workstation["result_class"].as_str().unwrap().to_owned(),
                workstation_span_present: true,
            });
        }
    }

    let recovery = records
        .iter()
        .filter(|record| record["event_name"] == "startup_recovery_terminal")
        .find(|record| {
            record["stale_runtime_count"]
                .as_u64()
                .is_some_and(|value| value >= 1)
        })
        .expect("restart recovery telemetry");
    assert_eq!(recovery["work_interrupted"], 0);
    assert_eq!(recovery["model_attempts_marked_unknown"], 0);
    assert_eq!(recovery["tool_attempts_marked_unknown"], 0);
    assert!(
        records
            .iter()
            .any(|record| record["event_name"] == "websocket_connected")
    );
    let handoffs: Vec<&Value> = records
        .iter()
        .filter(|record| record["event_name"] == "websocket_live_handoff")
        .collect();
    assert!(handoffs.len() >= 3);
    assert!(handoffs.iter().all(|record| {
        record["cursor"].is_number()
            && find_span(record, "websocket_connection")
                .is_some_and(|span| span["request_id"].is_string())
    }));

    let shell_stdout = format!(
        "{}\n{}\n{}\n{}\ncredential_canary=absent\n",
        facts.os, facts.architecture, facts.cwd, facts.git_version
    );
    for forbidden in [
        first_bearer,
        second_bearer,
        CANONICAL_REQUEST,
        FOLLOW_UP,
        facts.answer().as_str(),
        RAW_SHELL_COMMAND,
        FIXTURE_CONTENT.trim(),
        shell_stdout.as_str(),
        CREDENTIAL_CANARY_VALUE,
        "stage18-req-1",
        "stage18-resp-1",
        "stage18-req-2",
        "stage18-resp-2",
        "Authorization",
    ] {
        assert!(
            !output.contains(forbidden),
            "production telemetry leaked {forbidden}"
        );
    }
    assert_eq!(telemetry_chains.len(), 1);
    telemetry_chains
}

fn find_event<'a>(
    records: &'a [Value],
    event_name: &str,
    predicate: impl Fn(&Value) -> bool,
) -> &'a Value {
    records
        .iter()
        .find(|record| record["event_name"] == event_name && predicate(record))
        .unwrap_or_else(|| panic!("missing telemetry event {event_name}"))
}

fn find_span<'a>(record: &'a Value, name: &str) -> Option<&'a Value> {
    record["spans"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|span| span["name"] == name)
        .or_else(|| (record["span"]["name"] == name).then_some(&record["span"]))
}

fn assert_span_path(record: &Value, names: &[&str]) {
    for name in names {
        assert!(
            find_span(record, name).is_some(),
            "missing span {name}: {record}"
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    const TOOL_EXECUTION_1: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9a01";
    const TOOL_EXECUTION_2: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9a02";

    #[test]
    fn first_answer_changes_with_model_visible_tool_payload() {
        let first = fixture_tool_context("FixtureOS", "git version 1.2.3", "file-one\n");
        let changed = fixture_tool_context("ChangedOS", "git version 1.2.3", "file-two\n");
        let first_answer = derive_first_answer(&first).unwrap().answer();
        let changed_answer = derive_first_answer(&changed).unwrap().answer();
        assert_ne!(first_answer, changed_answer);
        assert!(first_answer.contains("OS: FixtureOS"));
        assert!(changed_answer.contains("OS: ChangedOS"));
        assert_ne!(
            first_answer.lines().last().unwrap(),
            changed_answer.lines().last().unwrap(),
            "file-result payload must affect the derived answer"
        );
    }

    #[test]
    fn follow_up_requires_consistent_durable_prior_context() {
        let tools = fixture_tool_context("FixtureOS", "git version 1.2.3", "file-one\n");
        let first = derive_first_answer(&tools).unwrap();
        let mut valid = tools.clone();
        valid.push(
            ModelInputItem::prior_assistant(vec![ModelTextPart::try_new(first.answer()).unwrap()])
                .unwrap(),
        );
        valid.push(user_message(FOLLOW_UP));
        assert_eq!(
            derive_follow_up_answer(&valid).unwrap().git_version,
            "git version 1.2.3"
        );

        let mut absent = valid.clone();
        absent.retain(|item| !matches!(item, ModelInputItem::PriorAssistant { .. }));
        assert!(derive_follow_up_answer(&absent).is_err());

        let mut wrong = tools;
        wrong.push(
            ModelInputItem::prior_assistant(vec![
                ModelTextPart::try_new(first.answer().replace("1.2.3", "9.9.9")).unwrap(),
            ])
            .unwrap(),
        );
        wrong.push(user_message(FOLLOW_UP));
        assert!(derive_follow_up_answer(&wrong).is_err());
    }

    #[test]
    fn normalized_tool_link_rejects_material_source_model_mutation() {
        let aliases = BTreeMap::from([(
            "model-a".to_owned(),
            "WORK_1_MODEL_STEP_1_ATTEMPT_1".to_owned(),
        )]);
        let mut tool = json!({"source_model_invocation_id": "model-a"});
        assert_eq!(
            normalized_tool_source_alias(&tool, &aliases).unwrap(),
            "WORK_1_MODEL_STEP_1_ATTEMPT_1"
        );
        tool["source_model_invocation_id"] = json!("model-b");
        assert!(normalized_tool_source_alias(&tool, &aliases).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_path_guard_kills_and_reaps_owned_runtime_child() {
        let root_path = Stage18Root::new("stage24-cleanup-regression").preserve();
        let root = Stage18Root::from_existing(root_path.clone());
        fs::create_dir(root.path().join("credentials")).unwrap();
        fs::set_permissions(
            root.path().join("credentials"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_configuration(&root);
        drop(root);

        let mut guard = Stage24ScenarioGuard::new(root_path.clone());
        guard.spawn("first");
        let _: ReadyRecord =
            wait_json(&root_path.join("ready-first.json"), guard.child_mut()).await;
        let pid = i32::try_from(guard.child_mut().id()).unwrap();
        let panic = catch_unwind(AssertUnwindSafe(move || {
            let _owned = guard;
            panic!("deliberately induced Stage 24 harness failure");
        }));
        assert!(panic.is_err());
        assert!(!root_path.exists(), "failed scenario root survived cleanup");
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: signal zero observes only the exact PID formerly owned and reaped by the guard.
        assert_ne!(
            unsafe { kill(pid, 0) },
            0,
            "owned runtime child remained alive"
        );
    }

    fn fixture_tool_context(os: &str, git: &str, file: &str) -> Vec<ModelInputItem> {
        let shell_call = ModelToolCallId::try_new(SHELL_CALL_ID).unwrap();
        let read_call = ModelToolCallId::try_new(READ_CALL_ID).unwrap();
        vec![
            user_message(CANONICAL_REQUEST),
            ModelInputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    shell_call.clone(),
                    "run_shell",
                    r#"{"command":"fixture"}"#,
                )
                .unwrap(),
            ),
            ModelInputItem::tool_result(
                shell_call,
                tool_result(
                    SHELL_CALL_ID,
                    "run_shell",
                    TOOL_EXECUTION_1,
                    "stdout",
                    &format!(
                        "{os}\nfixture-arch\n/fixture/workspace\n{git}\ncredential_canary=absent\n"
                    ),
                ),
            )
            .unwrap(),
            ModelInputItem::ToolCall(
                CanonicalModelToolCall::try_new(
                    read_call.clone(),
                    "read_file",
                    r#"{"path":"machine-note.txt"}"#,
                )
                .unwrap(),
            ),
            ModelInputItem::tool_result(
                read_call,
                tool_result(READ_CALL_ID, "read_file", TOOL_EXECUTION_2, "text", file),
            )
            .unwrap(),
        ]
    }

    fn tool_result(
        call_id: &str,
        tool_name: &str,
        tool_execution_id: &str,
        field_prefix: &str,
        value: &str,
    ) -> Value {
        json!({
            "provider_tool_call_id": call_id,
            "tool_execution_id": tool_execution_id,
            "tool_name": tool_name,
            "result": {
                "result_kind": "success",
                "fields": [[format!("{field_prefix}_000001"), value]]
            }
        })
    }

    fn user_message(value: &str) -> ModelInputItem {
        ModelInputItem::message(
            ModelInputRole::User,
            vec![ModelTextPart::try_new(value).unwrap()],
        )
        .unwrap()
    }
}
