//! Test-only deterministic crash-boundary foundation.
//!
//! This module is absent unless the non-default `test-failpoints` feature is
//! explicitly enabled. Architecture failpoints are registered here but remain
//! reserved until their owning stages install reviewed product hooks.

use std::fmt::{Debug, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

pub const CONTROL_ARGUMENT: &str = "--test-failpoint-control-v1";
pub const CONTROL_PROTOCOL: &str = "CRAXII_TEST_CONTROL_V1";
pub const MARKER_PROTOCOL: &str = "craxii.failpoint-marker.v1";
pub const MARKER_FILE_DESCRIPTOR: i32 = 198;
pub const MAX_CONTROL_BYTES: usize = 512;
pub const MAX_MARKER_BYTES: usize = 1_024;
pub const DUMMY_FILE_NAME: &str = "dummy-durable-file.bin";
pub const DUMMY_BYTES: &[u8] = b"craxii-failpoint-dummy-v1\n";

const TEMP_FILE_NAME: &str = "dummy-durable-file.pending";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailpointName {
    AfterMessageTransactionCommit,
    AfterWorkClaimCommit,
    AfterContextManifestCommit,
    AfterModelIntentCommit,
    AfterFirstProviderDelta,
    AfterModelResponseCommit,
    AfterToolRequestedCommit,
    AfterToolDispatchIntentCommit,
    AfterToolProcessSpawn,
    AfterToolProcessExitBeforeOutcomeCommit,
    AfterArtifactRenameBeforeDbCommit,
    AfterAssistantMessageCommit,
    AfterCancelRequestedCommit,
    DuringGracefulShutdown,
}

impl FailpointName {
    pub const ALL: [Self; 14] = [
        Self::AfterMessageTransactionCommit,
        Self::AfterWorkClaimCommit,
        Self::AfterContextManifestCommit,
        Self::AfterModelIntentCommit,
        Self::AfterFirstProviderDelta,
        Self::AfterModelResponseCommit,
        Self::AfterToolRequestedCommit,
        Self::AfterToolDispatchIntentCommit,
        Self::AfterToolProcessSpawn,
        Self::AfterToolProcessExitBeforeOutcomeCommit,
        Self::AfterArtifactRenameBeforeDbCommit,
        Self::AfterAssistantMessageCommit,
        Self::AfterCancelRequestedCommit,
        Self::DuringGracefulShutdown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AfterMessageTransactionCommit => "after_message_transaction_commit",
            Self::AfterWorkClaimCommit => "after_work_claim_commit",
            Self::AfterContextManifestCommit => "after_context_manifest_commit",
            Self::AfterModelIntentCommit => "after_model_intent_commit",
            Self::AfterFirstProviderDelta => "after_first_provider_delta",
            Self::AfterModelResponseCommit => "after_model_response_commit",
            Self::AfterToolRequestedCommit => "after_tool_requested_commit",
            Self::AfterToolDispatchIntentCommit => "after_tool_dispatch_intent_commit",
            Self::AfterToolProcessSpawn => "after_tool_process_spawn",
            Self::AfterToolProcessExitBeforeOutcomeCommit => {
                "after_tool_process_exit_before_outcome_commit"
            }
            Self::AfterArtifactRenameBeforeDbCommit => "after_artifact_rename_before_db_commit",
            Self::AfterAssistantMessageCommit => "after_assistant_message_commit",
            Self::AfterCancelRequestedCommit => "after_cancel_requested_commit",
            Self::DuringGracefulShutdown => "during_graceful_shutdown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, NameParseError> {
        match value {
            "after_message_transaction_commit" => Ok(Self::AfterMessageTransactionCommit),
            "after_work_claim_commit" => Ok(Self::AfterWorkClaimCommit),
            "after_context_manifest_commit" => Ok(Self::AfterContextManifestCommit),
            "after_model_intent_commit" => Ok(Self::AfterModelIntentCommit),
            "after_first_provider_delta" => Ok(Self::AfterFirstProviderDelta),
            "after_model_response_commit" => Ok(Self::AfterModelResponseCommit),
            "after_tool_requested_commit" => Ok(Self::AfterToolRequestedCommit),
            "after_tool_dispatch_intent_commit" => Ok(Self::AfterToolDispatchIntentCommit),
            "after_tool_process_spawn" => Ok(Self::AfterToolProcessSpawn),
            "after_tool_process_exit_before_outcome_commit" => {
                Ok(Self::AfterToolProcessExitBeforeOutcomeCommit)
            }
            "after_artifact_rename_before_db_commit" => Ok(Self::AfterArtifactRenameBeforeDbCommit),
            "after_assistant_message_commit" => Ok(Self::AfterAssistantMessageCommit),
            "after_cancel_requested_commit" => Ok(Self::AfterCancelRequestedCommit),
            "during_graceful_shutdown" => Ok(Self::DuringGracefulShutdown),
            _ => Err(NameParseError::UnknownArchitectureName),
        }
    }
}

impl Display for FailpointName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicPhysicalHook {
    ModelAttemptAfterManifestRowsBeforeIntent,
    ModelAttemptAfterAllRowsBeforeCommit,
    ModelAttemptAfterCommitBeforeProviderIo,
    FinalAnswerAfterAllRowsBeforeCommit,
    FinalAnswerAfterCommitBeforeNotification,
}

impl AtomicPhysicalHook {
    pub const ALL: [Self; 5] = [
        Self::ModelAttemptAfterManifestRowsBeforeIntent,
        Self::ModelAttemptAfterAllRowsBeforeCommit,
        Self::ModelAttemptAfterCommitBeforeProviderIo,
        Self::FinalAnswerAfterAllRowsBeforeCommit,
        Self::FinalAnswerAfterCommitBeforeNotification,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelAttemptAfterManifestRowsBeforeIntent => {
                "model_attempt_after_manifest_rows_before_intent"
            }
            Self::ModelAttemptAfterAllRowsBeforeCommit => {
                "model_attempt_after_all_rows_before_commit"
            }
            Self::ModelAttemptAfterCommitBeforeProviderIo => {
                "model_attempt_after_commit_before_provider_io"
            }
            Self::FinalAnswerAfterAllRowsBeforeCommit => {
                "final_answer_after_all_rows_before_commit"
            }
            Self::FinalAnswerAfterCommitBeforeNotification => {
                "final_answer_after_commit_before_notification"
            }
        }
    }

    pub fn parse(value: &str) -> Result<Self, NameParseError> {
        match value {
            "model_attempt_after_manifest_rows_before_intent" => {
                Ok(Self::ModelAttemptAfterManifestRowsBeforeIntent)
            }
            "model_attempt_after_all_rows_before_commit" => {
                Ok(Self::ModelAttemptAfterAllRowsBeforeCommit)
            }
            "model_attempt_after_commit_before_provider_io" => {
                Ok(Self::ModelAttemptAfterCommitBeforeProviderIo)
            }
            "final_answer_after_all_rows_before_commit" => {
                Ok(Self::FinalAnswerAfterAllRowsBeforeCommit)
            }
            "final_answer_after_commit_before_notification" => {
                Ok(Self::FinalAnswerAfterCommitBeforeNotification)
            }
            _ => Err(NameParseError::UnknownPhysicalHook),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundationHook {
    BeforeDummyRename,
    AfterDummyRename,
}

impl FoundationHook {
    pub const ALL: [Self; 2] = [Self::BeforeDummyRename, Self::AfterDummyRename];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeDummyRename => "foundation_before_dummy_rename",
            Self::AfterDummyRename => "foundation_after_dummy_rename",
        }
    }

    fn parse(value: &str) -> Result<Self, NameParseError> {
        match value {
            "foundation_before_dummy_rename" => Ok(Self::BeforeDummyRename),
            "foundation_after_dummy_rename" => Ok(Self::AfterDummyRename),
            _ => Err(NameParseError::UnknownPhysicalHook),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalHook {
    AfterMessageTransactionCommit,
    AfterWorkClaimCommit,
    AfterFirstProviderDelta,
    AfterModelResponseCommit,
    AfterToolRequestedCommit,
    AfterToolDispatchIntentCommit,
    AfterToolProcessSpawn,
    AfterToolProcessExitBeforeOutcomeCommit,
    AfterArtifactRenameBeforeDbCommit,
    AfterCancelRequestedCommit,
    DuringGracefulShutdown,
    ModelAttemptAfterManifestRowsBeforeIntent,
    ModelAttemptAfterAllRowsBeforeCommit,
    ModelAttemptAfterCommitBeforeProviderIo,
    FinalAnswerAfterAllRowsBeforeCommit,
    FinalAnswerAfterCommitBeforeNotification,
    FoundationBeforeDummyRename,
    FoundationAfterDummyRename,
}

impl PhysicalHook {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AfterMessageTransactionCommit => "after_message_transaction_commit",
            Self::AfterWorkClaimCommit => "after_work_claim_commit",
            Self::AfterFirstProviderDelta => "after_first_provider_delta",
            Self::AfterModelResponseCommit => "after_model_response_commit",
            Self::AfterToolRequestedCommit => "after_tool_requested_commit",
            Self::AfterToolDispatchIntentCommit => "after_tool_dispatch_intent_commit",
            Self::AfterToolProcessSpawn => "after_tool_process_spawn",
            Self::AfterToolProcessExitBeforeOutcomeCommit => {
                "after_tool_process_exit_before_outcome_commit"
            }
            Self::AfterArtifactRenameBeforeDbCommit => "after_artifact_rename_before_db_commit",
            Self::AfterCancelRequestedCommit => "after_cancel_requested_commit",
            Self::DuringGracefulShutdown => "during_graceful_shutdown",
            Self::ModelAttemptAfterManifestRowsBeforeIntent => {
                "model_attempt_after_manifest_rows_before_intent"
            }
            Self::ModelAttemptAfterAllRowsBeforeCommit => {
                "model_attempt_after_all_rows_before_commit"
            }
            Self::ModelAttemptAfterCommitBeforeProviderIo => {
                "model_attempt_after_commit_before_provider_io"
            }
            Self::FinalAnswerAfterAllRowsBeforeCommit => {
                "final_answer_after_all_rows_before_commit"
            }
            Self::FinalAnswerAfterCommitBeforeNotification => {
                "final_answer_after_commit_before_notification"
            }
            Self::FoundationBeforeDummyRename => "foundation_before_dummy_rename",
            Self::FoundationAfterDummyRename => "foundation_after_dummy_rename",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitSide {
    Before,
    After,
    None,
}

impl CommitSide {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoSide {
    Before,
    AfterObserved,
    None,
}

impl IoSide {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::AfterObserved => "after_observed",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableClassification {
    TransactionCommitted,
    TransactionRolledBack,
    NoncanonicalDraftOnly,
    DurableIntentCommitted,
    OutcomeUnknown,
    OrphanArtifactPossible,
    CancellationRequested,
    CleanupInProgress,
    DummyFinalAbsent,
    DummyFinalPresent,
}

impl DurableClassification {
    pub const ALL: [Self; 10] = [
        Self::TransactionCommitted,
        Self::TransactionRolledBack,
        Self::NoncanonicalDraftOnly,
        Self::DurableIntentCommitted,
        Self::OutcomeUnknown,
        Self::OrphanArtifactPossible,
        Self::CancellationRequested,
        Self::CleanupInProgress,
        Self::DummyFinalAbsent,
        Self::DummyFinalPresent,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransactionCommitted => "transaction_committed",
            Self::TransactionRolledBack => "transaction_rolled_back",
            Self::NoncanonicalDraftOnly => "noncanonical_draft_only",
            Self::DurableIntentCommitted => "durable_intent_committed",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::OrphanArtifactPossible => "orphan_artifact_possible",
            Self::CancellationRequested => "cancellation_requested",
            Self::CleanupInProgress => "cleanup_in_progress",
            Self::DummyFinalAbsent => "dummy_final_absent",
            Self::DummyFinalPresent => "dummy_final_present",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "transaction_committed" => Some(Self::TransactionCommitted),
            "transaction_rolled_back" => Some(Self::TransactionRolledBack),
            "noncanonical_draft_only" => Some(Self::NoncanonicalDraftOnly),
            "durable_intent_committed" => Some(Self::DurableIntentCommitted),
            "outcome_unknown" => Some(Self::OutcomeUnknown),
            "orphan_artifact_possible" => Some(Self::OrphanArtifactPossible),
            "cancellation_requested" => Some(Self::CancellationRequested),
            "cleanup_in_progress" => Some(Self::CleanupInProgress),
            "dummy_final_absent" => Some(Self::DummyFinalAbsent),
            "dummy_final_present" => Some(Self::DummyFinalPresent),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BoundaryMetadata {
    pub commit_side: CommitSide,
    pub io_side: IoSide,
    pub cleanup_phase: bool,
    pub expected_durable_classification: DurableClassification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwningStage {
    Stage8,
    Stage9,
    Stage10,
    Stage13,
    Stage14,
    Stage17,
    Stage19,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    Reserved,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PhysicalBoundarySpec {
    pub physical_hook: PhysicalHook,
    pub boundary: BoundaryMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct FailpointSpec {
    pub architecture_name: FailpointName,
    pub physical_boundaries: &'static [PhysicalBoundarySpec],
    pub owning_stage: OwningStage,
    pub status: HookStatus,
}

const fn boundary(
    commit_side: CommitSide,
    io_side: IoSide,
    cleanup_phase: bool,
    expected_durable_classification: DurableClassification,
) -> BoundaryMetadata {
    BoundaryMetadata {
        commit_side,
        io_side,
        cleanup_phase,
        expected_durable_classification,
    }
}

const MESSAGE: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterMessageTransactionCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::None,
        false,
        DurableClassification::TransactionCommitted,
    ),
}];
const WORK_CLAIM: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterWorkClaimCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::None,
        false,
        DurableClassification::TransactionCommitted,
    ),
}];
const CONTEXT_MANIFEST: [PhysicalBoundarySpec; 2] = [
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::ModelAttemptAfterManifestRowsBeforeIntent,
        boundary: boundary(
            CommitSide::Before,
            IoSide::None,
            false,
            DurableClassification::TransactionRolledBack,
        ),
    },
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::ModelAttemptAfterCommitBeforeProviderIo,
        boundary: boundary(
            CommitSide::After,
            IoSide::Before,
            false,
            DurableClassification::DurableIntentCommitted,
        ),
    },
];
const MODEL_INTENT: [PhysicalBoundarySpec; 2] = [
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::ModelAttemptAfterAllRowsBeforeCommit,
        boundary: boundary(
            CommitSide::Before,
            IoSide::None,
            false,
            DurableClassification::TransactionRolledBack,
        ),
    },
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::ModelAttemptAfterCommitBeforeProviderIo,
        boundary: boundary(
            CommitSide::After,
            IoSide::Before,
            false,
            DurableClassification::DurableIntentCommitted,
        ),
    },
];
const PROVIDER_DELTA: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterFirstProviderDelta,
    boundary: boundary(
        CommitSide::None,
        IoSide::AfterObserved,
        false,
        DurableClassification::NoncanonicalDraftOnly,
    ),
}];
const MODEL_RESPONSE: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterModelResponseCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::None,
        false,
        DurableClassification::TransactionCommitted,
    ),
}];
const TOOL_REQUESTED: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterToolRequestedCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::None,
        false,
        DurableClassification::TransactionCommitted,
    ),
}];
const TOOL_DISPATCH: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterToolDispatchIntentCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::Before,
        false,
        DurableClassification::OutcomeUnknown,
    ),
}];
const PROCESS_SPAWN: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterToolProcessSpawn,
    boundary: boundary(
        CommitSide::None,
        IoSide::AfterObserved,
        false,
        DurableClassification::OutcomeUnknown,
    ),
}];
const PROCESS_EXIT: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterToolProcessExitBeforeOutcomeCommit,
    boundary: boundary(
        CommitSide::Before,
        IoSide::AfterObserved,
        false,
        DurableClassification::OutcomeUnknown,
    ),
}];
const ARTIFACT_RENAME: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterArtifactRenameBeforeDbCommit,
    boundary: boundary(
        CommitSide::Before,
        IoSide::AfterObserved,
        false,
        DurableClassification::OrphanArtifactPossible,
    ),
}];
const FINAL_ANSWER: [PhysicalBoundarySpec; 2] = [
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::FinalAnswerAfterAllRowsBeforeCommit,
        boundary: boundary(
            CommitSide::Before,
            IoSide::None,
            false,
            DurableClassification::TransactionRolledBack,
        ),
    },
    PhysicalBoundarySpec {
        physical_hook: PhysicalHook::FinalAnswerAfterCommitBeforeNotification,
        boundary: boundary(
            CommitSide::After,
            IoSide::Before,
            false,
            DurableClassification::TransactionCommitted,
        ),
    },
];
const CANCELLATION: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::AfterCancelRequestedCommit,
    boundary: boundary(
        CommitSide::After,
        IoSide::None,
        false,
        DurableClassification::CancellationRequested,
    ),
}];
const SHUTDOWN: [PhysicalBoundarySpec; 1] = [PhysicalBoundarySpec {
    physical_hook: PhysicalHook::DuringGracefulShutdown,
    boundary: boundary(
        CommitSide::None,
        IoSide::None,
        true,
        DurableClassification::CleanupInProgress,
    ),
}];

