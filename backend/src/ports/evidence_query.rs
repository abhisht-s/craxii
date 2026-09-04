//! Read-only, redacted operational evidence query boundary.

use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;

use serde::Serialize;

use crate::domain::{RuntimeInstanceId, WorkId};
use crate::ports::artifact_store::ArtifactStore;

pub type EvidenceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, EvidenceQueryError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceQueryErrorKind {
    NotFound,
    Storage,
    Integrity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceQueryError(EvidenceQueryErrorKind);

impl EvidenceQueryError {
    #[must_use]
    pub const fn new(kind: EvidenceQueryErrorKind) -> Self {
        Self(kind)
    }

    #[must_use]
    pub const fn kind(self) -> EvidenceQueryErrorKind {
        self.0
    }
}

impl Display for EvidenceQueryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            EvidenceQueryErrorKind::NotFound => "evidence target not found",
            EvidenceQueryErrorKind::Storage => "evidence query storage failure",
            EvidenceQueryErrorKind::Integrity => "evidence integrity failure",
        })
    }
}

impl std::error::Error for EvidenceQueryError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidencePreflight {
    pub schema_version: u64,
    pub database_disposition: &'static str,
    pub journal_head: Option<u64>,
    pub work_count: u64,
    pub runtime_count: u64,
    pub model_attempt_count: u64,
    pub tool_execution_count: u64,
    pub artifact_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationIssue {
    JournalProjectionInconsistent,
    ReferencedArtifactMissingOrCorrupt,
    ArtifactMetadataInconsistent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StateVerification {
    pub consistent: bool,
    pub checked_invariants: Option<u64>,
    pub journal_head: Option<u64>,
    pub referenced_artifact_count: u64,
    pub verified_artifact_count: u64,
    pub issues: Vec<VerificationIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JournalObservation {
    pub journal_offset: u64,
    pub event_id: String,
    pub stream_sequence: u64,
    pub event_type: String,
    pub event_version: u64,
    pub causation_event_id: Option<String>,
    pub correlation_id: String,
    pub runtime_instance_id: Option<String>,
    pub payload_sha256: String,
    pub recorded_at: String,
    pub occurred_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContextObservation {
    pub context_manifest_id: String,
    pub logical_invocation_id: String,
    pub target: String,
    pub provider: String,
    pub model: String,
    pub target_configuration_version: u64,
    pub assembler_version: String,
    pub context_policy_version: String,
    pub source_count: u64,
    pub canonical_byte_count: u64,
    pub rendered_request_byte_count: u64,
    pub estimated_input_tokens: u64,
    pub token_estimator_id: String,
    pub context_window_tokens: u64,
    pub reserved_output_tokens: u64,
    pub utilization_basis_points: u64,
    pub manifest_sha256: String,
    pub rendered_request_sha256: String,
    pub rendered_request_artifact_id: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelAttemptObservation {
    pub model_invocation_id: String,
    pub logical_invocation_id: String,
    pub runtime_instance_id: String,
    pub context_manifest_id: String,
    pub agent_step: u64,
    pub attempt: u64,
    pub retry_of_invocation_id: Option<String>,
    pub target: String,
    pub provider: String,
    pub model: String,
    pub selection_reason: String,
    pub state: String,
    pub request_sha256: String,
    pub response_sha256: Option<String>,
    pub request_artifact_id: Option<String>,
    pub response_artifact_id: Option<String>,
    pub started_at: String,
    pub first_byte_at: Option<String>,
    pub first_output_at: Option<String>,
    pub completed_at: Option<String>,
    pub usage_status: String,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub stop_reason: Option<String>,
    pub tool_call_count: Option<u64>,
    pub draft_exposed: bool,
    pub provider_request_digest: Option<String>,
    pub provider_response_digest: Option<String>,
    pub provider_error_kind: Option<String>,
    pub provider_outcome_certainty: Option<String>,
    pub retry_reason: Option<String>,
    pub retry_delay_ms: Option<u64>,
    pub provider_retry_after_ms: Option<u64>,
    pub billing_ambiguity: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolExecutionObservation {
    pub tool_execution_id: String,
    pub workstation_execution_id: String,
    pub source_model_invocation_id: String,
    pub runtime_instance_id: String,
    pub agent_step: u64,
    pub tool_ordinal: u64,
    pub tool_name: String,
    pub tool_version: String,
    pub tool_schema_version: u64,
    pub arguments_sha256: String,
    pub workstation_id: String,
    pub workstation_generation: u64,
    pub workspace_id: String,
    pub requested_privilege: String,
    pub effective_privilege: Option<String>,
    pub timeout_ms: u64,
    pub state: String,
    pub dispatch_intent_at: Option<String>,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub result_class: Option<String>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: Option<bool>,
    pub cancelled: Option<bool>,
    pub cleanup_confirmed: Option<bool>,
    pub stdout_artifact_id: Option<String>,
    pub stderr_artifact_id: Option<String>,
    pub stdout_observed_bytes: Option<u64>,
    pub stdout_captured_bytes: Option<u64>,
    pub stderr_observed_bytes: Option<u64>,
    pub stderr_captured_bytes: Option<u64>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactObservation {
    pub artifact_id: String,
    pub producing_work_id: Option<String>,
    pub producer_kind: Option<String>,
    pub producer_id: Option<String>,
    pub storage_key: String,
    pub sha256: String,
    pub captured_byte_count: u64,
    pub observed_byte_count: Option<u64>,
    pub retention_class: String,
    pub truncated: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkEvidence {
    pub work_id: String,
    pub craxii_id: String,
    pub conversation_id: String,
    pub conversation_work_ordinal: u64,
    pub workspace_id: String,
    pub correlation_id: String,
    pub state: String,
    pub state_version: u64,
    pub runtime_instance_id: Option<String>,
    pub current_model_invocation_id: Option<String>,
    pub current_tool_execution_id: Option<String>,
    pub created_at: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub cancel_requested_at: Option<String>,
    pub cancellation_reason_code: Option<String>,
    pub terminal_at: Option<String>,
    pub terminal_reason_code: Option<String>,
    pub journal: Vec<JournalObservation>,
    pub contexts: Vec<ContextObservation>,
    pub model_attempts: Vec<ModelAttemptObservation>,
    pub tool_executions: Vec<ToolExecutionObservation>,
    pub artifacts: Vec<ArtifactObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecoveryObservation {
    pub journal_offset: u64,
    pub recorded_at: String,
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
pub struct RuntimeEvidence {
    pub runtime_instance_id: String,
    pub craxii_id: String,
    pub workstation_id: String,
    pub workstation_generation: u64,
    pub binary_version: String,
    pub git_revision: String,
    pub schema_version: u64,
    pub state: String,
    pub started_at: String,
    pub last_heartbeat_at: Option<String>,
    pub stopped_at: Option<String>,
    pub stop_reason: Option<String>,
    pub owned_work_count: u64,
    pub model_attempt_count: u64,
    pub tool_execution_count: u64,
    pub journal: Vec<JournalObservation>,
    pub recovery: Vec<RecoveryObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceExport {
    pub preflight: EvidencePreflight,
    pub verification: StateVerification,
    pub works: Vec<WorkEvidence>,
    pub runtimes: Vec<RuntimeEvidence>,
}

pub trait EvidenceQueryStore: Send + Sync {
    fn preflight(&self) -> EvidenceFuture<'_, EvidencePreflight>;
    fn verify_state<'a>(
        &'a self,
        artifacts: &'a dyn ArtifactStore,
    ) -> EvidenceFuture<'a, StateVerification>;
    fn inspect_work(&self, work_id: WorkId) -> EvidenceFuture<'_, WorkEvidence>;
    fn inspect_runtime(&self, runtime_id: RuntimeInstanceId)
    -> EvidenceFuture<'_, RuntimeEvidence>;
    fn export<'a>(&'a self, artifacts: &'a dyn ArtifactStore)
    -> EvidenceFuture<'a, EvidenceExport>;
}