pub static REGISTRY: [FailpointSpec; 14] = [
    active_spec(
        FailpointName::AfterMessageTransactionCommit,
        &MESSAGE,
        OwningStage::Stage10,
    ),
    active_spec(
        FailpointName::AfterWorkClaimCommit,
        &WORK_CLAIM,
        OwningStage::Stage10,
    ),
    spec(
        FailpointName::AfterContextManifestCommit,
        &CONTEXT_MANIFEST,
        OwningStage::Stage17,
    ),
    spec(
        FailpointName::AfterModelIntentCommit,
        &MODEL_INTENT,
        OwningStage::Stage17,
    ),
    spec(
        FailpointName::AfterFirstProviderDelta,
        &PROVIDER_DELTA,
        OwningStage::Stage19,
    ),
    spec(
        FailpointName::AfterModelResponseCommit,
        &MODEL_RESPONSE,
        OwningStage::Stage17,
    ),
    spec(
        FailpointName::AfterToolRequestedCommit,
        &TOOL_REQUESTED,
        OwningStage::Stage14,
    ),
    spec(
        FailpointName::AfterToolDispatchIntentCommit,
        &TOOL_DISPATCH,
        OwningStage::Stage14,
    ),
    spec(
        FailpointName::AfterToolProcessSpawn,
        &PROCESS_SPAWN,
        OwningStage::Stage13,
    ),
    spec(
        FailpointName::AfterToolProcessExitBeforeOutcomeCommit,
        &PROCESS_EXIT,
        OwningStage::Stage13,
    ),
    spec(
        FailpointName::AfterArtifactRenameBeforeDbCommit,
        &ARTIFACT_RENAME,
        OwningStage::Stage8,
    ),
    spec(
        FailpointName::AfterAssistantMessageCommit,
        &FINAL_ANSWER,
        OwningStage::Stage17,
    ),
    active_spec(
        FailpointName::AfterCancelRequestedCommit,
        &CANCELLATION,
        OwningStage::Stage10,
    ),
    active_spec(
        FailpointName::DuringGracefulShutdown,
        &SHUTDOWN,
        OwningStage::Stage10,
    ),
];

const fn spec(
    architecture_name: FailpointName,
    physical_boundaries: &'static [PhysicalBoundarySpec],
    owning_stage: OwningStage,
) -> FailpointSpec {
    FailpointSpec {
        architecture_name,
        physical_boundaries,
        owning_stage,
        status: HookStatus::Reserved,
    }
}

const fn active_spec(
    architecture_name: FailpointName,
    physical_boundaries: &'static [PhysicalBoundarySpec],
    owning_stage: OwningStage,
) -> FailpointSpec {
    FailpointSpec {
        architecture_name,
        physical_boundaries,
        owning_stage,
        status: HookStatus::Active,
    }
}

pub fn resolve_architecture_alias(
    architecture_name: &str,
    requested_hook: Option<&str>,
) -> Result<PhysicalBoundarySpec, ResolutionError> {
    let architecture_name =
        FailpointName::parse(architecture_name).map_err(|_| ResolutionError::UnknownName)?;
    let failpoint = REGISTRY
        .iter()
        .find(|candidate| candidate.architecture_name == architecture_name)
        .ok_or(ResolutionError::UnknownName)?;

    match requested_hook {
        None if failpoint.physical_boundaries.len() != 1 => Err(ResolutionError::AmbiguousAlias),
        None => Ok(failpoint.physical_boundaries[0]),
        Some(requested) => failpoint
            .physical_boundaries
            .iter()
            .copied()
            .find(|candidate| candidate.physical_hook.as_str() == requested)
            .ok_or_else(|| {
                if AtomicPhysicalHook::parse(requested).is_err() {
                    ResolutionError::UnknownPhysicalHook
                } else {
                    ResolutionError::IncompatiblePhysicalHook
                }
            }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameParseError {
    UnknownArchitectureName,
    UnknownPhysicalHook,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionError {
    UnknownName,
    UnknownPhysicalHook,
    AmbiguousAlias,
    IncompatiblePhysicalHook,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSelection {
    pub architecture_name: Option<FailpointName>,
    pub physical_hook: PhysicalHook,
    pub boundary: BoundaryMetadata,
}

impl ControlSelection {
    pub const fn foundation(hook: FoundationHook) -> Self {
        let (physical_hook, boundary) = match hook {
            FoundationHook::BeforeDummyRename => (
                PhysicalHook::FoundationBeforeDummyRename,
                boundary(
                    CommitSide::None,
                    IoSide::Before,
                    false,
                    DurableClassification::DummyFinalAbsent,
                ),
            ),
            FoundationHook::AfterDummyRename => (
                PhysicalHook::FoundationAfterDummyRename,
                boundary(
                    CommitSide::None,
                    IoSide::AfterObserved,
                    false,
                    DurableClassification::DummyFinalPresent,
                ),
            ),
        };
        Self {
            architecture_name: None,
            physical_hook,
            boundary,
        }
    }

    pub fn encode(self, run_id: &str) -> Result<String, ControlError> {
        validate_run_id(run_id)?;
        let architecture_name = self.architecture_name.map_or("none", FailpointName::as_str);
        Ok(format!(
            "{CONTROL_PROTOCOL}\trun_id={run_id}\tarchitecture_name={architecture_name}\tphysical_hook={}\n",
            self.physical_hook.as_str()
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActivationRecord {
    run_id: String,
    selection: ControlSelection,
    startup_ready: bool,
}

pub fn parse_control(input: &[u8]) -> Result<ControlSelection, ControlError> {
    parse_activation_record(input).map(|record| record.selection)
}

fn parse_activation_record(input: &[u8]) -> Result<ActivationRecord, ControlError> {
    if input.is_empty() {
        return Err(ControlError::ZeroSelections);
    }
    if input.len() > MAX_CONTROL_BYTES {
        return Err(ControlError::ControlTooLarge);
    }
    let input = std::str::from_utf8(input).map_err(|_| ControlError::MalformedControl)?;
    if !input.ends_with('\n') {
        return Err(ControlError::MalformedControl);
    }
    let records: Vec<_> = input
        .strip_suffix('\n')
        .unwrap_or_default()
        .split('\n')
        .collect();
    if records.len() > 1 {
        return Err(ControlError::MultipleSelections);
    }
    let fields: Vec<_> = records[0].split('\t').collect();
    if fields.len() != 4 || fields[0] != CONTROL_PROTOCOL {
        return Err(ControlError::MalformedControl);
    }
    let run_id = exact_field(fields[1], "run_id=")?;
    validate_run_id(run_id)?;
    let architecture = exact_field(fields[2], "architecture_name=")?;
    let physical = exact_field(fields[3], "physical_hook=")?;

    if architecture == "none" {
        let hook =
            FoundationHook::parse(physical).map_err(|_| ControlError::UnknownPhysicalHook)?;
        return Ok(ActivationRecord {
            run_id: run_id.to_owned(),
            selection: ControlSelection::foundation(hook),
            startup_ready: false,
        });
    }

    let architecture_name =
        FailpointName::parse(architecture).map_err(|_| ControlError::UnknownArchitectureName)?;
    let requested_hook = (physical != "none").then_some(physical);
    let resolved =
        resolve_architecture_alias(architecture, requested_hook).map_err(|error| match error {
            ResolutionError::UnknownName => ControlError::UnknownArchitectureName,
            ResolutionError::UnknownPhysicalHook => ControlError::UnknownPhysicalHook,
            ResolutionError::AmbiguousAlias => ControlError::AmbiguousAlias,
            ResolutionError::IncompatiblePhysicalHook => ControlError::IncompatiblePhysicalHook,
        })?;
    let selection = ControlSelection {
        architecture_name: Some(architecture_name),
        physical_hook: resolved.physical_hook,
        boundary: resolved.boundary,
    };
    let status = REGISTRY
        .iter()
        .find(|candidate| candidate.architecture_name == architecture_name)
        .map(|candidate| candidate.status)
        .ok_or(ControlError::UnknownArchitectureName)?;
    if status == HookStatus::Reserved {
        Err(ControlError::ReservedArchitectureFailpoint)
    } else {
        Ok(ActivationRecord {
            run_id: run_id.to_owned(),
            selection,
            startup_ready: false,
        })
    }
}

fn exact_field<'a>(field: &'a str, prefix: &str) -> Result<&'a str, ControlError> {
    field
        .strip_prefix(prefix)
        .filter(|value| !value.is_empty())
        .ok_or(ControlError::MalformedControl)
}

fn validate_run_id(run_id: &str) -> Result<(), ControlError> {
    if !(5..=64).contains(&run_id.len())
        || !run_id.starts_with("run-")
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ControlError::InvalidRunId);
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ControlError {
    ZeroSelections,
    MultipleSelections,
    ControlTooLarge,
    MalformedControl,
    InvalidRunId,
    UnknownArchitectureName,
    UnknownPhysicalHook,
    AmbiguousAlias,
    IncompatiblePhysicalHook,
    ReservedArchitectureFailpoint,
    AlreadyInitialized,
    MarkerChannel,
    ProbeIo,
}

impl ControlError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ZeroSelections => "zero_selections",
            Self::MultipleSelections => "multiple_selections",
            Self::ControlTooLarge => "control_too_large",
            Self::MalformedControl => "malformed_control",
            Self::InvalidRunId => "invalid_run_id",
            Self::UnknownArchitectureName => "unknown_architecture_name",
            Self::UnknownPhysicalHook => "unknown_physical_hook",
            Self::AmbiguousAlias => "ambiguous_alias",
            Self::IncompatiblePhysicalHook => "incompatible_physical_hook",
            Self::ReservedArchitectureFailpoint => "reserved_architecture_failpoint",
            Self::AlreadyInitialized => "already_initialized",
            Self::MarkerChannel => "marker_channel_failure",
            Self::ProbeIo => "foundation_probe_io_failure",
        }
    }
}

impl Display for ControlError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Debug for ControlError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ControlError {}

struct Activation {
    record: ActivationRecord,
    marker: Mutex<File>,
    marker_claimed: AtomicBool,
}

impl Activation {
    fn new(record: ActivationRecord, marker: File) -> Self {
        Self {
            record,
            marker: Mutex::new(marker),
            marker_claimed: AtomicBool::new(false),
        }
    }

    fn reach(&self, hook: PhysicalHook) {
        if self.record.selection.physical_hook != hook {
            return;
        }

        if !self.marker_claimed.swap(true, Ordering::AcqRel) {
            let marker_result = self
                .marker
                .lock()
                .map_err(|_| io::Error::other("marker lock poisoned"))
                .and_then(|mut marker| {
                    write_marker(&mut *marker, &self.record)?;
                    marker.flush()
                });
            if marker_result.is_err() {
                std::process::abort();
            }
        }

        loop {
            std::thread::park();
        }
    }
}

struct ActivationSlot(OnceLock<Arc<Activation>>);

impl ActivationSlot {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    fn initialize(
        &self,
        record: ActivationRecord,
        marker: File,
    ) -> Result<Arc<Activation>, ControlError> {
        let activation = Arc::new(Activation::new(record, marker));
        self.0
            .set(Arc::clone(&activation))
            .map_err(|_| ControlError::AlreadyInitialized)?;
        Ok(activation)
    }
}

static PROCESS_ACTIVATION: ActivationSlot = ActivationSlot::new();

pub fn reach(hook: PhysicalHook) {
    if let Some(activation) = PROCESS_ACTIVATION.0.get() {
        activation.reach(hook);
    }
    if std::env::var("CRAXII_TEST_ABORT_AT_FAILPOINT")
        .ok()
        .as_deref()
        == Some(hook.as_str())
    {
        std::process::abort();
    }
}

fn write_marker(writer: &mut impl Write, record: &ActivationRecord) -> io::Result<()> {
    let selection = record.selection;
    let architecture = selection
        .architecture_name
        .map_or("null".to_owned(), |name| format!("\"{}\"", name.as_str()));
    writeln!(
        writer,
        concat!(
            "{{\"protocol\":\"{}\",\"run_id\":\"{}\",",
            "\"architecture_name\":{},\"physical_hook\":\"{}\",",
            "\"commit_side\":\"{}\",\"io_side\":\"{}\",",
            "\"cleanup_phase\":{},",
            "\"expected_durable_classification\":\"{}\",",
            "\"sequence\":1,",
            "\"evidence_role\":\"operational_only\",",
            "\"recovery_truth\":false,\"startup_ready\":{}}}"
        ),
        MARKER_PROTOCOL,
        record.run_id,
        architecture,
        selection.physical_hook.as_str(),
        selection.boundary.commit_side.as_str(),
        selection.boundary.io_side.as_str(),
        selection.boundary.cleanup_phase,
        selection.boundary.expected_durable_classification.as_str(),
        record.startup_ready,
    )
}

pub fn foundation_directory(run_id: &str) -> Result<PathBuf, ControlError> {
    validate_run_id(run_id)?;
    Ok(std::env::temp_dir().join(format!("craxii-failpoint-{run_id}")))
}

#[cfg(unix)]
pub fn run_controlled_startup() -> Result<(), ControlError> {
    let mut input = Vec::new();
    std::io::stdin()
        .take((MAX_CONTROL_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .map_err(|_| ControlError::MalformedControl)?;
    let mut record = parse_activation_record(&input)?;
    let health = crate::bootstrap::health::Health::new();
    record.startup_ready = health.snapshot().is_ready();
    let marker = duplicate_marker_file(MARKER_FILE_DESCRIPTOR)?;
    PROCESS_ACTIVATION.initialize(record.clone(), marker)?;
    if record.selection.architecture_name.is_some() {
        reach(record.selection.physical_hook);
        Err(ControlError::ProbeIo)
    } else {
        run_foundation_probe(&record.run_id)
    }
}

fn run_foundation_probe(run_id: &str) -> Result<(), ControlError> {
    let directory = foundation_directory(run_id)?;
    fs::create_dir(&directory).map_err(|_| ControlError::ProbeIo)?;
    let pending = directory.join(TEMP_FILE_NAME);
    let final_path = directory.join(DUMMY_FILE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|_| ControlError::ProbeIo)?;
    file.write_all(DUMMY_BYTES)
        .and_then(|()| file.sync_all())
        .map_err(|_| ControlError::ProbeIo)?;
    drop(file);

    reach(PhysicalHook::FoundationBeforeDummyRename);
    fs::rename(&pending, &final_path).map_err(|_| ControlError::ProbeIo)?;
    File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ControlError::ProbeIo)?;
    reach(PhysicalHook::FoundationAfterDummyRename);
    Err(ControlError::ProbeIo)
}

#[cfg(unix)]
fn duplicate_marker_file(descriptor: RawFd) -> Result<File, ControlError> {
    unsafe extern "C" {
        fn dup(old_descriptor: RawFd) -> RawFd;
    }

    // SAFETY: `dup` accepts any integer descriptor and reports invalid or closed
    // descriptors with `-1`; the returned nonnegative descriptor is newly owned.
    let duplicated = unsafe { dup(descriptor) };
    if duplicated < 0 {
        return Err(ControlError::MarkerChannel);
    }
    // SAFETY: the successful `dup` above returned a fresh descriptor owned by
    // this function, which is transferred exactly once into `File`.
    Ok(unsafe { File::from_raw_fd(duplicated) })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::{Seek as _, SeekFrom};
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;

    fn temporary_file(label: &str) -> (PathBuf, File) {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "craxii-failpoint-unit-{label}-{}-{sequence}",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        (path, file)
    }

    fn activation_record(hook: FoundationHook) -> ActivationRecord {
        ActivationRecord {
            run_id: "run-unit-1".to_owned(),
            selection: ControlSelection::foundation(hook),
            startup_ready: false,
        }
    }

    #[test]
    fn registry_contains_exactly_the_frozen_names_once_each() {
        assert_eq!(REGISTRY.len(), 14);
        let names: HashSet<_> = REGISTRY.iter().map(|spec| spec.architecture_name).collect();
        assert_eq!(names.len(), 14);
        assert_eq!(names, FailpointName::ALL.into_iter().collect());
    }

    #[test]
    fn architecture_names_parse_and_render_exactly() {
        for name in FailpointName::ALL {
            assert_eq!(FailpointName::parse(name.as_str()), Ok(name));
            assert_eq!(name.to_string(), name.as_str());
        }
        assert_eq!(
            FailpointName::parse("After_Message_Transaction_Commit"),
            Err(NameParseError::UnknownArchitectureName)
        );
    }

    #[test]
    fn registry_metadata_is_complete_typed_and_has_exact_stage10_activation() {
        for spec in REGISTRY {
            let active = matches!(
                spec.architecture_name,
                FailpointName::AfterMessageTransactionCommit
                    | FailpointName::AfterWorkClaimCommit
                    | FailpointName::AfterCancelRequestedCommit
                    | FailpointName::DuringGracefulShutdown
            );
            assert_eq!(
                spec.status,
                if active {
                    HookStatus::Active
                } else {
                    HookStatus::Reserved
                }
            );
            assert!(!spec.physical_boundaries.is_empty());
            for physical in spec.physical_boundaries {
                assert!(!physical.physical_hook.as_str().is_empty());
                let _ = physical.boundary.commit_side.as_str();
                let _ = physical.boundary.io_side.as_str();
                let _ = physical.boundary.cleanup_phase;
                let serialized = serde_json::to_value(physical).unwrap();
                assert!(serialized.get("boundary").is_some());
            }
            let _ = serde_json::to_value(spec).unwrap();
        }
    }

    #[test]
    fn atomic_physical_hook_vocabulary_is_exact() {
        assert_eq!(
            AtomicPhysicalHook::ALL.map(AtomicPhysicalHook::as_str),
            [
                "model_attempt_after_manifest_rows_before_intent",
                "model_attempt_after_all_rows_before_commit",
                "model_attempt_after_commit_before_provider_io",
                "final_answer_after_all_rows_before_commit",
                "final_answer_after_commit_before_notification",
            ]
        );
        for hook in AtomicPhysicalHook::ALL {
            assert_eq!(AtomicPhysicalHook::parse(hook.as_str()), Ok(hook));
        }
    }

    #[test]
    fn ambiguous_aliases_require_explicit_compatible_physical_hooks() {
        for alias in [
            FailpointName::AfterContextManifestCommit,
            FailpointName::AfterModelIntentCommit,
            FailpointName::AfterAssistantMessageCommit,
        ] {
            assert_eq!(
                resolve_architecture_alias(alias.as_str(), None),
                Err(ResolutionError::AmbiguousAlias)
            );
        }
        assert_eq!(
            resolve_architecture_alias(
                FailpointName::AfterModelIntentCommit.as_str(),
                Some(AtomicPhysicalHook::ModelAttemptAfterAllRowsBeforeCommit.as_str())
            )
            .unwrap()
            .physical_hook,
            PhysicalHook::ModelAttemptAfterAllRowsBeforeCommit
        );
        assert_eq!(
            resolve_architecture_alias(
                FailpointName::AfterAssistantMessageCommit.as_str(),
                Some(AtomicPhysicalHook::ModelAttemptAfterAllRowsBeforeCommit.as_str())
            ),
            Err(ResolutionError::IncompatiblePhysicalHook)
        );
    }

    #[test]
    fn unknown_names_and_hooks_fail_closed() {
        assert_eq!(
            resolve_architecture_alias("unknown", None),
            Err(ResolutionError::UnknownName)
        );
        assert_eq!(
            resolve_architecture_alias(
                FailpointName::AfterAssistantMessageCommit.as_str(),
                Some("not_a_hook")
            ),
            Err(ResolutionError::UnknownPhysicalHook)
        );
    }

    #[test]
    fn malformed_control_is_redacted_and_zero_or_multiple_records_are_rejected() {
        let sentinel = b"sentinel-secret /private/path Authorization: Bearer token";
        let error = parse_control(sentinel).unwrap_err();
        assert_eq!(error, ControlError::MalformedControl);
        assert!(!format!("{error:?} {error}").contains("sentinel"));
        assert_eq!(parse_control(b""), Err(ControlError::ZeroSelections));

        let record = ControlSelection::foundation(FoundationHook::BeforeDummyRename)
            .encode("run-unit-2")
            .unwrap();
        let two = format!("{record}{record}");
        assert_eq!(
            parse_control(two.as_bytes()),
            Err(ControlError::MultipleSelections)
        );
    }

    #[test]
    fn active_and_reserved_selections_are_distinct_from_unknown_and_ambiguous() {
        let direct = format!(
            "{CONTROL_PROTOCOL}\trun_id=run-unit-3\tarchitecture_name={}\tphysical_hook=none\n",
            FailpointName::AfterWorkClaimCommit.as_str()
        );
        assert!(parse_control(direct.as_bytes()).is_ok());
        let reserved = format!(
            "{CONTROL_PROTOCOL}\trun_id=run-unit-3\tarchitecture_name={}\tphysical_hook=none\n",
            FailpointName::AfterFirstProviderDelta.as_str()
        );
        assert_eq!(
            parse_control(reserved.as_bytes()),
            Err(ControlError::ReservedArchitectureFailpoint)
        );
        let ambiguous = format!(
            "{CONTROL_PROTOCOL}\trun_id=run-unit-3\tarchitecture_name={}\tphysical_hook=none\n",
            FailpointName::AfterModelIntentCommit.as_str()
        );
        assert_eq!(
            parse_control(ambiguous.as_bytes()),
            Err(ControlError::AmbiguousAlias)
        );
        let unknown = format!(
            "{CONTROL_PROTOCOL}\trun_id=run-unit-3\tarchitecture_name=unknown\tphysical_hook=none\n"
        );
        assert_eq!(
            parse_control(unknown.as_bytes()),
            Err(ControlError::UnknownArchitectureName)
        );
    }

    #[test]
    fn activation_slot_initializes_once_and_cannot_rearm() {
        let slot = ActivationSlot::new();
        let (first_path, first) = temporary_file("once-first");
        let (second_path, second) = temporary_file("once-second");
        assert!(
            slot.initialize(activation_record(FoundationHook::BeforeDummyRename), first)
                .is_ok()
        );
        assert!(matches!(
            slot.initialize(activation_record(FoundationHook::AfterDummyRename), second),
            Err(ControlError::AlreadyInitialized)
        ));
        fs::remove_file(first_path).unwrap();
        fs::remove_file(second_path).unwrap();
    }

    #[test]
    fn nonselected_hook_returns_immediately() {
        let (path, file) = temporary_file("nonselected");
        let activation =
            Activation::new(activation_record(FoundationHook::BeforeDummyRename), file);
        let start = Instant::now();
        activation.reach(PhysicalHook::FoundationAfterDummyRename);
        assert!(start.elapsed() < Duration::from_millis(100));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn concurrent_selected_callers_emit_one_marker_and_never_cross() {
        let (path, file) = temporary_file("concurrent");
        let activation = Arc::new(Activation::new(
            activation_record(FoundationHook::BeforeDummyRename),
            file,
        ));
        let crossed = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let activation = Arc::clone(&activation);
            let crossed = Arc::clone(&crossed);
            std::thread::spawn(move || {
                activation.reach(PhysicalHook::FoundationBeforeDummyRename);
                crossed.fetch_add(1, Ordering::Relaxed);
            });
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !activation.marker_claimed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "marker was not claimed");
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(crossed.load(Ordering::Relaxed), 0);

        let mut marker = activation.marker.lock().unwrap();
        marker.seek(SeekFrom::Start(0)).unwrap();
        let mut contents = String::new();
        marker.read_to_string(&mut contents).unwrap();
        assert_eq!(contents.lines().count(), 1);
        assert!(contents.ends_with('\n'));
        drop(marker);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn marker_contains_only_closed_operational_fields() {
        let record = activation_record(FoundationHook::AfterDummyRename);
        let mut output = Vec::new();
        write_marker(&mut output, &record).unwrap();
        assert!(output.len() <= MAX_MARKER_BYTES);
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["protocol"], MARKER_PROTOCOL);
        assert_eq!(value["architecture_name"], serde_json::Value::Null);
        assert_eq!(
            value["expected_durable_classification"],
            record
                .selection
                .boundary
                .expected_durable_classification
                .as_str()
        );
        assert_eq!(value["sequence"], 1);
        assert_eq!(value["evidence_role"], "operational_only");
        assert_eq!(value["recovery_truth"], false);
        assert_eq!(value["startup_ready"], false);
        let serialized = String::from_utf8(output).unwrap();
        for sentinel in [
            "sentinel-secret",
            "/private/sentinel/path",
            "raw-user-content",
            "raw-command-content",
            "Authorization: Bearer",
        ] {
            assert!(!serialized.contains(sentinel));
        }
        for forbidden in [
            "message",
            "user",
            "model",
            "command",
            "stdout",
            "stderr",
            "path",
            "authorization",
            "token",
            "secret",
        ] {
            assert!(value.get(forbidden).is_none());
        }
    }

    #[test]
    fn durable_classification_wire_values_are_exact_and_closed() {
        for classification in DurableClassification::ALL {
            assert_eq!(
                serde_json::to_value(classification).unwrap(),
                classification.as_str()
            );
            assert_eq!(
                DurableClassification::parse(classification.as_str()),
                Some(classification)
            );
        }
        assert_eq!(DurableClassification::parse("unknown_state"), None);
    }
}
