//! Stage 16 causal eligibility rendering, conservative budgeting, and exact manifest construction.

use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::application::model_selection::{ModelSelectionResult, project_model_tool_definitions};
use crate::application::tool_registry::ToolRegistry;
use crate::domain::{
    CanonicalByteCount, CanonicalModelToolCall, ContextManifestId, LogicalInvocationId,
    ModelInputItem, ModelInputRole, ModelInvocationState, ModelRequest, ModelRequestInput,
    ModelTextPart, ModelToolCallId, ModelToolChoicePolicy, ModelToolDefinition, NormalizedError,
    ProviderNativeOptions, ProviderOpaqueEvidence, Sha256Digest, ToolExecutionState, UtcTimestamp,
    WorkId, WorkState, model_toolset_fingerprint,
};
use crate::ports::artifact_store::{
    ArtifactObjectReference, ArtifactStore, ArtifactStoreErrorKind,
};
use crate::ports::clock::Clock;
use crate::ports::context_source_store::{
    ContextArtifactDescriptor, ContextContinuationBoundary, ContextEligibilityRequest,
    ContextEligibilitySnapshot, ContextModelOutputSource, ContextReconstructionRequest,
    ContextReconstructionSnapshot, ContextReloadedMessageSource, ContextReloadedSource,
    ContextSourceStore, ContextToolResultSource, ContextWorkSource,
};
use crate::ports::model_provider::{TokenEstimateUnit, TokenEstimator};
use crate::ports::state_store::{
    ContextModelRole, ContextSourceIdentity, ContextSourceKind, ContextSourceRecordKind,
    ContextTransformKind, NormalizedModelOutputItem, PreparedContextManifest,
    PreparedContextSource,
};

/// Defensive complete canonical provider-neutral request limit. Equality is allowed.
pub const MAX_CANONICAL_MODEL_REQUEST_BYTES: u64 = 16_777_216;
/// Fixed V0 instruction template version.
pub const V0_INSTRUCTION_VERSION: &str = "craxii-v0-instructions-v1";
/// Fixed V0 assembler implementation version.
pub const V0_CONTEXT_ASSEMBLER_VERSION: &str = "causal-context-assembler-v1";
/// Fixed V0 mandatory full-history policy version.
pub const V0_CONTEXT_POLICY_VERSION: &str = "mandatory-causal-history-v1";

const SYSTEM_INSTRUCTION: &str =
    "You are Craxii. Use only the supplied durable context and report uncertainty honestly.";
const DEVELOPER_INSTRUCTION: &str = "Preserve causal order, use tools only through their definitions, and never assume an unknown tool outcome is safe to repeat.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextAssemblyVersions {
    assembler_version: String,
    context_policy_version: String,
    prompt_version: String,
}

impl ContextAssemblyVersions {
    pub fn try_new(
        assembler_version: impl Into<String>,
        context_policy_version: impl Into<String>,
        prompt_version: impl Into<String>,
    ) -> Result<Self, ContextAssemblyError> {
        let value = Self {
            assembler_version: assembler_version.into(),
            context_policy_version: context_policy_version.into(),
            prompt_version: prompt_version.into(),
        };
        if [
            value.assembler_version.as_str(),
            value.context_policy_version.as_str(),
            value.prompt_version.as_str(),
        ]
        .into_iter()
        .any(|part| {
            part.is_empty()
                || part.len() > 64
                || part.trim() != part
                || part.chars().any(char::is_control)
        }) {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidVersions,
            ));
        }
        Ok(value)
    }

    #[must_use]
    pub fn v0() -> Self {
        Self::try_new(
            V0_CONTEXT_ASSEMBLER_VERSION,
            V0_CONTEXT_POLICY_VERSION,
            V0_INSTRUCTION_VERSION,
        )
        .expect("fixed Stage 16 versions are valid")
    }

    pub fn assembler_version(&self) -> &str {
        &self.assembler_version
    }

    pub fn context_policy_version(&self) -> &str {
        &self.context_policy_version
    }

    pub fn prompt_version(&self) -> &str {
        &self.prompt_version
    }
}

/// Immutable fixed instruction template with separate ordered system/developer blocks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionedInstructionSnapshot {
    version: String,
    system: Box<[ModelTextPart]>,
    developer: Box<[ModelTextPart]>,
    canonical_bytes: Box<[u8]>,
    fingerprint: Sha256Digest,
}

impl VersionedInstructionSnapshot {
    pub fn try_new(
        version: impl Into<String>,
        system: Vec<ModelTextPart>,
        developer: Vec<ModelTextPart>,
    ) -> Result<Self, ContextAssemblyError> {
        let version = version.into();
        if version.is_empty()
            || version.len() > 64
            || version.trim() != version
            || version.chars().any(char::is_control)
            || system.is_empty()
            || developer.is_empty()
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidInstructions,
            ));
        }
        let semantic = json!({
            "developer": developer.iter().map(ModelTextPart::as_str).collect::<Vec<_>>(),
            "system": system.iter().map(ModelTextPart::as_str).collect::<Vec<_>>(),
            "version": version,
        });
        let canonical_bytes = canonical_json_bytes(&semantic);
        let fingerprint = Sha256Digest::hash_bytes(&canonical_bytes);
        Ok(Self {
            version,
            system: system.into_boxed_slice(),
            developer: developer.into_boxed_slice(),
            canonical_bytes: canonical_bytes.into_boxed_slice(),
            fingerprint,
        })
    }

    #[must_use]
    pub fn v0() -> Self {
        Self::try_new(
            V0_INSTRUCTION_VERSION,
            vec![ModelTextPart::try_new(SYSTEM_INSTRUCTION).expect("fixed instruction")],
            vec![ModelTextPart::try_new(DEVELOPER_INSTRUCTION).expect("fixed instruction")],
        )
        .expect("fixed Stage 16 instruction snapshot is valid")
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn system(&self) -> &[ModelTextPart] {
        &self.system
    }

    pub fn developer(&self) -> &[ModelTextPart] {
        &self.developer
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub const fn fingerprint(&self) -> Sha256Digest {
        self.fingerprint
    }

    fn request_instructions(&self) -> Vec<ModelTextPart> {
        self.system
            .iter()
            .chain(self.developer.iter())
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextByteContribution {
    pub source_position: i64,
    pub rendered_bytes: CanonicalByteCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextBudgetEvidence {
    pub estimated_input_tokens: u64,
    pub requested_output_tokens: u64,
    pub context_window_tokens: u64,
    pub request_serialized_bytes: u64,
    pub request_byte_limit: u64,
    pub contributions: Box<[ContextByteContribution]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextLimitEvidence {
    pub estimated_input_tokens: u64,
    pub requested_output_tokens: u64,
    pub reserved_output_tokens: u64,
    pub context_window_tokens: u64,
    pub request_serialized_bytes: u64,
    pub request_byte_limit: u64,
    pub model_target_id: String,
    pub provider_id: String,
    pub provider_model_id: String,
    pub target_configuration_version: i64,
    pub estimator_id: String,
    pub estimator_version: u64,
    pub source_count: u64,
    pub toolset_fingerprint: Sha256Digest,
    pub prompt_fingerprint: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextAssemblyErrorKind {
    Source,
    InvalidVersions,
    InvalidInstructions,
    InvalidSelection,
    DuplicateSource,
    ToolsetMismatch,
    ToolPairing,
    UnknownModelOutput,
    MissingArtifact,
    CorruptArtifact,
    EstimatorMismatch,
    EstimatorFailure,
    ArithmeticOverflow,
    ContextLimitExceeded,
    ContractViolation,
    ClockFailure,
    ReconstructionDrift,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ContextAssemblyError {
    kind: ContextAssemblyErrorKind,
    limit_evidence: Option<Box<ContextLimitEvidence>>,
}

impl ContextAssemblyError {
    #[must_use]
    pub const fn new(kind: ContextAssemblyErrorKind) -> Self {
        Self {
            kind,
            limit_evidence: None,
        }
    }

    fn context_limit(evidence: ContextLimitEvidence) -> Self {
        Self {
            kind: ContextAssemblyErrorKind::ContextLimitExceeded,
            limit_evidence: Some(Box::new(evidence)),
        }
    }

    pub const fn kind(&self) -> ContextAssemblyErrorKind {
        self.kind
    }

    pub fn limit_evidence(&self) -> Option<&ContextLimitEvidence> {
        self.limit_evidence.as_deref()
    }

    #[must_use]
    pub const fn normalized(&self) -> NormalizedError {
        if matches!(self.kind, ContextAssemblyErrorKind::ContextLimitExceeded) {
            NormalizedError::context_limit_exceeded()
        } else {
            NormalizedError::context()
        }
    }
}

impl Display for ContextAssemblyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ContextAssemblyErrorKind::ContextLimitExceeded => "context_limit_exceeded",
            _ => "context assembly failed",
        })
    }
}

impl fmt::Debug for ContextAssemblyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextAssemblyError")
            .field("kind", &self.kind)
            .field("has_limit_evidence", &self.limit_evidence.is_some())
            .finish()
    }
}

impl std::error::Error for ContextAssemblyError {}

/// Immutable provider-neutral context package. It has no mutation surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPackage {
    context_manifest_id: ContextManifestId,
    logical_invocation_id: LogicalInvocationId,
    selected_target: ModelSelectionResult,
    ordered_sources: Box<[PreparedContextSource]>,
    ordered_input_items: Box<[ModelInputItem]>,
    instructions: Box<[ModelTextPart]>,
    tool_definitions: Box<[ModelToolDefinition]>,
    tool_choice: ModelToolChoicePolicy,
    requested_output_tokens: u64,
    estimator_id: String,
    estimator_version: u64,
    prompt_fingerprint: Sha256Digest,
    toolset_fingerprint: Sha256Digest,
}

impl ContextPackage {
    pub const fn context_manifest_id(&self) -> ContextManifestId {
        self.context_manifest_id
    }

    pub const fn logical_invocation_id(&self) -> LogicalInvocationId {
        self.logical_invocation_id
    }

    pub const fn selected_target(&self) -> &ModelSelectionResult {
        &self.selected_target
    }

    pub fn ordered_sources(&self) -> &[PreparedContextSource] {
        &self.ordered_sources
    }

    pub fn ordered_input_items(&self) -> &[ModelInputItem] {
        &self.ordered_input_items
    }

    pub fn instructions(&self) -> &[ModelTextPart] {
        &self.instructions
    }

    pub fn tool_definitions(&self) -> &[ModelToolDefinition] {
        &self.tool_definitions
    }

    pub const fn tool_choice(&self) -> ModelToolChoicePolicy {
        self.tool_choice
    }

    pub const fn requested_output_tokens(&self) -> u64 {
        self.requested_output_tokens
    }

    pub fn estimator_id(&self) -> &str {
        &self.estimator_id
    }

    pub const fn estimator_version(&self) -> u64 {
        self.estimator_version
    }

    pub const fn prompt_fingerprint(&self) -> Sha256Digest {
        self.prompt_fingerprint
    }

    pub const fn toolset_fingerprint(&self) -> Sha256Digest {
        self.toolset_fingerprint
    }

    pub const fn provider_native_options(&self) -> ProviderNativeOptions {
        self.selected_target
            .selected_target()
            .provider_native_options()
    }
}

/// Immutable successful Stage 16 output; successful persistence remains Stage 17-owned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextAssemblyResult {
    package: ContextPackage,
    request: ModelRequest,
    prepared_manifest: PreparedContextManifest,
    prepared_sources: Box<[PreparedContextSource]>,
    budget: ContextBudgetEvidence,
}

impl ContextAssemblyResult {
    pub const fn package(&self) -> &ContextPackage {
        &self.package
    }

    pub const fn request(&self) -> &ModelRequest {
        &self.request
    }

    pub const fn prepared_manifest(&self) -> &PreparedContextManifest {
        &self.prepared_manifest
    }

    pub fn prepared_sources(&self) -> &[PreparedContextSource] {
        &self.prepared_sources
    }

    pub const fn budget(&self) -> &ContextBudgetEvidence {
        &self.budget
    }
}

pub struct ContextAssembler {
    source_store: Arc<dyn ContextSourceStore>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    estimator: Arc<dyn TokenEstimator>,
    tool_registry: Arc<ToolRegistry>,
    instructions: VersionedInstructionSnapshot,
    clock: Arc<dyn Clock>,
}

impl ContextAssembler {
    #[must_use]
    pub fn new(
        source_store: Arc<dyn ContextSourceStore>,
        artifact_store: Option<Arc<dyn ArtifactStore>>,
        estimator: Arc<dyn TokenEstimator>,
        tool_registry: Arc<ToolRegistry>,
        instructions: VersionedInstructionSnapshot,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            source_store,
            artifact_store,
            estimator,
            tool_registry,
            instructions,
            clock,
        }
    }

    pub async fn assemble(
        &self,
        work_id: WorkId,
        selection: &ModelSelectionResult,
        versions: &ContextAssemblyVersions,
    ) -> Result<ContextAssemblyResult, ContextAssemblyError> {
        let snapshot = self
            .source_store
            .load_context_eligibility_snapshot(ContextEligibilityRequest { work_id })
            .await
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::Source))?;
        self.assemble_snapshot(
            snapshot,
            selection,
            versions,
            ContextManifestId::generate(),
            LogicalInvocationId::generate(),
        )
    }

    /// Rebuilds from exact durable sources with the committed immutable IDs and compares bytes.
    pub async fn verify_reconstruction(
        &self,
        prepared: &ContextAssemblyResult,
    ) -> Result<(), ContextAssemblyError> {
        self.validate_reconstruction_bindings(prepared)?;
        let snapshot = self
            .source_store
            .reload_context_sources(ContextReconstructionRequest {
                manifest: prepared.prepared_manifest.clone(),
                ordered_sources: prepared.prepared_sources.clone(),
            })
            .await
            .map_err(|_| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::ReconstructionDrift)
            })?;
        self.verify_exact_reconstruction(prepared, snapshot)
    }

    fn validate_reconstruction_bindings(
        &self,
        prepared: &ContextAssemblyResult,
    ) -> Result<(), ContextAssemblyError> {
        let manifest = &prepared.prepared_manifest;
        let package = &prepared.package;
        let selected = package.selected_target().selected_target();
        let estimator_storage_id =
            format!("{}@{}", package.estimator_id(), package.estimator_version());
        let current_tools = project_model_tool_definitions(&self.tool_registry)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation))?;
        if manifest.sources.as_slice() != prepared.prepared_sources.as_ref()
            || package.ordered_sources() != prepared.prepared_sources.as_ref()
            || prepared
                .prepared_sources
                .iter()
                .enumerate()
                .any(|(index, source)| source.position != index as i64 + 1)
            || package.context_manifest_id() != manifest.context_manifest_id
            || package.logical_invocation_id() != manifest.logical_invocation_id
            || prepared.request.context_manifest_id() != manifest.context_manifest_id
            || prepared.request.logical_invocation_id() != manifest.logical_invocation_id
            || prepared.request.target() != selected
            || selected.reference() != &manifest.provider_model
            || package.selected_target().target_configuration_version()
                != manifest.provider_model.target_configuration_version()
            || package.requested_output_tokens() != manifest.reserved_output_tokens
            || selected.requested_output_tokens().get() as u64 != manifest.reserved_output_tokens
            || estimator_storage_id != manifest.token_estimator_id
            || package.prompt_fingerprint() != manifest.system_prompt_fingerprint
            || package.toolset_fingerprint() != manifest.toolset_fingerprint
            || self.instructions.fingerprint() != manifest.system_prompt_fingerprint
            || self.instructions.version() != V0_INSTRUCTION_VERSION
            || manifest.assembler_version != V0_CONTEXT_ASSEMBLER_VERSION
            || manifest.context_policy_version != V0_CONTEXT_POLICY_VERSION
            || model_toolset_fingerprint(&current_tools) != manifest.toolset_fingerprint
            || package.instructions() != self.instructions.request_instructions()
            || package.tool_definitions() != current_tools
            || self.estimator.identity() != selected.estimator()
            || prepared.request.requested_output_limit() != selected.requested_output_tokens()
            || prepared.request.canonical_sha256() != manifest.rendered_request_sha256
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
        Ok(())
    }

    fn verify_exact_reconstruction(
        &self,
        prepared: &ContextAssemblyResult,
        snapshot: ContextReconstructionSnapshot,
    ) -> Result<(), ContextAssemblyError> {
        let manifest = &prepared.prepared_manifest;
        if snapshot.active_work.work_id != manifest.work_id
            || snapshot.active_work.conversation_id != manifest.eligibility_conversation_id
            || snapshot.active_work.ordinal.get() != manifest.active_work_ordinal
            || snapshot.ordered_sources.len() != prepared.prepared_sources.len()
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
        let selected = prepared.package.selected_target();
        let tool_definitions = project_model_tool_definitions(&self.tool_registry)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation))?;
        let mut builder = AssemblyBuilder::default();
        for (expected, reloaded) in prepared
            .prepared_sources
            .iter()
            .zip(snapshot.ordered_sources.iter())
        {
            let before = builder.sources.len();
            self.render_exact_source(
                &mut builder,
                expected,
                reloaded,
                selected,
                &tool_definitions,
            )?;
            if builder.sources.len() != before + 1 || builder.sources.last() != Some(expected) {
                return Err(ContextAssemblyError::new(
                    ContextAssemblyErrorKind::ReconstructionDrift,
                ));
            }
        }
        if builder.sources.as_slice() != prepared.prepared_sources.as_ref()
            || builder.canonical_source_bytes != manifest.canonical_byte_count.get()
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
        let target = selected.selected_target();
        let canonical_input_items = builder.freeze_canonical_input_items()?;
        let final_request = construct_final_model_request(FinalModelRequestInput {
            logical_invocation_id: manifest.logical_invocation_id,
            target,
            canonical_input_items: canonical_input_items.as_ref(),
            canonical_instructions: &self.instructions.request_instructions(),
            canonical_tool_definitions: &tool_definitions,
            expected_toolset_fingerprint: manifest.toolset_fingerprint,
            context_manifest_id: manifest.context_manifest_id,
        })?;
        let canonical_request_bytes = final_request.canonical_bytes();
        if canonical_request_bytes != prepared.request.canonical_bytes()
            || final_request.canonical_sha256() != manifest.rendered_request_sha256
            || canonical_request_bytes.len() as u64 != manifest.rendered_request_byte_count.get()
            || semantic_manifest_hash_from_prepared(
                manifest,
                selected,
                self.instructions.version(),
                prepared.prepared_sources.as_ref(),
            ) != manifest.manifest_sha256
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
        Ok(())
    }

    fn render_exact_source(
        &self,
        builder: &mut AssemblyBuilder,
        expected: &PreparedContextSource,
        reloaded: &ContextReloadedSource,
        selection: &ModelSelectionResult,
        tool_definitions: &[ModelToolDefinition],
    ) -> Result<(), ContextAssemblyError> {
        match reloaded {
            ContextReloadedSource::InstructionVersion => {
                self.render_exact_instruction_source(builder, expected)
            }
            ContextReloadedSource::ToolDefinition => {
                self.render_exact_tool_definition(builder, expected, tool_definitions)
            }
            ContextReloadedSource::Workstation(source) => {
                render_workstation_source(builder, source)
            }
            ContextReloadedSource::Workspace(source) => render_workspace_source(builder, source),
            ContextReloadedSource::Message(source) => {
                render_exact_message_source(builder, expected.kind, source)
            }
            ContextReloadedSource::ModelOutput(source) => {
                self.render_exact_model_output_source(builder, expected, source, selection)
            }
            ContextReloadedSource::ToolResult(source) => {
                self.verify_tool_artifacts(source)?;
                let call_id = ModelToolCallId::try_new(source.provider_tool_call_id.clone())
                    .map_err(contract_error)?;
                render_tool_result(builder, source, call_id)
            }
            ContextReloadedSource::Work(source) => render_prior_terminal_status(builder, source),
            ContextReloadedSource::Artifact(_) => Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            )),
        }
    }

    fn render_exact_instruction_source(
        &self,
        builder: &mut AssemblyBuilder,
        expected: &PreparedContextSource,
    ) -> Result<(), ContextAssemblyError> {
        let ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::InstructionVersion,
            id,
        } = &expected.identity
        else {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        };
        let (role, instructions, kind, model_role) = match expected.kind {
            ContextSourceKind::SystemInstruction => (
                "system",
                self.instructions.system(),
                ContextSourceKind::SystemInstruction,
                ContextModelRole::System,
            ),
            ContextSourceKind::DeveloperInstruction => (
                "developer",
                self.instructions.developer(),
                ContextSourceKind::DeveloperInstruction,
                ContextModelRole::Developer,
            ),
            _ => {
                return Err(ContextAssemblyError::new(
                    ContextAssemblyErrorKind::ReconstructionDrift,
                ));
            }
        };
        let prefix = format!("{}:{role}:", self.instructions.version());
        let index = id
            .strip_prefix(&prefix)
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::ReconstructionDrift)
            })?;
        let instruction = instructions.get(index - 1).ok_or_else(|| {
            ContextAssemblyError::new(ContextAssemblyErrorKind::ReconstructionDrift)
        })?;
        add_instruction_source(
            builder,
            self.instructions.version(),
            role,
            index - 1,
            instruction,
            kind,
            model_role,
        )
    }

    fn render_exact_tool_definition(
        &self,
        builder: &mut AssemblyBuilder,
        expected: &PreparedContextSource,
        definitions: &[ModelToolDefinition],
    ) -> Result<(), ContextAssemblyError> {
        let ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::ToolDefinition,
            id,
        } = &expected.identity
        else {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        };
        let matches = self
            .tool_registry
            .definitions()
            .iter()
            .zip(definitions.iter())
            .filter(|(durable, _)| {
                format!(
                    "{}:{}:{}",
                    durable.name().as_str(),
                    durable.implementation_version().as_str(),
                    durable.schema_version().get()
                ) == *id
            })
            .collect::<Vec<_>>();
        let [(durable, rendered)] = matches.as_slice() else {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        };
        let source_bytes = durable.canonical_semantic_bytes();
        builder.add_source(
            ContextSourceKind::ToolDefinition,
            expected.identity.clone(),
            None,
            Some("tool_definition".to_owned()),
            Sha256Digest::hash_bytes(&source_bytes),
            ContextTransformKind::InlineProjection,
            source_bytes.len() as u64,
            rendered.canonical_bytes().len() as u64,
        )
    }

    fn render_exact_model_output_source(
        &self,
        builder: &mut AssemblyBuilder,
        expected: &PreparedContextSource,
        output: &ContextModelOutputSource,
        selection: &ModelSelectionResult,
    ) -> Result<(), ContextAssemblyError> {
        match expected.kind {
            ContextSourceKind::CompletedModelOutput => {
                let ContextSourceIdentity::Record { id, .. } = &expected.identity else {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::ReconstructionDrift,
                    ));
                };
                let prefix = format!("{}:item:", output.model_invocation_id);
                let index = id
                    .strip_prefix(&prefix)
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| {
                        ContextAssemblyError::new(ContextAssemblyErrorKind::ReconstructionDrift)
                    })?;
                let item = output
                    .normalized_output
                    .items
                    .get(index - 1)
                    .ok_or_else(|| {
                        ContextAssemblyError::new(ContextAssemblyErrorKind::ReconstructionDrift)
                    })?;
                render_exact_normalized_item(builder, expected.identity.clone(), output, item)
            }
            ContextSourceKind::ProviderNativeContinuation => {
                let selected = selection.selected_target();
                if output.provider_model != *selected.reference()
                    || !selected.reference().capabilities().reasoning_continuation()
                    || !selected.provider_native_options().reasoning_continuation()
                {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::ReconstructionDrift,
                    ));
                }
                let opaque = output
                    .normalized_output
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        NormalizedModelOutputItem::ProviderOpaque {
                            provider_id,
                            item_type,
                            sha256,
                            artifact_id,
                        } => Some((provider_id, item_type, sha256, artifact_id)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let [(provider_id, item_type, sha256, artifact_id)] = opaque.as_slice() else {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::ReconstructionDrift,
                    ));
                };
                if *provider_id != selected.reference().provider_id() {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::ReconstructionDrift,
                    ));
                }
                let descriptor = exactly_one_artifact(output, **artifact_id, **sha256)?;
                let opaque = self.read_opaque_artifact(descriptor)?;
                let evidence = ProviderOpaqueEvidence::try_new(
                    (*provider_id).clone(),
                    (*item_type).clone(),
                    opaque,
                )
                .map_err(contract_error)?;
                if evidence.sha256() != **sha256 {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::CorruptArtifact,
                    ));
                }
                builder.add_item_source(SourceSpec {
                    kind: ContextSourceKind::ProviderNativeContinuation,
                    identity: expected.identity.clone(),
                    model_role: Some(ContextModelRole::Assistant),
                    item_class: "provider_opaque_continuation",
                    source_hash: **sha256,
                    transform: ContextTransformKind::ProviderContinuation,
                    source_bytes: descriptor.captured_byte_count.get(),
                    rendered: ModelInputItem::ProviderOpaqueContinuation(evidence),
                })
            }
            _ => Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            )),
        }
    }

    fn assemble_snapshot(
        &self,
        snapshot: ContextEligibilitySnapshot,
        selection: &ModelSelectionResult,
        versions: &ContextAssemblyVersions,
        context_manifest_id: ContextManifestId,
        logical_invocation_id: LogicalInvocationId,
    ) -> Result<ContextAssemblyResult, ContextAssemblyError> {
        validate_snapshot(&snapshot)?;
        if versions.prompt_version() != self.instructions.version() {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidVersions,
            ));
        }
        let selected = selection.selected_target();
        if selection.target_configuration_version()
            != selected.reference().target_configuration_version()
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
        let tool_definitions = project_model_tool_definitions(&self.tool_registry)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation))?;
        let toolset_fingerprint = model_toolset_fingerprint(&tool_definitions);
        if toolset_fingerprint != self.tool_registry.model_projection_fingerprint()
            || tool_definitions.len() != 2
            || tool_definitions[0].name().as_str() != "read_file"
            || tool_definitions[1].name().as_str() != "run_shell"
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ToolsetMismatch,
            ));
        }

        let mut builder = AssemblyBuilder::default();
        self.render_instruction_sources(&mut builder)?;
        render_capability_sources(&mut builder, &snapshot)?;
        render_tool_sources(&mut builder, &self.tool_registry, &tool_definitions)?;

        for prior_work in &snapshot.prior_works {
            let message = exactly_one_prior_message(&snapshot, prior_work)?;
            render_message(
                &mut builder,
                message,
                ContextSourceKind::UserMessage,
                "user_message",
                ModelInputRole::User,
            )?;
            self.render_model_and_tool_trace(
                &mut builder,
                &snapshot,
                prior_work.work_id,
                selection,
            )?;
            let assistants = snapshot
                .prior_final_assistant_messages
                .iter()
                .filter(|source| source.work_id == prior_work.work_id)
                .collect::<Vec<_>>();
            if assistants.len() > 1 {
                return Err(ContextAssemblyError::new(
                    ContextAssemblyErrorKind::DuplicateSource,
                ));
            }
            if let Some(assistant) = assistants.first() {
                render_assistant_message(&mut builder, assistant)?;
            } else {
                render_prior_terminal_status(&mut builder, prior_work)?;
            }
        }

        render_message(
            &mut builder,
            &snapshot.active_trigger,
            ContextSourceKind::ActiveTrigger,
            "active_trigger",
            ModelInputRole::User,
        )?;
        self.render_model_and_tool_trace(
            &mut builder,
            &snapshot,
            snapshot.active_work.work_id,
            selection,
        )?;

        if builder.paired_tool_ids.len() != snapshot.observed_tool_results.len() {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ToolPairing,
            ));
        }

        let canonical_input_items = builder.freeze_canonical_input_items()?;
        let instructions = self.instructions.request_instructions();
        let requested_output = selected.requested_output_tokens();
        let final_request = construct_final_model_request(FinalModelRequestInput {
            logical_invocation_id,
            target: selected,
            canonical_input_items: canonical_input_items.as_ref(),
            canonical_instructions: &instructions,
            canonical_tool_definitions: &tool_definitions,
            expected_toolset_fingerprint: self.tool_registry.model_projection_fingerprint(),
            context_manifest_id,
        })?;
        let canonical_request_bytes = final_request.canonical_bytes();
        let request_byte_count = u64::try_from(canonical_request_bytes.len())
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        let context_window_tokens = selected
            .reference()
            .capabilities()
            .context_window_tokens()
            .get() as u64;
        let requested_output_tokens = requested_output.get() as u64;
        let limit_evidence = |estimated_input_tokens| ContextLimitEvidence {
            estimated_input_tokens,
            requested_output_tokens,
            reserved_output_tokens: requested_output_tokens,
            context_window_tokens,
            request_serialized_bytes: request_byte_count,
            request_byte_limit: MAX_CANONICAL_MODEL_REQUEST_BYTES,
            model_target_id: selected.reference().model_target_id().as_str().to_owned(),
            provider_id: selected.reference().provider_id().as_str().to_owned(),
            provider_model_id: selected.reference().provider_model_id().as_str().to_owned(),
            target_configuration_version: selected.reference().target_configuration_version().get(),
            estimator_id: selected.estimator().id().to_owned(),
            estimator_version: selected.estimator().version(),
            source_count: u64::try_from(builder.sources.len()).unwrap_or(u64::MAX),
            toolset_fingerprint,
            prompt_fingerprint: self.instructions.fingerprint(),
        };
        if validate_request_byte_limit(request_byte_count, MAX_CANONICAL_MODEL_REQUEST_BYTES)
            .is_err()
        {
            return Err(ContextAssemblyError::context_limit(limit_evidence(0)));
        }
        if self.estimator.identity() != selected.estimator() {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::EstimatorMismatch,
            ));
        }
        let units = complete_request_units(&final_request, request_byte_count)?;
        let estimate = self
            .estimator
            .estimate(selected, &units)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::EstimatorFailure))?;
        if estimate.estimator() != selected.estimator()
            || estimate.estimator() != self.estimator.identity()
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::EstimatorMismatch,
            ));
        }
        let estimated_input_tokens = estimate.tokens();
        if validate_token_fit(
            estimated_input_tokens,
            requested_output_tokens,
            context_window_tokens,
        )
        .is_err()
        {
            return Err(ContextAssemblyError::context_limit(limit_evidence(
                estimated_input_tokens,
            )));
        }

        if estimated_input_tokens > 2_147_483_647 || context_window_tokens > 2_147_483_647 {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ArithmeticOverflow,
            ));
        }
        let total_tokens = estimated_input_tokens
            .checked_add(requested_output_tokens)
            .ok_or_else(|| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
            })?;
        let utilization_basis_points = total_tokens
            .checked_mul(10_000)
            .and_then(|value| value.checked_add(context_window_tokens - 1))
            .map(|value| value / context_window_tokens)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
            })?;
        let canonical_byte_count = CanonicalByteCount::try_new(builder.canonical_source_bytes)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        let rendered_request_byte_count = CanonicalByteCount::try_new(request_byte_count)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        let rendered_request_sha256 = final_request.canonical_sha256();
        let estimator_storage_id = format!(
            "{}@{}",
            selected.estimator().id(),
            selected.estimator().version()
        );
        if estimator_storage_id.len() > 64 {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
        let transformed_source_count = builder
            .sources
            .iter()
            .filter(|source| source.transformed)
            .count() as u64;
        let created_at = UtcTimestamp::from_offset_datetime(
            self.clock
                .utc_now()
                .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ClockFailure))?,
        )
        .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ClockFailure))?;
        let manifest_sha256 = semantic_manifest_hash(ManifestHashInput {
            context_manifest_id,
            logical_invocation_id,
            work_id: snapshot.active_work.work_id,
            eligibility_conversation_id: snapshot.active_work.conversation_id,
            active_work_ordinal: snapshot.active_work.ordinal.get(),
            highest_prior_terminal_work_ordinal: snapshot
                .highest_prior_terminal_work_ordinal
                .map(|value| value.get()),
            input_event_ids: &snapshot.exact_input_event_ids,
            active_output_record_ids: &snapshot.active_output_record_ids,
            maximum_journal_offset: snapshot.maximum_journal_offset,
            selection,
            assembler_version: versions.assembler_version(),
            context_policy_version: versions.context_policy_version(),
            prompt_version: versions.prompt_version(),
            prompt_fingerprint: self.instructions.fingerprint(),
            toolset_fingerprint,
            canonical_byte_count: canonical_byte_count.get(),
            request_byte_count,
            estimated_input_tokens,
            context_window_tokens,
            requested_output_tokens,
            utilization_basis_points,
            transformed_source_count,
            request_sha256: rendered_request_sha256,
            sources: &builder.sources,
        });
        let prepared_manifest = PreparedContextManifest {
            context_manifest_id,
            work_id: snapshot.active_work.work_id,
            logical_invocation_id,
            provider_model: selected.reference().clone(),
            assembler_version: versions.assembler_version().to_owned(),
            context_policy_version: versions.context_policy_version().to_owned(),
            system_prompt_fingerprint: self.instructions.fingerprint(),
            toolset_fingerprint,
            eligibility_conversation_id: snapshot.active_work.conversation_id,
            active_work_ordinal: snapshot.active_work.ordinal.get(),
            highest_prior_terminal_work_ordinal: snapshot
                .highest_prior_terminal_work_ordinal
                .map(|value| value.get()),
            input_event_ids: snapshot.exact_input_event_ids.clone(),
            active_output_record_ids: snapshot.active_output_record_ids.clone(),
            maximum_journal_offset: snapshot.maximum_journal_offset,
            canonical_byte_count,
            rendered_request_byte_count,
            estimated_input_tokens,
            token_estimator_id: estimator_storage_id,
            context_window_tokens,
            reserved_output_tokens: requested_output_tokens,
            utilization_basis_points,
            manifest_sha256,
            rendered_request_sha256,
            rendered_request_artifact_id: None,
            omitted_source_count: 0,
            transformed_source_count,
            sources: builder.sources.clone(),
            created_at,
        };
        let package = ContextPackage {
            context_manifest_id,
            logical_invocation_id,
            selected_target: selection.clone(),
            ordered_sources: builder.sources.clone().into_boxed_slice(),
            ordered_input_items: canonical_input_items.clone(),
            instructions: instructions.clone().into_boxed_slice(),
            tool_definitions: tool_definitions.clone().into_boxed_slice(),
            tool_choice: ModelToolChoicePolicy::Automatic,
            requested_output_tokens,
            estimator_id: selected.estimator().id().to_owned(),
            estimator_version: selected.estimator().version(),
            prompt_fingerprint: self.instructions.fingerprint(),
            toolset_fingerprint,
        };
        let budget = ContextBudgetEvidence {
            estimated_input_tokens,
            requested_output_tokens,
            context_window_tokens,
            request_serialized_bytes: request_byte_count,
            request_byte_limit: MAX_CANONICAL_MODEL_REQUEST_BYTES,
            contributions: builder.contributions.into_boxed_slice(),
        };
        Ok(ContextAssemblyResult {
            package,
            request: final_request,
            prepared_manifest,
            prepared_sources: builder.sources.into_boxed_slice(),
            budget,
        })
    }

    fn render_instruction_sources(
        &self,
        builder: &mut AssemblyBuilder,
    ) -> Result<(), ContextAssemblyError> {
        for (index, instruction) in self.instructions.system().iter().enumerate() {
            add_instruction_source(
                builder,
                self.instructions.version(),
                "system",
                index,
                instruction,
                ContextSourceKind::SystemInstruction,
                ContextModelRole::System,
            )?;
        }
        for (index, instruction) in self.instructions.developer().iter().enumerate() {
            add_instruction_source(
                builder,
                self.instructions.version(),
                "developer",
                index,
                instruction,
                ContextSourceKind::DeveloperInstruction,
                ContextModelRole::Developer,
            )?;
        }
        Ok(())
    }

    fn render_model_and_tool_trace(
        &self,
        builder: &mut AssemblyBuilder,
        snapshot: &ContextEligibilitySnapshot,
        work_id: WorkId,
        selection: &ModelSelectionResult,
    ) -> Result<(), ContextAssemblyError> {
        for output in snapshot
            .completed_model_outputs
            .iter()
            .filter(|source| source.work_id == work_id)
        {
            self.render_model_output(builder, snapshot, output, selection)?;
        }
        Ok(())
    }

    fn render_model_output(
        &self,
        builder: &mut AssemblyBuilder,
        snapshot: &ContextEligibilitySnapshot,
        output: &ContextModelOutputSource,
        selection: &ModelSelectionResult,
    ) -> Result<(), ContextAssemblyError> {
        let output_source_hash = normalized_output_source_hash(output);
        let has_tool_calls = output
            .normalized_output
            .items
            .iter()
            .any(|item| matches!(item, NormalizedModelOutputItem::ToolCall { .. }));
        let mut tool_call_ordinal = 0_i64;
        for (item_index, item) in output.normalized_output.items.iter().enumerate() {
            let identity = ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::ModelInvocation,
                id: format!("{}:item:{}", output.model_invocation_id, item_index + 1),
            };
            match item {
                NormalizedModelOutputItem::Text { text }
                    if output.has_committed_final_assistant && !has_tool_calls => {}
                NormalizedModelOutputItem::Text { text } => {
                    let rendered = ModelInputItem::prior_assistant(vec![model_text(text)?])
                        .map_err(contract_error)?;
                    builder.add_item_source(SourceSpec {
                        kind: ContextSourceKind::CompletedModelOutput,
                        identity,
                        model_role: Some(ContextModelRole::Assistant),
                        item_class: "model_text",
                        source_hash: output_source_hash,
                        transform: ContextTransformKind::Identity,
                        source_bytes: normalized_output_source_bytes(output).len() as u64,
                        rendered,
                    })?;
                }
                NormalizedModelOutputItem::ToolCall {
                    call_id,
                    tool_name,
                    arguments_json,
                } => {
                    tool_call_ordinal = tool_call_ordinal.checked_add(1).ok_or_else(|| {
                        ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
                    })?;
                    let call_id =
                        ModelToolCallId::try_new(call_id.clone()).map_err(contract_error)?;
                    let call = CanonicalModelToolCall::try_new(
                        call_id.clone(),
                        tool_name.as_str(),
                        arguments_json.clone(),
                    )
                    .map_err(contract_error)?;
                    call.require_valid_arguments().map_err(contract_error)?;
                    builder.add_item_source(SourceSpec {
                        kind: ContextSourceKind::CompletedModelOutput,
                        identity,
                        model_role: Some(ContextModelRole::Assistant),
                        item_class: "tool_call",
                        source_hash: output_source_hash,
                        transform: ContextTransformKind::Identity,
                        source_bytes: normalized_output_source_bytes(output).len() as u64,
                        rendered: ModelInputItem::ToolCall(call),
                    })?;
                    let matches = snapshot
                        .observed_tool_results
                        .iter()
                        .filter(|tool| {
                            tool.source_model_invocation_id == output.model_invocation_id
                                && tool.work_id == output.work_id
                                && tool.conversation_id == output.conversation_id
                                && tool.agent_step_no == output.agent_step_no
                                && tool.tool_ordinal.get() == tool_call_ordinal
                                && tool.provider_tool_call_id == call_id.as_str()
                                && tool.tool_name == *tool_name
                        })
                        .collect::<Vec<_>>();
                    if matches.len() > 1 {
                        return Err(ContextAssemblyError::new(
                            ContextAssemblyErrorKind::ToolPairing,
                        ));
                    }
                    if let Some(tool) = matches.first() {
                        self.verify_tool_artifacts(tool)?;
                        render_tool_result(builder, tool, call_id)?;
                    }
                }
                NormalizedModelOutputItem::StructuredData { canonical_json } => {
                    let value = serde_json::from_str(canonical_json).map_err(|_| {
                        ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation)
                    })?;
                    builder.add_item_source(SourceSpec {
                        kind: ContextSourceKind::CompletedModelOutput,
                        identity,
                        model_role: Some(ContextModelRole::Assistant),
                        item_class: "structured_data",
                        source_hash: output_source_hash,
                        transform: ContextTransformKind::Identity,
                        source_bytes: normalized_output_source_bytes(output).len() as u64,
                        rendered: ModelInputItem::structured_data(value).map_err(contract_error)?,
                    })?;
                }
                NormalizedModelOutputItem::Refusal { text } => {
                    builder.add_item_source(SourceSpec {
                        kind: ContextSourceKind::CompletedModelOutput,
                        identity,
                        model_role: Some(ContextModelRole::Assistant),
                        item_class: "historical_refusal",
                        source_hash: output_source_hash,
                        transform: ContextTransformKind::Identity,
                        source_bytes: normalized_output_source_bytes(output).len() as u64,
                        rendered: ModelInputItem::historical_refusal(vec![model_text(text)?])
                            .map_err(contract_error)?,
                    })?;
                }
                NormalizedModelOutputItem::ReasoningSummary { text } => {
                    builder.add_item_source(SourceSpec {
                        kind: ContextSourceKind::CompletedModelOutput,
                        identity,
                        model_role: Some(ContextModelRole::Assistant),
                        item_class: "historical_reasoning_summary",
                        source_hash: output_source_hash,
                        transform: ContextTransformKind::Identity,
                        source_bytes: normalized_output_source_bytes(output).len() as u64,
                        rendered: ModelInputItem::historical_reasoning_summary(vec![model_text(
                            text,
                        )?])
                        .map_err(contract_error)?,
                    })?;
                }
                NormalizedModelOutputItem::ProviderOpaque {
                    provider_id,
                    item_type,
                    sha256,
                    artifact_id,
                } => {
                    if provider_id == selection.selected_target().reference().provider_id()
                        && continuation_is_eligible(snapshot, output, selection)
                    {
                        let descriptor = exactly_one_artifact(output, *artifact_id, *sha256)?;
                        let opaque = self.read_opaque_artifact(descriptor)?;
                        let evidence = ProviderOpaqueEvidence::try_new(
                            provider_id.clone(),
                            item_type.clone(),
                            opaque,
                        )
                        .map_err(contract_error)?;
                        if evidence.sha256() != *sha256 {
                            return Err(ContextAssemblyError::new(
                                ContextAssemblyErrorKind::CorruptArtifact,
                            ));
                        }
                        builder.add_item_source(SourceSpec {
                            kind: ContextSourceKind::ProviderNativeContinuation,
                            identity: ContextSourceIdentity::Record {
                                kind: ContextSourceRecordKind::ModelInvocation,
                                id: format!("{}:continuation", output.model_invocation_id),
                            },
                            model_role: Some(ContextModelRole::Assistant),
                            item_class: "provider_opaque_continuation",
                            source_hash: *sha256,
                            transform: ContextTransformKind::ProviderContinuation,
                            source_bytes: descriptor.captured_byte_count.get(),
                            rendered: ModelInputItem::ProviderOpaqueContinuation(evidence),
                        })?;
                    }
                }
                NormalizedModelOutputItem::UnknownProviderItem { .. } => {
                    return Err(ContextAssemblyError::new(
                        ContextAssemblyErrorKind::UnknownModelOutput,
                    ));
                }
            }
        }
        Ok(())
    }

    fn read_opaque_artifact(
        &self,
        descriptor: &ContextArtifactDescriptor,
    ) -> Result<String, ContextAssemblyError> {
        let artifact_store = self
            .artifact_store
            .as_ref()
            .ok_or_else(|| ContextAssemblyError::new(ContextAssemblyErrorKind::MissingArtifact))?;
        let object = ArtifactObjectReference::from_persisted_metadata(
            descriptor.storage_key.clone(),
            descriptor.sha256,
            descriptor.captured_byte_count,
        );
        let bytes = artifact_store.read_verified(&object).map_err(|error| {
            ContextAssemblyError::new(match error.kind() {
                ArtifactStoreErrorKind::Integrity => ContextAssemblyErrorKind::CorruptArtifact,
                _ => ContextAssemblyErrorKind::MissingArtifact,
            })
        })?;
        String::from_utf8(bytes)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::CorruptArtifact))
    }

    fn verify_tool_artifacts(
        &self,
        tool: &ContextToolResultSource,
    ) -> Result<(), ContextAssemblyError> {
        for descriptor in [tool.stdout_artifact.as_ref(), tool.stderr_artifact.as_ref()]
            .into_iter()
            .flatten()
        {
            let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::MissingArtifact)
            })?;
            let object = ArtifactObjectReference::from_persisted_metadata(
                descriptor.storage_key.clone(),
                descriptor.sha256,
                descriptor.captured_byte_count,
            );
            artifact_store.verify(&object).map_err(|error| {
                ContextAssemblyError::new(match error.kind() {
                    ArtifactStoreErrorKind::Integrity => ContextAssemblyErrorKind::CorruptArtifact,
                    _ => ContextAssemblyErrorKind::MissingArtifact,
                })
            })?;
        }
        Ok(())
    }
}

struct FinalModelRequestInput<'a> {
    logical_invocation_id: LogicalInvocationId,
    target: &'a crate::domain::ModelTarget,
    canonical_input_items: &'a [ModelInputItem],
    canonical_instructions: &'a [ModelTextPart],
    canonical_tool_definitions: &'a [ModelToolDefinition],
    expected_toolset_fingerprint: Sha256Digest,
    context_manifest_id: ContextManifestId,
}

fn construct_final_model_request(
    input: FinalModelRequestInput<'_>,
) -> Result<ModelRequest, ContextAssemblyError> {
    if model_toolset_fingerprint(input.canonical_tool_definitions)
        != input.expected_toolset_fingerprint
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ToolsetMismatch,
        ));
    }
    let final_request = ModelRequest::try_new(ModelRequestInput {
        logical_invocation_id: input.logical_invocation_id,
        target: input.target.clone(),
        ordered_input_items: input.canonical_input_items.to_vec(),
        instructions: input.canonical_instructions.to_vec(),
        tool_definitions: input.canonical_tool_definitions.to_vec(),
        requested_output_limit: input.target.requested_output_tokens(),
        tool_choice_policy: ModelToolChoicePolicy::Automatic,
        provider_native_options: input.target.provider_native_options(),
        context_manifest_id: input.context_manifest_id,
    })
    .map_err(contract_error)?;
    if final_request.ordered_input_items() != input.canonical_input_items
        || final_request.tool_definitions() != input.canonical_tool_definitions
        || final_request.requested_output_limit() != input.target.requested_output_tokens()
        || model_toolset_fingerprint(final_request.tool_definitions())
            != input.expected_toolset_fingerprint
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ContractViolation,
        ));
    }
    Ok(final_request)
}

#[derive(Default)]
struct AssemblyBuilder {
    sources: Vec<PreparedContextSource>,
    items: Vec<ModelInputItem>,
    rendered_input_item_count: usize,
    contributions: Vec<ContextByteContribution>,
    seen_source_identities: BTreeSet<String>,
    paired_tool_ids: BTreeSet<String>,
    canonical_source_bytes: u64,
}

struct SourceSpec<'a> {
    kind: ContextSourceKind,
    identity: ContextSourceIdentity,
    model_role: Option<ContextModelRole>,
    item_class: &'a str,
    source_hash: Sha256Digest,
    transform: ContextTransformKind,
    source_bytes: u64,
    rendered: ModelInputItem,
}

impl AssemblyBuilder {
    fn add_item_source(&mut self, spec: SourceSpec<'_>) -> Result<(), ContextAssemblyError> {
        let rendered_input_item_count =
            self.rendered_input_item_count
                .checked_add(1)
                .ok_or_else(|| {
                    ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
                })?;
        let rendered_bytes = u64::try_from(spec.rendered.canonical_bytes().len())
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        self.add_source(
            spec.kind,
            spec.identity,
            spec.model_role,
            Some(spec.item_class.to_owned()),
            spec.source_hash,
            spec.transform,
            spec.source_bytes,
            rendered_bytes,
        )?;
        self.items.push(spec.rendered);
        self.rendered_input_item_count = rendered_input_item_count;
        Ok(())
    }

    fn freeze_canonical_input_items(&self) -> Result<Box<[ModelInputItem]>, ContextAssemblyError> {
        if self.items.len() != self.rendered_input_item_count {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ContractViolation,
            ));
        }
        Ok(self.items.clone().into_boxed_slice())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_source(
        &mut self,
        kind: ContextSourceKind,
        identity: ContextSourceIdentity,
        model_role: Option<ContextModelRole>,
        item_class: Option<String>,
        source_hash: Sha256Digest,
        transform: ContextTransformKind,
        source_bytes: u64,
        rendered_bytes: u64,
    ) -> Result<(), ContextAssemblyError> {
        if !self
            .seen_source_identities
            .insert(source_identity_key(&identity))
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::DuplicateSource,
            ));
        }
        self.canonical_source_bytes = self
            .canonical_source_bytes
            .checked_add(source_bytes)
            .ok_or_else(|| {
                ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
            })?;
        let position = i64::try_from(self.sources.len() + 1)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        let rendered_byte_contribution = CanonicalByteCount::try_new(rendered_bytes)
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        let transformed = transform != ContextTransformKind::Identity;
        self.sources.push(PreparedContextSource {
            position,
            kind,
            identity,
            model_role,
            item_class,
            source_content_sha256: source_hash,
            rendered_byte_contribution,
            transform,
            transformed,
        });
        self.contributions.push(ContextByteContribution {
            source_position: position,
            rendered_bytes: rendered_byte_contribution,
        });
        Ok(())
    }
}

fn validate_snapshot(snapshot: &ContextEligibilitySnapshot) -> Result<(), ContextAssemblyError> {
    let active = &snapshot.active_work;
    if snapshot.active_trigger.work_id != active.work_id
        || snapshot.active_trigger.work_ordinal != active.ordinal
        || snapshot.active_trigger.message.conversation_id() != active.conversation_id
        || snapshot.active_trigger.message.role() != crate::domain::MessageRole::User
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::InvalidSelection,
        ));
    }
    let mut prior_ordinals = BTreeSet::new();
    for work in &snapshot.prior_works {
        if work.conversation_id != active.conversation_id
            || work.ordinal >= active.ordinal
            || !prior_ordinals.insert(work.ordinal.get())
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
    }
    if snapshot
        .prior_works
        .windows(2)
        .any(|pair| pair[0].ordinal >= pair[1].ordinal)
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::InvalidSelection,
        ));
    }
    for message in &snapshot.prior_messages {
        if message.work_ordinal >= active.ordinal
            || message.message.conversation_id() != active.conversation_id
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
    }
    for assistant in &snapshot.prior_final_assistant_messages {
        if assistant.work_ordinal >= active.ordinal
            || assistant.message.conversation_id() != active.conversation_id
            || assistant.message.produced_by_work_id() != Some(assistant.work_id)
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
    }
    for output in &snapshot.completed_model_outputs {
        if output.conversation_id != active.conversation_id
            || output.work_ordinal > active.ordinal
            || output.work_ordinal == active.ordinal && output.work_id != active.work_id
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
    }
    for tool in &snapshot.observed_tool_results {
        if tool.conversation_id != active.conversation_id
            || tool.work_ordinal > active.ordinal
            || tool.work_ordinal == active.ordinal && tool.work_id != active.work_id
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::InvalidSelection,
            ));
        }
    }
    let mut input_ids = BTreeSet::new();
    if snapshot
        .exact_input_event_ids
        .iter()
        .any(|event_id| !input_ids.insert(event_id.to_string()))
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::DuplicateSource,
        ));
    }
    Ok(())
}

fn exactly_one_prior_message<'a>(
    snapshot: &'a ContextEligibilitySnapshot,
    work: &ContextWorkSource,
) -> Result<&'a crate::ports::context_source_store::ContextMessageSource, ContextAssemblyError> {
    let messages = snapshot
        .prior_messages
        .iter()
        .filter(|message| message.work_id == work.work_id && message.work_ordinal == work.ordinal)
        .collect::<Vec<_>>();
    if messages.len() != 1 {
        return Err(ContextAssemblyError::new(if messages.is_empty() {
            ContextAssemblyErrorKind::Source
        } else {
            ContextAssemblyErrorKind::DuplicateSource
        }));
    }
    Ok(messages[0])
}

fn add_instruction_source(
    builder: &mut AssemblyBuilder,
    version: &str,
    role: &str,
    index: usize,
    instruction: &ModelTextPart,
    kind: ContextSourceKind,
    model_role: ContextModelRole,
) -> Result<(), ContextAssemblyError> {
    let semantic = json!({
        "content": instruction.as_str(),
        "ordinal": index + 1,
        "role": role,
        "version": version,
    });
    let source_bytes = canonical_json_bytes(&semantic);
    let rendered_bytes = serde_json::to_vec(instruction.as_str())
        .expect("validated instruction serializes")
        .len() as u64;
    builder.add_source(
        kind,
        ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::InstructionVersion,
            id: format!("{version}:{role}:{}", index + 1),
        },
        Some(model_role),
        Some(format!("{role}_instruction")),
        Sha256Digest::hash_bytes(&source_bytes),
        ContextTransformKind::Identity,
        source_bytes.len() as u64,
        rendered_bytes,
    )
}

fn render_capability_sources(
    builder: &mut AssemblyBuilder,
    snapshot: &ContextEligibilitySnapshot,
) -> Result<(), ContextAssemblyError> {
    render_workstation_source(builder, &snapshot.workstation)?;
    render_workspace_source(builder, &snapshot.workspace)
}

fn render_workstation_source(
    builder: &mut AssemblyBuilder,
    source: &crate::ports::context_source_store::ContextWorkstationSource,
) -> Result<(), ContextAssemblyError> {
    let workstation = json!({
        "kind": "workstation_capability_summary",
        "value": source.semantic_json,
    });
    builder.add_item_source(SourceSpec {
        kind: ContextSourceKind::WorkstationCapabilitySummary,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::Workstation,
            id: source.workstation_id.to_string(),
        },
        model_role: Some(ContextModelRole::Developer),
        item_class: "workstation_capability_summary",
        source_hash: source.source_sha256,
        transform: ContextTransformKind::InlineProjection,
        source_bytes: canonical_json_bytes(&source.semantic_json).len() as u64,
        rendered: ModelInputItem::structured_data(workstation).map_err(contract_error)?,
    })
}

fn render_workspace_source(
    builder: &mut AssemblyBuilder,
    source: &crate::ports::context_source_store::ContextWorkspaceSource,
) -> Result<(), ContextAssemblyError> {
    let workspace = json!({
        "kind": "logical_workspace_identity",
        "value": source.semantic_json,
    });
    builder.add_item_source(SourceSpec {
        kind: ContextSourceKind::WorkspaceIdentity,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::Workspace,
            id: source.workspace_id.to_string(),
        },
        model_role: Some(ContextModelRole::Developer),
        item_class: "workspace_identity",
        source_hash: source.source_sha256,
        transform: ContextTransformKind::InlineProjection,
        source_bytes: canonical_json_bytes(&source.semantic_json).len() as u64,
        rendered: ModelInputItem::structured_data(workspace).map_err(contract_error)?,
    })
}

fn render_tool_sources(
    builder: &mut AssemblyBuilder,
    registry: &ToolRegistry,
    definitions: &[ModelToolDefinition],
) -> Result<(), ContextAssemblyError> {
    if registry.definitions().len() != definitions.len() {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ToolsetMismatch,
        ));
    }
    for (durable, rendered) in registry.definitions().iter().zip(definitions) {
        if durable.name() != rendered.name() {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ToolsetMismatch,
            ));
        }
        let source_bytes = durable.canonical_semantic_bytes();
        builder.add_source(
            ContextSourceKind::ToolDefinition,
            ContextSourceIdentity::Record {
                kind: ContextSourceRecordKind::ToolDefinition,
                id: format!(
                    "{}:{}:{}",
                    durable.name().as_str(),
                    durable.implementation_version().as_str(),
                    durable.schema_version().get()
                ),
            },
            None,
            Some("tool_definition".to_owned()),
            Sha256Digest::hash_bytes(&source_bytes),
            ContextTransformKind::InlineProjection,
            source_bytes.len() as u64,
            rendered.canonical_bytes().len() as u64,
        )?;
    }
    Ok(())
}

fn render_message(
    builder: &mut AssemblyBuilder,
    source: &crate::ports::context_source_store::ContextMessageSource,
    kind: ContextSourceKind,
    item_class: &str,
    role: ModelInputRole,
) -> Result<(), ContextAssemblyError> {
    let parts = source
        .message
        .content()
        .blocks()
        .iter()
        .map(|block| model_text(block.as_text()))
        .collect::<Result<Vec<_>, _>>()?;
    builder.add_item_source(SourceSpec {
        kind,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::Message,
            id: source.message.message_id().to_string(),
        },
        model_role: Some(ContextModelRole::User),
        item_class,
        source_hash: source.message.content_sha256(),
        transform: ContextTransformKind::Identity,
        source_bytes: source.message.content().canonical_bytes().len() as u64,
        rendered: ModelInputItem::message(role, parts).map_err(contract_error)?,
    })
}

fn render_assistant_message(
    builder: &mut AssemblyBuilder,
    source: &crate::ports::context_source_store::ContextAssistantMessageSource,
) -> Result<(), ContextAssemblyError> {
    let parts = source
        .message
        .content()
        .blocks()
        .iter()
        .map(|block| model_text(block.as_text()))
        .collect::<Result<Vec<_>, _>>()?;
    builder.add_item_source(SourceSpec {
        kind: ContextSourceKind::AssistantMessage,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::Message,
            id: source.message.message_id().to_string(),
        },
        model_role: Some(ContextModelRole::Assistant),
        item_class: "final_assistant_message",
        source_hash: source.message.content_sha256(),
        transform: ContextTransformKind::Identity,
        source_bytes: source.message.content().canonical_bytes().len() as u64,
        rendered: ModelInputItem::prior_assistant(parts).map_err(contract_error)?,
    })
}

fn render_exact_message_source(
    builder: &mut AssemblyBuilder,
    kind: ContextSourceKind,
    source: &ContextReloadedMessageSource,
) -> Result<(), ContextAssemblyError> {
    match kind {
        ContextSourceKind::UserMessage | ContextSourceKind::ActiveTrigger => {
            let message = crate::ports::context_source_store::ContextMessageSource {
                work_id: source.work_id,
                work_ordinal: source.work_ordinal,
                input_event_id: source.journal_event_id,
                journal_offset: source.journal_offset,
                message: source.message.clone(),
            };
            render_message(
                builder,
                &message,
                kind,
                if kind == ContextSourceKind::ActiveTrigger {
                    "active_trigger"
                } else {
                    "user_message"
                },
                ModelInputRole::User,
            )
        }
        ContextSourceKind::AssistantMessage => {
            let message = crate::ports::context_source_store::ContextAssistantMessageSource {
                work_id: source.work_id,
                work_ordinal: source.work_ordinal,
                journal_event_id: source.journal_event_id,
                journal_offset: source.journal_offset,
                message: source.message.clone(),
            };
            render_assistant_message(builder, &message)
        }
        _ => Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ReconstructionDrift,
        )),
    }
}

fn render_prior_terminal_status(
    builder: &mut AssemblyBuilder,
    work: &ContextWorkSource,
) -> Result<(), ContextAssemblyError> {
    let (kind, status, item_class) = match work.state {
        WorkState::Failed => (
            ContextSourceKind::SyntheticFailure,
            "prior_work_failed",
            "synthetic_failure",
        ),
        WorkState::Interrupted => (
            ContextSourceKind::SyntheticInterruption,
            "prior_work_interrupted",
            "synthetic_interruption",
        ),
        WorkState::Cancelled => return Ok(()),
        _ => return Ok(()),
    };
    let terminal_offset = work
        .terminal_journal_offset
        .ok_or_else(|| ContextAssemblyError::new(ContextAssemblyErrorKind::Source))?;
    let durable = json!({
        "journal_offset": terminal_offset.get(),
        "state": work_state_literal(work.state),
        "terminal_reason": work.terminal_reason,
        "work_id": work.work_id.to_string(),
    });
    let rendered = json!({
        "state": work_state_literal(work.state),
        "work_id": work.work_id.to_string(),
    });
    let durable_bytes = canonical_json_bytes(&durable);
    builder.add_item_source(SourceSpec {
        kind,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::Work,
            id: work.work_id.to_string(),
        },
        model_role: Some(ContextModelRole::Assistant),
        item_class,
        source_hash: Sha256Digest::hash_bytes(&durable_bytes),
        transform: ContextTransformKind::SyntheticStatus,
        source_bytes: durable_bytes.len() as u64,
        rendered: ModelInputItem::synthetic_runtime_status(status, rendered)
            .map_err(contract_error)?,
    })
}

fn render_tool_result(
    builder: &mut AssemblyBuilder,
    tool: &ContextToolResultSource,
    call_id: ModelToolCallId,
) -> Result<(), ContextAssemblyError> {
    if !builder
        .paired_tool_ids
        .insert(tool.tool_execution_id.to_string())
    {
        return Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ToolPairing,
        ));
    }
    let durable = tool_semantic_value(tool);
    let durable_bytes = canonical_json_bytes(&durable);
    let (kind, item_class, rendered) = match tool.state {
        ToolExecutionState::Completed => {
            let result = tool
                .result
                .as_ref()
                .ok_or_else(|| ContextAssemblyError::new(ContextAssemblyErrorKind::Source))?;
            let projection = json!({
                "artifacts": artifact_projection(tool),
                "provider_tool_call_id": tool.provider_tool_call_id,
                "result": result,
                "stderr": counts_projection(tool.stderr_counts.as_ref()),
                "stdout": counts_projection(tool.stdout_counts.as_ref()),
                "tool_execution_id": tool.tool_execution_id.to_string(),
                "tool_name": tool.tool_name.as_str(),
                "truncated": tool.truncated,
            });
            (
                ContextSourceKind::ObservedToolResult,
                "observed_tool_result",
                ModelInputItem::tool_result(call_id, projection).map_err(contract_error)?,
            )
        }
        ToolExecutionState::OutcomeUnknown => {
            let details = json!({
                "execution_may_have_occurred": true,
                "outcome": "unknown",
                "provider_tool_call_id": tool.provider_tool_call_id,
                "repeat_safety": "must_not_be_assumed_safe",
                "tool_execution_id": tool.tool_execution_id.to_string(),
                "tool_name": tool.tool_name.as_str(),
                "work_id": tool.work_id.to_string(),
            });
            (
                ContextSourceKind::SyntheticOutcomeUnknown,
                "synthetic_tool_outcome_unknown",
                ModelInputItem::synthetic_runtime_status("tool_outcome_unknown", details)
                    .map_err(contract_error)?,
            )
        }
        ToolExecutionState::InterruptedBeforeDispatch => {
            let details = json!({
                "execution_dispatched": false,
                "provider_tool_call_id": tool.provider_tool_call_id,
                "tool_execution_id": tool.tool_execution_id.to_string(),
                "tool_name": tool.tool_name.as_str(),
                "work_id": tool.work_id.to_string(),
            });
            (
                ContextSourceKind::SyntheticInterruption,
                "synthetic_tool_interruption",
                ModelInputItem::synthetic_runtime_status(
                    "tool_interrupted_before_dispatch",
                    details,
                )
                .map_err(contract_error)?,
            )
        }
        _ => {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ToolPairing,
            ));
        }
    };
    builder.add_item_source(SourceSpec {
        kind,
        identity: ContextSourceIdentity::Record {
            kind: ContextSourceRecordKind::ToolExecution,
            id: tool.tool_execution_id.to_string(),
        },
        model_role: Some(ContextModelRole::Tool),
        item_class,
        source_hash: Sha256Digest::hash_bytes(&durable_bytes),
        transform: match tool.state {
            ToolExecutionState::Completed => ContextTransformKind::InlineProjection,
            _ => ContextTransformKind::SyntheticStatus,
        },
        source_bytes: durable_bytes.len() as u64,
        rendered,
    })
}

fn artifact_projection(tool: &ContextToolResultSource) -> Vec<Value> {
    [
        tool.stdout_artifact.as_ref().map(|artifact| {
            json!({
                "artifact_id": artifact.artifact_id.to_string(),
                "captured_byte_count": artifact.captured_byte_count.get(),
                "sha256": artifact.sha256.to_string(),
                "stream": "stdout",
            })
        }),
        tool.stderr_artifact.as_ref().map(|artifact| {
            json!({
                "artifact_id": artifact.artifact_id.to_string(),
                "captured_byte_count": artifact.captured_byte_count.get(),
                "sha256": artifact.sha256.to_string(),
                "stream": "stderr",
            })
        }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn counts_projection(
    counts: Option<&crate::ports::context_source_store::ContextStreamCounts>,
) -> Option<Value> {
    counts.map(|counts| {
        json!({
            "captured": counts.captured.get(),
            "observed": counts.observed.get(),
            "omitted": counts.omitted.get(),
            "returned_inline": counts.returned_inline.get(),
        })
    })
}

fn tool_semantic_value(tool: &ContextToolResultSource) -> Value {
    json!({
        "agent_step_no": tool.agent_step_no.get(),
        "artifacts": artifact_projection(tool),
        "journal_offset": tool.journal_offset.get(),
        "provider_tool_call_id": tool.provider_tool_call_id,
        "result": tool.result,
        "source_model_invocation_id": tool.source_model_invocation_id.to_string(),
        "state": tool_state_literal(tool.state),
        "stderr": counts_projection(tool.stderr_counts.as_ref()),
        "stdout": counts_projection(tool.stdout_counts.as_ref()),
        "tool_execution_id": tool.tool_execution_id.to_string(),
        "tool_name": tool.tool_name.as_str(),
        "tool_ordinal": tool.tool_ordinal.get(),
        "truncated": tool.truncated,
        "work_id": tool.work_id.to_string(),
    })
}

fn render_exact_normalized_item(
    builder: &mut AssemblyBuilder,
    identity: ContextSourceIdentity,
    output: &ContextModelOutputSource,
    item: &NormalizedModelOutputItem,
) -> Result<(), ContextAssemblyError> {
    let source_hash = normalized_output_source_hash(output);
    let source_bytes = normalized_output_source_bytes(output).len() as u64;
    let has_tool_calls = output
        .normalized_output
        .items
        .iter()
        .any(|item| matches!(item, NormalizedModelOutputItem::ToolCall { .. }));
    let (item_class, rendered) = match item {
        NormalizedModelOutputItem::Text { text }
            if output.has_committed_final_assistant && !has_tool_calls =>
        {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
        NormalizedModelOutputItem::Text { text } => (
            "model_text",
            ModelInputItem::prior_assistant(vec![model_text(text)?]).map_err(contract_error)?,
        ),
        NormalizedModelOutputItem::ToolCall {
            call_id,
            tool_name,
            arguments_json,
        } => {
            let call = CanonicalModelToolCall::try_new(
                ModelToolCallId::try_new(call_id.clone()).map_err(contract_error)?,
                tool_name.as_str(),
                arguments_json.clone(),
            )
            .map_err(contract_error)?;
            call.require_valid_arguments().map_err(contract_error)?;
            ("tool_call", ModelInputItem::ToolCall(call))
        }
        NormalizedModelOutputItem::StructuredData { canonical_json } => (
            "structured_data",
            ModelInputItem::structured_data(serde_json::from_str(canonical_json).map_err(
                |_| ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation),
            )?)
            .map_err(contract_error)?,
        ),
        NormalizedModelOutputItem::Refusal { text } => (
            "historical_refusal",
            ModelInputItem::historical_refusal(vec![model_text(text)?]).map_err(contract_error)?,
        ),
        NormalizedModelOutputItem::ReasoningSummary { text } => (
            "historical_reasoning_summary",
            ModelInputItem::historical_reasoning_summary(vec![model_text(text)?])
                .map_err(contract_error)?,
        ),
        NormalizedModelOutputItem::ProviderOpaque { .. }
        | NormalizedModelOutputItem::UnknownProviderItem { .. } => {
            return Err(ContextAssemblyError::new(
                ContextAssemblyErrorKind::ReconstructionDrift,
            ));
        }
    };
    builder.add_item_source(SourceSpec {
        kind: ContextSourceKind::CompletedModelOutput,
        identity,
        model_role: Some(ContextModelRole::Assistant),
        item_class,
        source_hash,
        transform: ContextTransformKind::Identity,
        source_bytes,
        rendered,
    })
}

fn continuation_is_eligible(
    snapshot: &ContextEligibilitySnapshot,
    output: &ContextModelOutputSource,
    selection: &ModelSelectionResult,
) -> bool {
    let selected = selection.selected_target();
    if output.work_id != snapshot.active_work.work_id
        || output.provider_model.provider_id() != selected.reference().provider_id()
        || output.provider_model.provider_model_id() != selected.reference().provider_model_id()
        || output.provider_model.target_configuration_version()
            != selected.reference().target_configuration_version()
        || !selected.reference().capabilities().reasoning_continuation()
        || !selected.provider_native_options().reasoning_continuation()
        || !output
            .normalized_output
            .items
            .iter()
            .any(|item| !matches!(item, NormalizedModelOutputItem::ProviderOpaque { .. }))
    {
        return false;
    }

    let candidate_position = snapshot
        .continuation_boundaries
        .iter()
        .position(|boundary| {
            matches!(
                boundary,
                ContextContinuationBoundary::Model {
                    model_invocation_id,
                    logical_invocation_id,
                    work_id,
                    state: ModelInvocationState::Completed,
                    ..
                } if *model_invocation_id == output.model_invocation_id
                    && *logical_invocation_id == output.logical_invocation_id
                    && *work_id == snapshot.active_work.work_id
            )
        });
    let Some(candidate_position) = candidate_position else {
        return false;
    };
    let last_completed_position = snapshot
        .continuation_boundaries
        .iter()
        .enumerate()
        .rev()
        .find(|(_, boundary)| {
            matches!(
                boundary,
                ContextContinuationBoundary::Model {
                    work_id,
                    state: ModelInvocationState::Completed,
                    ..
                } if *work_id == snapshot.active_work.work_id
            )
        })
        .map(|(index, _)| index);
    if last_completed_position != Some(candidate_position) {
        return false;
    }
    !snapshot.continuation_boundaries[candidate_position + 1..]
        .iter()
        .any(|boundary| match boundary {
            ContextContinuationBoundary::Model { work_id, state, .. }
                if *work_id == snapshot.active_work.work_id =>
            {
                matches!(
                    state,
                    ModelInvocationState::Failed
                        | ModelInvocationState::CancelledLocally
                        | ModelInvocationState::ProviderOutcomeUnknown
                )
            }
            ContextContinuationBoundary::Tool { work_id, state, .. }
                if *work_id == snapshot.active_work.work_id =>
            {
                *state == ToolExecutionState::OutcomeUnknown
            }
            _ => false,
        })
}

fn exactly_one_artifact(
    output: &ContextModelOutputSource,
    artifact_id: crate::domain::ArtifactId,
    sha256: Sha256Digest,
) -> Result<&ContextArtifactDescriptor, ContextAssemblyError> {
    let matches = output
        .provider_opaque_artifacts
        .iter()
        .filter(|artifact| artifact.artifact_id == artifact_id && artifact.sha256 == sha256)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ContextAssemblyError::new(if matches.is_empty() {
            ContextAssemblyErrorKind::MissingArtifact
        } else {
            ContextAssemblyErrorKind::DuplicateSource
        }));
    }
    Ok(matches[0])
}

fn normalized_output_source_hash(output: &ContextModelOutputSource) -> Sha256Digest {
    Sha256Digest::hash_bytes(&normalized_output_source_bytes(output))
}

fn normalized_output_source_bytes(output: &ContextModelOutputSource) -> Vec<u8> {
    let items = output
        .normalized_output
        .items
        .iter()
        .map(|item| match item {
            NormalizedModelOutputItem::Text { text } => json!({"kind": "text", "text": text}),
            NormalizedModelOutputItem::ToolCall {
                call_id,
                tool_name,
                arguments_json,
            } => json!({
                "arguments_json": arguments_json,
                "call_id": call_id,
                "kind": "tool_call",
                "tool_name": tool_name.as_str(),
            }),
            NormalizedModelOutputItem::StructuredData { canonical_json } => json!({
                "canonical_json": canonical_json,
                "kind": "structured_data",
            }),
            NormalizedModelOutputItem::Refusal { text } => {
                json!({"kind": "refusal", "text": text})
            }
            NormalizedModelOutputItem::ReasoningSummary { text } => {
                json!({"kind": "reasoning_summary", "text": text})
            }
            NormalizedModelOutputItem::ProviderOpaque {
                provider_id,
                item_type,
                sha256,
                artifact_id,
            } => json!({
                "artifact_id": artifact_id.to_string(),
                "item_type": item_type,
                "kind": "provider_opaque",
                "provider_id": provider_id.as_str(),
                "sha256": sha256.to_string(),
            }),
            NormalizedModelOutputItem::UnknownProviderItem { item_type, sha256 } => json!({
                "item_type": item_type,
                "kind": "unknown_provider_item",
                "sha256": sha256.to_string(),
            }),
        })
        .collect::<Vec<_>>();
    canonical_json_bytes(&json!({
        "agent_step_no": output.agent_step_no.get(),
        "items": items,
        "logical_invocation_id": output.logical_invocation_id.to_string(),
        "model_invocation_id": output.model_invocation_id.to_string(),
        "stop_reason": output.stop_reason,
        "work_id": output.work_id.to_string(),
    }))
}

fn complete_request_units(
    request: &ModelRequest,
    request_bytes: u64,
) -> Result<Vec<TokenEstimateUnit>, ContextAssemblyError> {
    let mut units = Vec::new();
    let mut represented = 0_u64;
    for instruction in request.instructions() {
        let length = u64::try_from(
            serde_json::to_vec(instruction.as_str())
                .expect("validated instruction serializes")
                .len(),
        )
        .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        represented = represented.checked_add(length).ok_or_else(|| {
            ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
        })?;
        units.push(TokenEstimateUnit::TextBytes(length));
    }
    for item in request.ordered_input_items() {
        let length = u64::try_from(item.canonical_bytes().len())
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        represented = represented.checked_add(length).ok_or_else(|| {
            ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
        })?;
        units.push(match item {
            ModelInputItem::Message { .. }
            | ModelInputItem::PriorAssistant { .. }
            | ModelInputItem::HistoricalRefusal { .. }
            | ModelInputItem::HistoricalReasoningSummary { .. } => {
                TokenEstimateUnit::TextBytes(length)
            }
            ModelInputItem::ProviderOpaqueContinuation(_) => {
                TokenEstimateUnit::ProviderOpaqueBytes(length)
            }
            ModelInputItem::ToolCall(_)
            | ModelInputItem::ToolResult { .. }
            | ModelInputItem::StructuredData { .. }
            | ModelInputItem::SyntheticRuntimeStatus { .. } => {
                TokenEstimateUnit::StructuredBytes(length)
            }
        });
    }
    for tool in request.tool_definitions() {
        let length = u64::try_from(tool.canonical_bytes().len())
            .map_err(|_| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
        represented = represented.checked_add(length).ok_or_else(|| {
            ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow)
        })?;
        units.push(TokenEstimateUnit::ToolDefinitionBytes(length));
    }
    // Exact residual covers request IDs, selected target, option/tool-choice framing, separators,
    // and any escaping not already covered. If components exceed the envelope, the estimate remains
    // conservative and no negative residual is created.
    if request_bytes > represented {
        units.push(TokenEstimateUnit::StructuredBytes(
            request_bytes - represented,
        ));
    }
    Ok(units)
}

/// Exact inclusive byte-limit helper used by boundary tests and assembly.
pub fn validate_request_byte_limit(
    serialized_bytes: u64,
    limit: u64,
) -> Result<(), ContextAssemblyError> {
    if serialized_bytes <= limit {
        Ok(())
    } else {
        Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ContextLimitExceeded,
        ))
    }
}

/// Exact checked full-history fit helper. Equality is accepted.
pub fn validate_token_fit(
    estimated_input_tokens: u64,
    requested_output_tokens: u64,
    context_window_tokens: u64,
) -> Result<(), ContextAssemblyError> {
    let total = estimated_input_tokens
        .checked_add(requested_output_tokens)
        .ok_or_else(|| ContextAssemblyError::new(ContextAssemblyErrorKind::ArithmeticOverflow))?;
    if total <= context_window_tokens {
        Ok(())
    } else {
        Err(ContextAssemblyError::new(
            ContextAssemblyErrorKind::ContextLimitExceeded,
        ))
    }
}

struct ManifestHashInput<'a> {
    context_manifest_id: ContextManifestId,
    logical_invocation_id: LogicalInvocationId,
    work_id: WorkId,
    eligibility_conversation_id: crate::domain::ConversationId,
    active_work_ordinal: i64,
    highest_prior_terminal_work_ordinal: Option<i64>,
    input_event_ids: &'a [crate::domain::JournalEventId],
    active_output_record_ids: &'a [String],
    maximum_journal_offset: crate::domain::JournalOffset,
    selection: &'a ModelSelectionResult,
    assembler_version: &'a str,
    context_policy_version: &'a str,
    prompt_version: &'a str,
    prompt_fingerprint: Sha256Digest,
    toolset_fingerprint: Sha256Digest,
    canonical_byte_count: u64,
    request_byte_count: u64,
    estimated_input_tokens: u64,
    context_window_tokens: u64,
    requested_output_tokens: u64,
    utilization_basis_points: u16,
    transformed_source_count: u64,
    request_sha256: Sha256Digest,
    sources: &'a [PreparedContextSource],
}

fn semantic_manifest_hash(input: ManifestHashInput<'_>) -> Sha256Digest {
    let target = input.selection.selected_target();
    let capabilities = target.reference().capabilities();
    let sources = input
        .sources
        .iter()
        .map(source_semantic_value)
        .collect::<Vec<_>>();
    let semantic = json!({
        "active_output_record_ids": input.active_output_record_ids,
        "active_work_ordinal": input.active_work_ordinal,
        "assembler_version": input.assembler_version,
        "canonical_byte_count": input.canonical_byte_count,
        "context_manifest_id": input.context_manifest_id.to_string(),
        "context_policy_version": input.context_policy_version,
        "context_window_tokens": input.context_window_tokens,
        "eligibility_conversation_id": input.eligibility_conversation_id.to_string(),
        "estimated_input_tokens": input.estimated_input_tokens,
        "estimator": {
            "id": target.estimator().id(),
            "version": target.estimator().version(),
        },
        "highest_prior_terminal_work_ordinal": input.highest_prior_terminal_work_ordinal,
        "input_event_ids": input.input_event_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "logical_invocation_id": input.logical_invocation_id.to_string(),
        "maximum_journal_offset": input.maximum_journal_offset.get(),
        "omitted_source_count": 0,
        "prompt_fingerprint": input.prompt_fingerprint.to_string(),
        "prompt_version": input.prompt_version,
        "provider_native_options": {
            "reasoning_continuation": target.provider_native_options().reasoning_continuation(),
        },
        "rendered_request_byte_count": input.request_byte_count,
        "rendered_request_sha256": input.request_sha256.to_string(),
        "reserved_output_tokens": input.requested_output_tokens,
        "source_count": sources.len(),
        "sources": sources,
        "target": {
            "capabilities": {
                "context_window_tokens": capabilities.context_window_tokens().get(),
                "custom_tool_calling": capabilities.custom_tool_calling(),
                "max_output_tokens": capabilities.max_output_tokens().get(),
                "ordered_output_items": capabilities.ordered_output_items(),
                "reasoning_continuation": capabilities.reasoning_continuation(),
                "streaming": capabilities.streaming(),
                "structured_output": capabilities.structured_output(),
                "text_input": capabilities.text_input(),
                "text_output": capabilities.text_output(),
            },
            "model_target_id": target.reference().model_target_id().as_str(),
            "provider_id": target.reference().provider_id().as_str(),
            "provider_model_id": target.reference().provider_model_id().as_str(),
            "target_configuration_version": target.reference().target_configuration_version().get(),
        },
        "toolset_fingerprint": input.toolset_fingerprint.to_string(),
        "transformed_source_count": input.transformed_source_count,
        "utilization_basis_points": input.utilization_basis_points,
        "work_id": input.work_id.to_string(),
    });
    Sha256Digest::hash_bytes(&canonical_json_bytes(&semantic))
}

fn semantic_manifest_hash_from_prepared(
    manifest: &PreparedContextManifest,
    selection: &ModelSelectionResult,
    prompt_version: &str,
    sources: &[PreparedContextSource],
) -> Sha256Digest {
    semantic_manifest_hash(ManifestHashInput {
        context_manifest_id: manifest.context_manifest_id,
        logical_invocation_id: manifest.logical_invocation_id,
        work_id: manifest.work_id,
        eligibility_conversation_id: manifest.eligibility_conversation_id,
        active_work_ordinal: manifest.active_work_ordinal,
        highest_prior_terminal_work_ordinal: manifest.highest_prior_terminal_work_ordinal,
        input_event_ids: &manifest.input_event_ids,
        active_output_record_ids: &manifest.active_output_record_ids,
        maximum_journal_offset: manifest.maximum_journal_offset,
        selection,
        assembler_version: &manifest.assembler_version,
        context_policy_version: &manifest.context_policy_version,
        prompt_version,
        prompt_fingerprint: manifest.system_prompt_fingerprint,
        toolset_fingerprint: manifest.toolset_fingerprint,
        canonical_byte_count: manifest.canonical_byte_count.get(),
        request_byte_count: manifest.rendered_request_byte_count.get(),
        estimated_input_tokens: manifest.estimated_input_tokens,
        context_window_tokens: manifest.context_window_tokens,
        requested_output_tokens: manifest.reserved_output_tokens,
        utilization_basis_points: manifest.utilization_basis_points,
        transformed_source_count: manifest.transformed_source_count,
        request_sha256: manifest.rendered_request_sha256,
        sources,
    })
}

fn source_semantic_value(source: &PreparedContextSource) -> Value {
    json!({
        "identity": source_identity_key(&source.identity),
        "item_class": source.item_class,
        "kind": source_kind_literal(source.kind),
        "model_role": source.model_role.map(model_role_literal),
        "position": source.position,
        "rendered_byte_contribution": source.rendered_byte_contribution.get(),
        "source_content_sha256": source.source_content_sha256.to_string(),
        "transform": transform_literal(source.transform),
        "transformed": source.transformed,
    })
}

fn source_identity_key(identity: &ContextSourceIdentity) -> String {
    match identity {
        ContextSourceIdentity::Event(id) => format!("event:{id}"),
        ContextSourceIdentity::Artifact(id) => format!("artifact:{id}"),
        ContextSourceIdentity::Record { kind, id } => {
            format!("record:{}:{id}", source_record_kind_literal(*kind))
        }
    }
}

fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("semantic JSON must serialize")
}

fn model_text(value: &str) -> Result<ModelTextPart, ContextAssemblyError> {
    ModelTextPart::try_new(value.to_owned()).map_err(contract_error)
}

fn contract_error(_: crate::domain::ModelContractError) -> ContextAssemblyError {
    ContextAssemblyError::new(ContextAssemblyErrorKind::ContractViolation)
}

fn work_state_literal(state: WorkState) -> &'static str {
    match state {
        WorkState::Queued => "queued",
        WorkState::Running => "running",
        WorkState::WaitingOnModel => "waiting_on_model",
        WorkState::WaitingOnTool => "waiting_on_tool",
        WorkState::CancelRequested => "cancel_requested",
        WorkState::Completed => "completed",
        WorkState::Failed => "failed",
        WorkState::Cancelled => "cancelled",
        WorkState::Interrupted => "interrupted",
    }
}

fn tool_state_literal(state: ToolExecutionState) -> &'static str {
    match state {
        ToolExecutionState::Requested => "requested",
        ToolExecutionState::Dispatching => "dispatching",
        ToolExecutionState::Completed => "completed",
        ToolExecutionState::InterruptedBeforeDispatch => "interrupted_before_dispatch",
        ToolExecutionState::OutcomeUnknown => "outcome_unknown",
    }
}

fn source_kind_literal(kind: ContextSourceKind) -> &'static str {
    match kind {
        ContextSourceKind::SystemInstruction => "system_instruction",
        ContextSourceKind::DeveloperInstruction => "developer_instruction",
        ContextSourceKind::WorkstationCapabilitySummary => "workstation_capability_summary",
        ContextSourceKind::WorkspaceIdentity => "workspace_identity",
        ContextSourceKind::ToolDefinition => "tool_definition",
        ContextSourceKind::UserMessage => "user_message",
        ContextSourceKind::ActiveTrigger => "active_trigger",
        ContextSourceKind::AssistantMessage => "assistant_message",
        ContextSourceKind::CompletedModelOutput => "completed_model_output",
        ContextSourceKind::ObservedToolResult => "observed_tool_result",
        ContextSourceKind::ArtifactContent => "artifact_content",
        ContextSourceKind::SyntheticFailure => "synthetic_failure",
        ContextSourceKind::SyntheticInterruption => "synthetic_interruption",
        ContextSourceKind::SyntheticOutcomeUnknown => "synthetic_outcome_unknown",
        ContextSourceKind::SyntheticDraftStatus => "synthetic_draft_status",
        ContextSourceKind::ProviderNativeContinuation => "provider_native_continuation",
    }
}

fn source_record_kind_literal(kind: ContextSourceRecordKind) -> &'static str {
    match kind {
        ContextSourceRecordKind::InstructionVersion => "instruction_version",
        ContextSourceRecordKind::Workstation => "workstation",
        ContextSourceRecordKind::Workspace => "workspace",
        ContextSourceRecordKind::ToolDefinition => "tool_definition",
        ContextSourceRecordKind::Message => "message",
        ContextSourceRecordKind::ModelInvocation => "model_invocation",
        ContextSourceRecordKind::ToolExecution => "tool_execution",
        ContextSourceRecordKind::Work => "work",
    }
}

fn model_role_literal(role: ContextModelRole) -> &'static str {
    match role {
        ContextModelRole::System => "system",
        ContextModelRole::Developer => "developer",
        ContextModelRole::User => "user",
        ContextModelRole::Assistant => "assistant",
        ContextModelRole::Tool => "tool",
    }
}

fn transform_literal(transform: ContextTransformKind) -> &'static str {
    match transform {
        ContextTransformKind::Identity => "identity",
        ContextTransformKind::InlineProjection => "inline_projection",
        ContextTransformKind::SyntheticStatus => "synthetic_status",
        ContextTransformKind::ProviderContinuation => "provider_continuation",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::json;

    use crate::application::model_selection::{ModelSelectionPolicy, ModelTargetSnapshot};
    use crate::application::tool_registry::ToolSemanticPolicy;
    use crate::domain::{
        AgentStepNo, ArtifactId, ArtifactStorageKey, ClientMessageId, ContentBlock, ConversationId,
        ConversationWorkOrdinal, DeviceId, ErrorCategory, ErrorCode, Message, MessageContent,
        MessageId, MessageInput, MessageRole, ModelCapabilitySnapshot,
        ModelCapabilitySnapshotInput, ModelConfigReference, ModelTarget, ModelTargetId,
        ModelTargetInput, ProviderId, ProviderModelId, ProviderModelReference,
        ProviderNativeOptions, Retryability, TargetConfigurationVersion, TokenCount,
        TokenEstimatorIdentity, ToolExecutionId, ToolName, ToolOrdinal, WorkState, WorkspaceId,
        WorkstationId,
    };
    use crate::ports::artifact_store::{
        ArtifactCapture, ArtifactOrphanReport, ArtifactStoreError, ArtifactStoreErrorKind,
        BeginArtifactCapture,
    };
    use crate::ports::clock::{MonotonicInstant, TestClock};
    use crate::ports::context_source_store::{
        ContextAssistantMessageSource, ContextMessageSource, ContextToolResultSource,
        ContextWorkspaceSource, ContextWorkstationSource,
    };
    use crate::ports::model_provider::{
        ConservativeTokenEstimate, ProviderError, ProviderErrorKind, ProviderOutcomeCertainty,
    };
    use crate::ports::state_store::NormalizedModelOutput;

    fn now() -> UtcTimestamp {
        "2026-08-31T12:00:00.000000Z".parse().unwrap()
    }

    fn message(
        message_id: MessageId,
        conversation_id: ConversationId,
        role: MessageRole,
        text: &str,
        work_id: Option<WorkId>,
    ) -> Message {
        Message::try_new(MessageInput {
            message_id,
            craxii_id: crate::domain::CraxiiId::generate(),
            conversation_id,
            role,
            content: MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
            produced_by_work_id: work_id,
            device_id: (role == MessageRole::User).then(DeviceId::generate),
            client_message_id: (role == MessageRole::User).then(|| {
                ClientMessageId::parse_canonical(&uuid::Uuid::now_v7().hyphenated().to_string())
                    .unwrap()
            }),
            committed_at: now(),
        })
        .unwrap()
    }

    fn work(
        work_id: WorkId,
        conversation_id: ConversationId,
        ordinal: i64,
        workspace_id: WorkspaceId,
        state: WorkState,
    ) -> ContextWorkSource {
        ContextWorkSource {
            work_id,
            conversation_id,
            ordinal: ConversationWorkOrdinal::try_new(ordinal).unwrap(),
            workspace_id,
            state,
            terminal_reason: state.is_terminal().then(|| "fixture_terminal".to_owned()),
            terminal_journal_offset: state
                .is_terminal()
                .then(|| crate::domain::JournalOffset::try_new(ordinal + 10).unwrap()),
        }
    }

    fn base_snapshot() -> ContextEligibilitySnapshot {
        let conversation_id = ConversationId::generate();
        let work_id = WorkId::generate();
        let workspace_id = WorkspaceId::generate();
        let workstation_id = WorkstationId::generate();
        let trigger_message = message(
            MessageId::generate(),
            conversation_id,
            MessageRole::User,
            "current question",
            None,
        );
        let event_id = crate::domain::JournalEventId::generate();
        let workstation_semantic =
            json!({"generation": 1, "workstation_id": workstation_id.to_string()});
        let workspace_semantic =
            json!({"logical_name": "primary", "workspace_id": workspace_id.to_string()});
        ContextEligibilitySnapshot {
            active_work: work(
                work_id,
                conversation_id,
                1,
                workspace_id,
                WorkState::Running,
            ),
            active_trigger: ContextMessageSource {
                work_id,
                work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                input_event_id: event_id,
                journal_offset: crate::domain::JournalOffset::try_new(3).unwrap(),
                message: trigger_message,
            },
            prior_works: Vec::new(),
            prior_messages: Vec::new(),
            prior_final_assistant_messages: Vec::new(),
            completed_model_outputs: Vec::new(),
            observed_tool_results: Vec::new(),
            continuation_boundaries: Vec::new(),
            workstation: ContextWorkstationSource {
                workstation_id,
                source_sha256: Sha256Digest::hash_bytes(
                    &serde_json::to_vec(&workstation_semantic).unwrap(),
                ),
                semantic_json: workstation_semantic,
            },
            workspace: ContextWorkspaceSource {
                workspace_id,
                source_sha256: Sha256Digest::hash_bytes(
                    &serde_json::to_vec(&workspace_semantic).unwrap(),
                ),
                semantic_json: workspace_semantic,
            },
            highest_prior_terminal_work_ordinal: None,
            maximum_journal_offset: crate::domain::JournalOffset::try_new(3).unwrap(),
            exact_input_event_ids: vec![event_id],
            active_output_record_ids: Vec::new(),
        }
    }

    struct MutableSourceStore(Mutex<ContextEligibilitySnapshot>);

    impl MutableSourceStore {
        fn set(&self, snapshot: ContextEligibilitySnapshot) {
            *self.0.lock().unwrap() = snapshot;
        }
    }

    impl ContextSourceStore for MutableSourceStore {
        fn load_context_eligibility_snapshot(
            &self,
            _: ContextEligibilityRequest,
        ) -> crate::ports::context_source_store::ContextSourceStoreFuture<
            '_,
            ContextEligibilitySnapshot,
        > {
            Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
        }

        fn reload_context_sources(
            &self,
            request: ContextReconstructionRequest,
        ) -> crate::ports::context_source_store::ContextSourceStoreFuture<
            '_,
            ContextReconstructionSnapshot,
        > {
            Box::pin(async move {
                if request.manifest.sources.as_slice() != request.ordered_sources.as_ref() {
                    return Err(crate::ports::context_source_store::ContextSourceStoreError::new(
                        crate::ports::context_source_store::ContextSourceStoreErrorKind::CorruptSource,
                    ));
                }
                let snapshot = self.0.lock().unwrap().clone();
                let mut reloaded = Vec::with_capacity(request.ordered_sources.len());
                for source in request.ordered_sources.iter() {
                    reloaded.push(reload_fixture_source(&snapshot, source)?);
                }
                Ok(ContextReconstructionSnapshot {
                    active_work: snapshot.active_work,
                    ordered_sources: reloaded,
                })
            })
        }
    }

    fn reload_fixture_source(
        snapshot: &ContextEligibilitySnapshot,
        source: &PreparedContextSource,
    ) -> Result<ContextReloadedSource, crate::ports::context_source_store::ContextSourceStoreError>
    {
        use crate::ports::context_source_store::{
            ContextReloadedMessageSource, ContextSourceStoreError, ContextSourceStoreErrorKind,
        };
        let missing = || ContextSourceStoreError::new(ContextSourceStoreErrorKind::MissingSource);
        match (&source.identity, source.kind) {
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::InstructionVersion,
                    ..
                },
                ContextSourceKind::SystemInstruction | ContextSourceKind::DeveloperInstruction,
            ) => Ok(ContextReloadedSource::InstructionVersion),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::ToolDefinition,
                    ..
                },
                ContextSourceKind::ToolDefinition,
            ) => Ok(ContextReloadedSource::ToolDefinition),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::Workstation,
                    id,
                },
                ContextSourceKind::WorkstationCapabilitySummary,
            ) if *id == snapshot.workstation.workstation_id.to_string() => Ok(
                ContextReloadedSource::Workstation(snapshot.workstation.clone()),
            ),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::Workspace,
                    id,
                },
                ContextSourceKind::WorkspaceIdentity,
            ) if *id == snapshot.workspace.workspace_id.to_string() => {
                Ok(ContextReloadedSource::Workspace(snapshot.workspace.clone()))
            }
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::Message,
                    id,
                },
                ContextSourceKind::UserMessage | ContextSourceKind::ActiveTrigger,
            ) => snapshot
                .prior_messages
                .iter()
                .chain(std::iter::once(&snapshot.active_trigger))
                .find(|candidate| candidate.message.message_id().to_string() == *id)
                .map(|candidate| {
                    ContextReloadedSource::Message(ContextReloadedMessageSource {
                        work_id: candidate.work_id,
                        work_ordinal: candidate.work_ordinal,
                        journal_event_id: candidate.input_event_id,
                        journal_offset: candidate.journal_offset,
                        message: candidate.message.clone(),
                    })
                })
                .ok_or_else(missing),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::Message,
                    id,
                },
                ContextSourceKind::AssistantMessage,
            ) => snapshot
                .prior_final_assistant_messages
                .iter()
                .find(|candidate| candidate.message.message_id().to_string() == *id)
                .map(|candidate| {
                    ContextReloadedSource::Message(ContextReloadedMessageSource {
                        work_id: candidate.work_id,
                        work_ordinal: candidate.work_ordinal,
                        journal_event_id: candidate.journal_event_id,
                        journal_offset: candidate.journal_offset,
                        message: candidate.message.clone(),
                    })
                })
                .ok_or_else(missing),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::ModelInvocation,
                    id,
                },
                ContextSourceKind::CompletedModelOutput
                | ContextSourceKind::ProviderNativeContinuation,
            ) => {
                let prefix = id.split(':').next().ok_or_else(missing)?;
                snapshot
                    .completed_model_outputs
                    .iter()
                    .find(|candidate| candidate.model_invocation_id.to_string() == prefix)
                    .cloned()
                    .map(ContextReloadedSource::ModelOutput)
                    .ok_or_else(missing)
            }
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::ToolExecution,
                    id,
                },
                ContextSourceKind::ObservedToolResult | ContextSourceKind::SyntheticOutcomeUnknown,
            ) => snapshot
                .observed_tool_results
                .iter()
                .find(|candidate| candidate.tool_execution_id.to_string() == *id)
                .cloned()
                .map(ContextReloadedSource::ToolResult)
                .ok_or_else(missing),
            (
                ContextSourceIdentity::Record {
                    kind: ContextSourceRecordKind::Work,
                    id,
                },
                ContextSourceKind::SyntheticFailure | ContextSourceKind::SyntheticInterruption,
            ) => snapshot
                .prior_works
                .iter()
                .find(|candidate| candidate.work_id.to_string() == *id)
                .cloned()
                .map(ContextReloadedSource::Work)
                .ok_or_else(missing),
            _ => Err(missing()),
        }
    }

    struct FixedEstimator {
        identity: TokenEstimatorIdentity,
        tokens: u64,
        returned_identity: Option<TokenEstimatorIdentity>,
        fail: bool,
    }

    impl TokenEstimator for FixedEstimator {
        fn identity(&self) -> &TokenEstimatorIdentity {
            &self.identity
        }

        fn estimate(
            &self,
            _: &ModelTarget,
            units: &[TokenEstimateUnit],
        ) -> Result<ConservativeTokenEstimate, ProviderError> {
            assert!(!units.is_empty());
            if self.fail {
                return Err(ProviderError::new(
                    ProviderErrorKind::ContextError,
                    ProviderOutcomeCertainty::DefinitelyNotSent,
                ));
            }
            ConservativeTokenEstimate::try_new(
                self.returned_identity
                    .clone()
                    .unwrap_or_else(|| self.identity.clone()),
                self.tokens,
            )
        }
    }

    fn target(
        context_window: i64,
        requested_output: i64,
        continuation: bool,
    ) -> (ModelSelectionResult, TokenEstimatorIdentity) {
        target_options(context_window, requested_output, continuation, continuation)
    }

    fn target_options(
        context_window: i64,
        requested_output: i64,
        continuation_capability: bool,
        native_continuation: bool,
    ) -> (ModelSelectionResult, TokenEstimatorIdentity) {
        let estimator = TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap();
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: continuation_capability,
            context_window_tokens: TokenCount::try_new(context_window).unwrap(),
            max_output_tokens: TokenCount::try_new(requested_output).unwrap(),
        });
        let target = ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new("fixture").unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new("fixture-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("fixture-account").unwrap(),
            requested_output_tokens: TokenCount::try_new(requested_output).unwrap(),
            estimator: estimator.clone(),
            provider_native_options: ProviderNativeOptions::new(native_continuation),
        })
        .unwrap();
        let snapshot = Arc::new(
            ModelTargetSnapshot::try_new(
                target.reference().model_target_id().clone(),
                vec![target],
            )
            .unwrap(),
        );
        let required = crate::domain::model::RequiredModelCapabilities {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            required_output_tokens: TokenCount::try_new(1).unwrap(),
        };
        (
            ModelSelectionPolicy::new(snapshot)
                .select(None, required)
                .unwrap(),
            estimator,
        )
    }

    fn registry() -> Arc<ToolRegistry> {
        Arc::new(
            ToolRegistry::v0(ToolSemanticPolicy {
                read_file_default_bytes: 4_096,
                read_file_max_bytes: 65_536,
                run_shell_command_max_bytes: 65_536,
                run_shell_default_timeout_ms: 60_000,
                run_shell_max_timeout_ms: 900_000,
            })
            .unwrap(),
        )
    }

    fn clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            now().to_offset_datetime(),
            Duration::from_secs(1),
        ))
    }

    fn test_assembler(
        store: Arc<MutableSourceStore>,
        estimator: TokenEstimatorIdentity,
        tokens: u64,
    ) -> ContextAssembler {
        ContextAssembler::new(
            store,
            None,
            Arc::new(FixedEstimator {
                identity: estimator,
                tokens,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        )
    }

    #[tokio::test]
    async fn single_current_message_builds_exact_immutable_package_and_manifest() {
        let snapshot = base_snapshot();
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert_eq!(result.prepared_sources().len(), 7);
        assert_eq!(
            result.request().ordered_input_items(),
            result.package().ordered_input_items()
        );
        assert_eq!(
            result.request().tool_definitions(),
            result.package().tool_definitions()
        );
        assert_eq!(
            model_toolset_fingerprint(result.request().tool_definitions()),
            result.prepared_manifest().toolset_fingerprint
        );
        assert_eq!(result.request().requested_output_limit().get(), 100);
        assert_eq!(result.package().selected_target(), &selection);
        assert_eq!(result.package().requested_output_tokens(), 100);
        assert_eq!(
            result.package().provider_native_options(),
            selection.selected_target().provider_native_options()
        );
        assert_eq!(
            result
                .package()
                .tool_definitions()
                .iter()
                .map(|definition| definition.name().as_str())
                .collect::<Vec<_>>(),
            vec!["read_file", "run_shell"]
        );
        assert_eq!(result.prepared_manifest().reserved_output_tokens, 100);
        assert_eq!(result.prepared_manifest().omitted_source_count, 0);
        assert_eq!(
            result.request().canonical_bytes().len() as u64,
            result.prepared_manifest().rendered_request_byte_count.get()
        );
        assert_eq!(
            result.budget().request_serialized_bytes,
            result.prepared_manifest().rendered_request_byte_count.get()
        );
        assert_eq!(
            result.request().canonical_sha256(),
            result.prepared_manifest().rendered_request_sha256
        );
        assert!(
            result
                .prepared_sources()
                .iter()
                .enumerate()
                .all(|(index, source)| source.position == index as i64 + 1)
        );
    }

    #[test]
    fn instruction_snapshot_is_stable_ordered_time_free_and_sensitive() {
        let first = VersionedInstructionSnapshot::v0();
        let same = VersionedInstructionSnapshot::v0();
        assert_eq!(first, same);
        assert_eq!(first.version(), V0_INSTRUCTION_VERSION);
        assert_eq!(first.system().len(), 1);
        assert_eq!(first.developer().len(), 1);
        assert!(!String::from_utf8_lossy(first.canonical_bytes()).contains("2026-"));
        assert!(!String::from_utf8_lossy(first.canonical_bytes()).contains("api_key"));
        let changed = VersionedInstructionSnapshot::try_new(
            V0_INSTRUCTION_VERSION,
            vec![ModelTextPart::try_new("changed").unwrap()],
            first.developer().to_vec(),
        )
        .unwrap();
        assert_ne!(first.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn token_and_request_byte_boundaries_are_inclusive_and_checked() {
        assert!(validate_token_fit(90, 10, 100).is_ok());
        assert_eq!(
            validate_token_fit(91, 10, 100).unwrap_err().kind(),
            ContextAssemblyErrorKind::ContextLimitExceeded
        );
        assert_eq!(
            validate_token_fit(u64::MAX, 1, u64::MAX)
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ArithmeticOverflow
        );
        assert!(validate_request_byte_limit(16_777_216, 16_777_216).is_ok());
        assert!(validate_request_byte_limit(16_777_217, 16_777_216).is_err());
    }

    struct CountingEstimator {
        identity: TokenEstimatorIdentity,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }

    impl TokenEstimator for CountingEstimator {
        fn identity(&self) -> &TokenEstimatorIdentity {
            &self.identity
        }

        fn estimate(
            &self,
            _: &ModelTarget,
            _: &[TokenEstimateUnit],
        ) -> Result<ConservativeTokenEstimate, ProviderError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail {
                return Err(ProviderError::new(
                    ProviderErrorKind::ContextError,
                    ProviderOutcomeCertainty::DefinitelyNotSent,
                ));
            }
            ConservativeTokenEstimate::try_new(self.identity.clone(), 1)
        }
    }

    fn large_request_snapshot(
        full_prior_messages: usize,
        tail: Option<String>,
    ) -> ContextEligibilitySnapshot {
        let mut snapshot = base_snapshot();
        let active_ordinal =
            i64::try_from(full_prior_messages + usize::from(tail.is_some()) + 1).unwrap();
        snapshot.active_work.ordinal = ConversationWorkOrdinal::try_new(active_ordinal).unwrap();
        snapshot.active_trigger.work_ordinal = snapshot.active_work.ordinal;
        let mut texts =
            vec!["a".repeat(crate::domain::MAX_CONTENT_TEXT_BYTES); full_prior_messages];
        if let Some(tail) = tail {
            texts.push(tail);
        }
        for (index, text) in texts.into_iter().enumerate() {
            let ordinal = i64::try_from(index + 1).unwrap();
            let work_id = WorkId::generate();
            snapshot.prior_works.push(work(
                work_id,
                snapshot.active_work.conversation_id,
                ordinal,
                snapshot.active_work.workspace_id,
                WorkState::Cancelled,
            ));
            let event_id = crate::domain::JournalEventId::generate();
            snapshot.prior_messages.push(ContextMessageSource {
                work_id,
                work_ordinal: ConversationWorkOrdinal::try_new(ordinal).unwrap(),
                input_event_id: event_id,
                journal_offset: crate::domain::JournalOffset::try_new(ordinal + 10).unwrap(),
                message: message(
                    MessageId::generate(),
                    snapshot.active_work.conversation_id,
                    MessageRole::User,
                    &text,
                    None,
                ),
            });
            snapshot
                .exact_input_event_ids
                .insert(snapshot.exact_input_event_ids.len() - 1, event_id);
        }
        snapshot.highest_prior_terminal_work_ordinal =
            snapshot.prior_works.last().map(|work| work.ordinal);
        snapshot.maximum_journal_offset =
            crate::domain::JournalOffset::try_new(active_ordinal + 20).unwrap();
        snapshot
    }

    async fn assemble_with_counting_estimator(
        snapshot: ContextEligibilitySnapshot,
        selection: &ModelSelectionResult,
        identity: TokenEstimatorIdentity,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    ) -> Result<ContextAssemblyResult, ContextAssemblyError> {
        let work_id = snapshot.active_work.work_id;
        ContextAssembler::new(
            Arc::new(MutableSourceStore(Mutex::new(snapshot))),
            None,
            Arc::new(CountingEstimator {
                identity,
                calls,
                fail,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        )
        .assemble(work_id, selection, &ContextAssemblyVersions::v0())
        .await
    }

    #[tokio::test]
    async fn request_byte_ceiling_precedes_estimator_and_uses_actual_serialization() {
        let (selection, estimator) = target(2_000_000_000, 1, false);
        let setup_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let base = assemble_with_counting_estimator(
            large_request_snapshot(255, None),
            &selection,
            estimator.clone(),
            setup_calls.clone(),
            false,
        )
        .await
        .unwrap();
        let one = assemble_with_counting_estimator(
            large_request_snapshot(255, Some("x".to_owned())),
            &selection,
            estimator.clone(),
            setup_calls.clone(),
            false,
        )
        .await
        .unwrap();
        let fixed_tail_overhead =
            one.budget().request_serialized_bytes - base.budget().request_serialized_bytes - 1;
        let tail_length = usize::try_from(
            MAX_CANONICAL_MODEL_REQUEST_BYTES
                - base.budget().request_serialized_bytes
                - fixed_tail_overhead,
        )
        .unwrap();
        assert!((1..=crate::domain::MAX_CONTENT_TEXT_BYTES).contains(&tail_length));

        let exact_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let exact = assemble_with_counting_estimator(
            large_request_snapshot(255, Some("x".repeat(tail_length))),
            &selection,
            estimator.clone(),
            exact_calls.clone(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            exact.budget().request_serialized_bytes,
            MAX_CANONICAL_MODEL_REQUEST_BYTES
        );
        assert_eq!(exact_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let oversized_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let oversized = assemble_with_counting_estimator(
            large_request_snapshot(255, Some("x".repeat(tail_length + 1))),
            &selection,
            TokenEstimatorIdentity::try_new("mismatched_v1", 1).unwrap(),
            oversized_calls.clone(),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(
            oversized.kind(),
            ContextAssemblyErrorKind::ContextLimitExceeded
        );
        assert_eq!(oversized_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let evidence = oversized.limit_evidence().unwrap();
        assert_eq!(evidence.request_serialized_bytes, 16_777_217);
        assert_eq!(evidence.request_byte_limit, 16_777_216);
        assert_eq!(evidence.estimated_input_tokens, 0);

        let escaped_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let escaped = assemble_with_counting_estimator(
            large_request_snapshot(0, Some("\"\\".repeat(100))),
            &selection,
            selection.selected_target().estimator().clone(),
            escaped_calls,
            false,
        )
        .await
        .unwrap();
        let plain = assemble_with_counting_estimator(
            large_request_snapshot(0, Some("aa".repeat(100))),
            &selection,
            selection.selected_target().estimator().clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            escaped.budget().request_serialized_bytes - plain.budget().request_serialized_bytes,
            200
        );
        let utf8 = assemble_with_counting_estimator(
            large_request_snapshot(0, Some("é".repeat(100))),
            &selection,
            selection.selected_target().estimator().clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            false,
        )
        .await
        .unwrap();
        let ascii = assemble_with_counting_estimator(
            large_request_snapshot(0, Some("a".repeat(100))),
            &selection,
            selection.selected_target().estimator().clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            utf8.budget().request_serialized_bytes - ascii.budget().request_serialized_bytes,
            100
        );
    }

    #[tokio::test]
    async fn one_token_over_returns_exact_safe_context_limit_evidence() {
        let snapshot = base_snapshot();
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(1_000, 100, false);
        let error = test_assembler(store, estimator, 901)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ContextAssemblyErrorKind::ContextLimitExceeded);
        assert_eq!(error.normalized().category(), ErrorCategory::ContextError);
        assert_eq!(error.normalized().code(), ErrorCode::CONTEXT_LIMIT_EXCEEDED);
        assert_eq!(error.normalized().retryability(), Retryability::Never);
        let evidence = error.limit_evidence().unwrap();
        assert_eq!(evidence.estimated_input_tokens, 901);
        assert_eq!(evidence.requested_output_tokens, 100);
        assert_eq!(evidence.reserved_output_tokens, 100);
        assert_eq!(evidence.context_window_tokens, 1_000);
        assert_eq!(evidence.estimator_version, 1);
    }

    fn add_prior_completed_turn(snapshot: &mut ContextEligibilitySnapshot) -> WorkId {
        let active_id = snapshot.active_work.work_id;
        let conversation = snapshot.active_work.conversation_id;
        let workspace = snapshot.active_work.workspace_id;
        snapshot.active_work.ordinal = ConversationWorkOrdinal::try_new(2).unwrap();
        snapshot.active_trigger.work_ordinal = ConversationWorkOrdinal::try_new(2).unwrap();
        let prior_id = WorkId::generate();
        snapshot.prior_works.push(work(
            prior_id,
            conversation,
            1,
            workspace,
            WorkState::Completed,
        ));
        let prior_event = crate::domain::JournalEventId::generate();
        snapshot.prior_messages.push(ContextMessageSource {
            work_id: prior_id,
            work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
            input_event_id: prior_event,
            journal_offset: crate::domain::JournalOffset::try_new(4).unwrap(),
            message: message(
                MessageId::generate(),
                conversation,
                MessageRole::User,
                "prior question",
                None,
            ),
        });
        snapshot.exact_input_event_ids.insert(0, prior_event);
        snapshot.highest_prior_terminal_work_ordinal =
            Some(ConversationWorkOrdinal::try_new(1).unwrap());
        assert_ne!(active_id, prior_id);
        prior_id
    }

    fn model_source(
        snapshot: &ContextEligibilitySnapshot,
        work_id: WorkId,
        ordinal: i64,
        items: Vec<NormalizedModelOutputItem>,
        has_final: bool,
    ) -> ContextModelOutputSource {
        let (selection, _) = target(100_000, 100, false);
        ContextModelOutputSource {
            model_invocation_id: crate::domain::ModelInvocationId::generate(),
            logical_invocation_id: LogicalInvocationId::generate(),
            work_id,
            conversation_id: snapshot.active_work.conversation_id,
            work_ordinal: ConversationWorkOrdinal::try_new(ordinal).unwrap(),
            agent_step_no: AgentStepNo::try_new(1).unwrap(),
            attempt_no: 1,
            provider_model: selection.selected_target().reference().clone(),
            normalized_output: NormalizedModelOutput { items },
            provider_opaque_artifacts: Vec::new(),
            stop_reason: "completed".to_owned(),
            journal_offset: crate::domain::JournalOffset::try_new(7).unwrap(),
            has_committed_final_assistant: has_final,
        }
    }

    fn add_completed_model_boundary(
        snapshot: &mut ContextEligibilitySnapshot,
        source: &ContextModelOutputSource,
    ) {
        snapshot
            .continuation_boundaries
            .push(ContextContinuationBoundary::Model {
                model_invocation_id: source.model_invocation_id,
                logical_invocation_id: source.logical_invocation_id,
                work_id: source.work_id,
                work_ordinal: source.work_ordinal,
                agent_step_no: source.agent_step_no,
                attempt_no: source.attempt_no,
                state: ModelInvocationState::Completed,
                journal_offset: source.journal_offset,
            });
    }

    #[tokio::test]
    async fn committed_final_assistant_deduplicates_terminal_model_text() {
        let mut snapshot = base_snapshot();
        let prior = add_prior_completed_turn(&mut snapshot);
        snapshot.completed_model_outputs.push(model_source(
            &snapshot,
            prior,
            1,
            vec![NormalizedModelOutputItem::Text {
                text: "duplicate final".to_owned(),
            }],
            true,
        ));
        snapshot
            .prior_final_assistant_messages
            .push(ContextAssistantMessageSource {
                work_id: prior,
                work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                journal_event_id: crate::domain::JournalEventId::generate(),
                journal_offset: crate::domain::JournalOffset::try_new(8).unwrap(),
                message: message(
                    MessageId::generate(),
                    snapshot.active_work.conversation_id,
                    MessageRole::Assistant,
                    "authoritative final",
                    Some(prior),
                ),
            });
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let assistant_count = result
            .request()
            .ordered_input_items()
            .iter()
            .filter(|item| matches!(item, ModelInputItem::PriorAssistant { .. }))
            .count();
        assert_eq!(assistant_count, 1);
        assert!(
            !String::from_utf8_lossy(&result.request().canonical_bytes())
                .contains("duplicate final")
        );
    }

    #[tokio::test]
    async fn intermediate_tool_continuation_survives_final_assistant_deduplication() {
        let mut snapshot = base_snapshot();
        let prior = add_prior_completed_turn(&mut snapshot);
        let mut intermediate = model_source(
            &snapshot,
            prior,
            1,
            vec![
                NormalizedModelOutputItem::Text {
                    text: "intermediate remains".to_owned(),
                },
                NormalizedModelOutputItem::ToolCall {
                    call_id: "prior-call".to_owned(),
                    tool_name: ToolName::try_new("read_file").unwrap(),
                    arguments_json: "{\"path\":\"README.md\"}".to_owned(),
                },
            ],
            false,
        );
        intermediate.stop_reason = "tool_continuation".to_owned();
        let intermediate_id = intermediate.model_invocation_id;
        snapshot.completed_model_outputs.push(intermediate);
        snapshot
            .observed_tool_results
            .push(ContextToolResultSource {
                tool_execution_id: ToolExecutionId::generate(),
                work_id: prior,
                conversation_id: snapshot.active_work.conversation_id,
                work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                source_model_invocation_id: intermediate_id,
                agent_step_no: AgentStepNo::try_new(1).unwrap(),
                tool_ordinal: ToolOrdinal::try_new(1).unwrap(),
                provider_tool_call_id: "prior-call".to_owned(),
                tool_name: ToolName::try_new("read_file").unwrap(),
                state: ToolExecutionState::Completed,
                result: Some(json!({"fields": [], "result_kind": "success", "summary": "read"})),
                stdout_counts: None,
                stderr_counts: None,
                stdout_artifact: None,
                stderr_artifact: None,
                truncated: false,
                journal_offset: crate::domain::JournalOffset::try_new(8).unwrap(),
            });
        let mut terminal = model_source(
            &snapshot,
            prior,
            1,
            vec![NormalizedModelOutputItem::Text {
                text: "duplicate terminal".to_owned(),
            }],
            true,
        );
        terminal.agent_step_no = AgentStepNo::try_new(2).unwrap();
        terminal.journal_offset = crate::domain::JournalOffset::try_new(9).unwrap();
        snapshot.completed_model_outputs.push(terminal);
        snapshot
            .prior_final_assistant_messages
            .push(ContextAssistantMessageSource {
                work_id: prior,
                work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
                journal_event_id: crate::domain::JournalEventId::generate(),
                journal_offset: crate::domain::JournalOffset::try_new(10).unwrap(),
                message: message(
                    MessageId::generate(),
                    snapshot.active_work.conversation_id,
                    MessageRole::Assistant,
                    "authoritative final",
                    Some(prior),
                ),
            });

        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let request_bytes = result.request().canonical_bytes();
        let request = String::from_utf8_lossy(&request_bytes);
        assert!(request.contains("intermediate remains"));
        assert!(request.contains("authoritative final"));
        assert!(!request.contains("duplicate terminal"));
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::ToolResult { .. }))
        );
    }

    fn add_tool_trace(snapshot: &mut ContextEligibilitySnapshot, state: ToolExecutionState) {
        let work_id = snapshot.active_work.work_id;
        let mut model = model_source(
            snapshot,
            work_id,
            snapshot.active_work.ordinal.get(),
            vec![
                NormalizedModelOutputItem::Text {
                    text: "I will inspect.".to_owned(),
                },
                NormalizedModelOutputItem::ToolCall {
                    call_id: "call-1".to_owned(),
                    tool_name: ToolName::try_new("read_file").unwrap(),
                    arguments_json: "{\"path\":\"README.md\"}".to_owned(),
                },
            ],
            false,
        );
        model.stop_reason = "tool_continuation".to_owned();
        let model_id = model.model_invocation_id;
        snapshot.completed_model_outputs.push(model);
        let tool_id = ToolExecutionId::generate();
        snapshot
            .observed_tool_results
            .push(ContextToolResultSource {
                tool_execution_id: tool_id,
                work_id,
                conversation_id: snapshot.active_work.conversation_id,
                work_ordinal: snapshot.active_work.ordinal,
                source_model_invocation_id: model_id,
                agent_step_no: AgentStepNo::try_new(1).unwrap(),
                tool_ordinal: ToolOrdinal::try_new(1).unwrap(),
                provider_tool_call_id: "call-1".to_owned(),
                tool_name: ToolName::try_new("read_file").unwrap(),
                state,
                result: (state == ToolExecutionState::Completed)
                    .then(|| json!({"fields": [], "result_kind": "success", "summary": "read"})),
                stdout_counts: None,
                stderr_counts: None,
                stdout_artifact: None,
                stderr_artifact: None,
                truncated: false,
                journal_offset: crate::domain::JournalOffset::try_new(8).unwrap(),
            });
        snapshot.active_output_record_ids = vec![model_id.to_string(), tool_id.to_string()];
    }

    #[tokio::test]
    async fn tool_call_and_definite_result_are_adjacent_and_exactly_paired() {
        let mut snapshot = base_snapshot();
        add_tool_trace(&mut snapshot, ToolExecutionState::Completed);
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let items = result.request().ordered_input_items();
        let call = items
            .iter()
            .position(|item| matches!(item, ModelInputItem::ToolCall(_)))
            .unwrap();
        assert!(matches!(items[call + 1], ModelInputItem::ToolResult { .. }));
    }

    #[tokio::test]
    async fn definite_failed_tool_result_remains_an_ordinary_tool_result() {
        let mut snapshot = base_snapshot();
        add_tool_trace(&mut snapshot, ToolExecutionState::Completed);
        snapshot.observed_tool_results[0].result = Some(json!({
            "error": "command_failed",
            "result_kind": "failure",
            "summary": "definite observed failure",
        }));
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let bytes = result.request().canonical_bytes();
        let rendered = String::from_utf8_lossy(&bytes);
        assert!(rendered.contains("definite observed failure"));
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::ToolResult { .. }))
        );
    }

    #[tokio::test]
    async fn outcome_unknown_is_synthetic_and_never_ordinary_tool_result() {
        let mut snapshot = base_snapshot();
        add_tool_trace(&mut snapshot, ToolExecutionState::OutcomeUnknown);
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert!(
            !result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::ToolResult { .. }))
        );
        let bytes = result.request().canonical_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("execution_may_have_occurred"));
        assert!(text.contains("must_not_be_assumed_safe"));
    }

    #[tokio::test]
    async fn newly_durable_tool_result_creates_a_distinct_new_step_package() {
        let initial_snapshot = base_snapshot();
        let work_id = initial_snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(initial_snapshot.clone())));
        let (selection, estimator) = target(100_000, 100, false);
        let assembler = test_assembler(store.clone(), estimator, 1_000);
        let initial = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();

        let mut next_snapshot = initial_snapshot;
        add_tool_trace(&mut next_snapshot, ToolExecutionState::Completed);
        store.set(next_snapshot);
        let next = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert_ne!(
            initial.package().context_manifest_id(),
            next.package().context_manifest_id()
        );
        assert_ne!(
            initial.package().logical_invocation_id(),
            next.package().logical_invocation_id()
        );
        assert_ne!(
            initial.prepared_manifest().rendered_request_sha256,
            next.prepared_manifest().rendered_request_sha256
        );
        assert!(next.prepared_sources().len() > initial.prepared_sources().len());
    }

    #[tokio::test]
    async fn orphan_and_mismatched_tool_results_fail_closed() {
        let mut snapshot = base_snapshot();
        add_tool_trace(&mut snapshot, ToolExecutionState::Completed);
        snapshot.observed_tool_results[0].provider_tool_call_id = "wrong".to_owned();
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let error = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ContextAssemblyErrorKind::ToolPairing);
    }

    #[tokio::test]
    async fn reasoning_refusal_and_structured_history_remain_distinct_items() {
        let mut snapshot = base_snapshot();
        let prior = add_prior_completed_turn(&mut snapshot);
        snapshot.completed_model_outputs.push(model_source(
            &snapshot,
            prior,
            1,
            vec![
                NormalizedModelOutputItem::ReasoningSummary {
                    text: "provider summary".to_owned(),
                },
                NormalizedModelOutputItem::StructuredData {
                    canonical_json: "{\"answer\":1}".to_owned(),
                },
                NormalizedModelOutputItem::Refusal {
                    text: "cannot comply".to_owned(),
                },
            ],
            false,
        ));
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, estimator) = target(100_000, 100, false);
        let result = test_assembler(store, estimator, 1_000)
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::HistoricalReasoningSummary { .. }))
        );
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::HistoricalRefusal { .. }))
        );
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::StructuredData { .. }))
        );
    }

    #[tokio::test]
    async fn failed_interrupted_and_cancelled_prior_work_follow_synthetic_rules() {
        for (state, expected) in [
            (WorkState::Failed, Some("prior_work_failed")),
            (WorkState::Interrupted, Some("prior_work_interrupted")),
            (WorkState::Cancelled, None),
        ] {
            let mut snapshot = base_snapshot();
            let prior = add_prior_completed_turn(&mut snapshot);
            snapshot.prior_works[0].state = state;
            snapshot.prior_works[0].terminal_reason = Some("fixture".to_owned());
            let work_id = snapshot.active_work.work_id;
            let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
            let (selection, estimator) = target(100_000, 100, false);
            let result = test_assembler(store, estimator, 1_000)
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap();
            let bytes = result.request().canonical_bytes();
            let text = String::from_utf8_lossy(&bytes);
            assert_eq!(
                expected.is_some_and(|value| text.contains(value)),
                expected.is_some()
            );
            assert!(
                text.contains("prior question"),
                "prior user input disappeared"
            );
            assert_eq!(snapshot_work_id_for_assertion(prior), prior);
        }
    }

    fn snapshot_work_id_for_assertion(value: WorkId) -> WorkId {
        value
    }

    #[tokio::test]
    async fn unknown_provider_output_and_duplicate_trigger_source_fail_closed() {
        let mut unknown = base_snapshot();
        unknown.completed_model_outputs.push(model_source(
            &unknown,
            unknown.active_work.work_id,
            1,
            vec![NormalizedModelOutputItem::UnknownProviderItem {
                item_type: "unknown".to_owned(),
                sha256: Sha256Digest::hash_bytes(b"unknown"),
            }],
            false,
        ));
        let work_id = unknown.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(unknown)));
        let (selection, estimator) = target(100_000, 100, false);
        assert_eq!(
            test_assembler(store, estimator, 1_000)
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::UnknownModelOutput
        );

        let mut duplicate = base_snapshot();
        duplicate
            .exact_input_event_ids
            .push(duplicate.active_trigger.input_event_id);
        let work_id = duplicate.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(duplicate)));
        let (selection, estimator) = target(100_000, 100, false);
        assert_eq!(
            test_assembler(store, estimator, 1_000)
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::DuplicateSource
        );
    }

    #[tokio::test]
    async fn estimator_mismatch_and_failure_have_no_fallback() {
        let snapshot = base_snapshot();
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot)));
        let (selection, identity) = target(100_000, 100, false);
        let mismatch = ContextAssembler::new(
            store.clone(),
            None,
            Arc::new(FixedEstimator {
                identity: TokenEstimatorIdentity::try_new("other_v1", 1).unwrap(),
                tokens: 1,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        );
        assert_eq!(
            mismatch
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::EstimatorMismatch
        );
        let failed = ContextAssembler::new(
            store,
            None,
            Arc::new(FixedEstimator {
                identity,
                tokens: 1,
                returned_identity: None,
                fail: true,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        );
        assert_eq!(
            failed
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::EstimatorFailure
        );
    }

    #[tokio::test]
    async fn reconstruction_reuses_ids_ignores_created_at_and_detects_source_tamper() {
        let snapshot = base_snapshot();
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot.clone())));
        let (selection, estimator) = target(100_000, 100, false);
        let assembler = test_assembler(store.clone(), estimator, 1_000);
        let result = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assembler.verify_reconstruction(&result).await.unwrap();
        let mut metadata_only = result.clone();
        metadata_only.prepared_manifest.created_at = "2026-08-31T12:00:01.000000Z".parse().unwrap();
        assembler
            .verify_reconstruction(&metadata_only)
            .await
            .unwrap();
        assert_eq!(
            metadata_only.prepared_manifest.manifest_sha256,
            result.prepared_manifest.manifest_sha256
        );
        let mut tampered = snapshot;
        tampered.active_trigger.message = message(
            MessageId::generate(),
            tampered.active_work.conversation_id,
            MessageRole::User,
            "tampered",
            None,
        );
        store.set(tampered);
        assert_eq!(
            assembler
                .verify_reconstruction(&result)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
    }

    #[tokio::test]
    async fn exact_source_reconstruction_survives_new_evidence_and_rejects_tamper_matrix() {
        let original_snapshot = base_snapshot();
        let work_id = original_snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(original_snapshot.clone())));
        let (selection, estimator) = target(100_000, 100, false);
        let assembler = test_assembler(store.clone(), estimator, 1_000);
        let prepared = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        let old_request_bytes = prepared.request().canonical_bytes();
        let old_request_hash = prepared.prepared_manifest().rendered_request_sha256;
        let old_manifest_hash = prepared.prepared_manifest().manifest_sha256;

        let mut advanced = original_snapshot.clone();
        add_tool_trace(&mut advanced, ToolExecutionState::Completed);
        store.set(advanced);
        assembler.verify_reconstruction(&prepared).await.unwrap();
        assert_eq!(prepared.request().canonical_bytes(), old_request_bytes);
        assert_eq!(
            prepared.prepared_manifest().rendered_request_sha256,
            old_request_hash
        );
        assert_eq!(
            prepared.prepared_manifest().manifest_sha256,
            old_manifest_hash
        );
        let fresh = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert_ne!(fresh.request().canonical_sha256(), old_request_hash);

        let original_message_id = original_snapshot.active_trigger.message.message_id();
        let mut content_tamper = original_snapshot.clone();
        content_tamper.active_trigger.message = message(
            original_message_id,
            content_tamper.active_work.conversation_id,
            MessageRole::User,
            "changed content under exact identity",
            None,
        );
        store.set(content_tamper);
        assert_eq!(
            assembler
                .verify_reconstruction(&prepared)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );

        let mut replacement = original_snapshot.clone();
        replacement.active_trigger.message = message(
            MessageId::generate(),
            replacement.active_work.conversation_id,
            MessageRole::User,
            "current question",
            None,
        );
        store.set(replacement);
        assert_eq!(
            assembler
                .verify_reconstruction(&prepared)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );

        store.set(original_snapshot.clone());
        let mut source_hash = prepared.clone();
        source_hash.prepared_sources[0].source_content_sha256 = Sha256Digest::hash_bytes(b"other");
        assert_eq!(
            assembler
                .verify_reconstruction(&source_hash)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
        let mut source_type = prepared.clone();
        source_type.prepared_sources[0].kind = ContextSourceKind::SyntheticFailure;
        assert_eq!(
            assembler
                .verify_reconstruction(&source_type)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
        let mut source_order = prepared.clone();
        source_order.prepared_sources.swap(0, 1);
        assert_eq!(
            assembler
                .verify_reconstruction(&source_order)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
        let mut source_position = prepared.clone();
        source_position.prepared_sources[0].position = 2;
        assert_eq!(
            assembler
                .verify_reconstruction(&source_position)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
        let mut target_version = prepared.clone();
        target_version.prepared_manifest.provider_model = ProviderModelReference::new(
            target_version
                .prepared_manifest
                .provider_model
                .model_target_id()
                .clone(),
            target_version
                .prepared_manifest
                .provider_model
                .provider_id()
                .clone(),
            target_version
                .prepared_manifest
                .provider_model
                .provider_model_id()
                .clone(),
            TargetConfigurationVersion::try_new(2).unwrap(),
            target_version
                .prepared_manifest
                .provider_model
                .capabilities()
                .clone(),
        );
        assert!(
            assembler
                .verify_reconstruction(&target_version)
                .await
                .is_err()
        );
        let mut toolset = prepared.clone();
        toolset.prepared_manifest.toolset_fingerprint = Sha256Digest::hash_bytes(b"other");
        assert!(assembler.verify_reconstruction(&toolset).await.is_err());
        let mut instructions = prepared.clone();
        instructions.prepared_manifest.system_prompt_fingerprint =
            Sha256Digest::hash_bytes(b"other");
        assert!(
            assembler
                .verify_reconstruction(&instructions)
                .await
                .is_err()
        );
        let mut estimator = prepared.clone();
        estimator.prepared_manifest.token_estimator_id = "other_v1@1".to_owned();
        assert!(assembler.verify_reconstruction(&estimator).await.is_err());

        let mut prior_snapshot = base_snapshot();
        add_prior_completed_turn(&mut prior_snapshot);
        let prior_store = Arc::new(MutableSourceStore(Mutex::new(prior_snapshot.clone())));
        let (_, estimator) = target(100_000, 100, false);
        let prior_assembler = test_assembler(prior_store.clone(), estimator, 1_000);
        let prior_prepared = prior_assembler
            .assemble(
                prior_snapshot.active_work.work_id,
                &selection,
                &ContextAssemblyVersions::v0(),
            )
            .await
            .unwrap();
        prior_snapshot.prior_messages.clear();
        prior_store.set(prior_snapshot);
        assert_eq!(
            prior_assembler
                .verify_reconstruction(&prior_prepared)
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::ReconstructionDrift
        );
    }

    struct BytesArtifactStore(Vec<u8>);

    impl ArtifactStore for BytesArtifactStore {
        fn begin_capture(
            &self,
            _: BeginArtifactCapture,
        ) -> Result<Box<dyn ArtifactCapture>, ArtifactStoreError> {
            Err(ArtifactStoreError::new(
                ArtifactStoreErrorKind::InvalidRequest,
            ))
        }

        fn verify(&self, artifact: &ArtifactObjectReference) -> Result<(), ArtifactStoreError> {
            if artifact.sha256() == Sha256Digest::hash_bytes(&self.0) {
                Ok(())
            } else {
                Err(ArtifactStoreError::new(ArtifactStoreErrorKind::Integrity))
            }
        }

        fn read_verified(
            &self,
            artifact: &ArtifactObjectReference,
        ) -> Result<Vec<u8>, ArtifactStoreError> {
            self.verify(artifact)?;
            Ok(self.0.clone())
        }

        fn scan_orphans(
            &self,
            _: &BTreeSet<ArtifactStorageKey>,
            _: UtcTimestamp,
        ) -> Result<ArtifactOrphanReport, ArtifactStoreError> {
            Ok(ArtifactOrphanReport {
                referenced_final_count: 0,
                orphans: Vec::new(),
            })
        }
    }

    fn opaque_continuation_snapshot(
        opaque: &[u8],
        selection: &ModelSelectionResult,
    ) -> ContextEligibilitySnapshot {
        let digest = Sha256Digest::hash_bytes(opaque);
        let artifact_id = ArtifactId::generate();
        let mut snapshot = base_snapshot();
        let mut source = model_source(
            &snapshot,
            snapshot.active_work.work_id,
            1,
            vec![
                NormalizedModelOutputItem::Text {
                    text: "semantic predecessor".to_owned(),
                },
                NormalizedModelOutputItem::ProviderOpaque {
                    provider_id: ProviderId::try_new("fixture").unwrap(),
                    item_type: "continuation_v1".to_owned(),
                    sha256: digest,
                    artifact_id,
                },
            ],
            false,
        );
        source.provider_model = selection.selected_target().reference().clone();
        source
            .provider_opaque_artifacts
            .push(ContextArtifactDescriptor {
                artifact_id,
                storage_key: ArtifactStorageKey::from_digest(digest),
                sha256: digest,
                captured_byte_count: CanonicalByteCount::try_new(opaque.len() as u64).unwrap(),
            });
        add_completed_model_boundary(&mut snapshot, &source);
        snapshot.completed_model_outputs.push(source);
        snapshot
    }

    async fn opaque_continuation_result(
        snapshot: ContextEligibilitySnapshot,
        selection: &ModelSelectionResult,
        estimator: TokenEstimatorIdentity,
        opaque: Vec<u8>,
    ) -> ContextAssemblyResult {
        let work_id = snapshot.active_work.work_id;
        ContextAssembler::new(
            Arc::new(MutableSourceStore(Mutex::new(snapshot))),
            Some(Arc::new(BytesArtifactStore(opaque))),
            Arc::new(FixedEstimator {
                identity: estimator,
                tokens: 1_000,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        )
        .assemble(work_id, selection, &ContextAssemblyVersions::v0())
        .await
        .unwrap()
    }

    fn has_opaque_continuation(result: &ContextAssemblyResult) -> bool {
        result
            .request()
            .ordered_input_items()
            .iter()
            .any(|item| matches!(item, ModelInputItem::ProviderOpaqueContinuation(_)))
    }

    fn push_model_barrier(snapshot: &mut ContextEligibilitySnapshot, state: ModelInvocationState) {
        let source = snapshot.completed_model_outputs[0].clone();
        snapshot
            .continuation_boundaries
            .push(ContextContinuationBoundary::Model {
                model_invocation_id: crate::domain::ModelInvocationId::generate(),
                logical_invocation_id: LogicalInvocationId::generate(),
                work_id: source.work_id,
                work_ordinal: source.work_ordinal,
                agent_step_no: AgentStepNo::try_new(source.agent_step_no.get() + 1).unwrap(),
                attempt_no: 1,
                state,
                journal_offset: crate::domain::JournalOffset::try_new(
                    source.journal_offset.get() + 1,
                )
                .unwrap(),
            });
    }

    #[tokio::test]
    async fn opaque_continuation_eligibility_and_barrier_matrix_is_exact() {
        let opaque = br#"{"role":"user","content":"must remain opaque"}"#.to_vec();
        let (selection, estimator) = target(100_000, 100, true);

        let exact = opaque_continuation_result(
            opaque_continuation_snapshot(&opaque, &selection),
            &selection,
            estimator.clone(),
            opaque.clone(),
        )
        .await;
        assert!(has_opaque_continuation(&exact));
        let rendered = exact
            .request()
            .ordered_input_items()
            .iter()
            .find_map(|item| match item {
                ModelInputItem::ProviderOpaqueContinuation(value) => Some(value.opaque()),
                _ => None,
            })
            .unwrap();
        assert_eq!(rendered.as_bytes(), opaque.as_slice());

        let mut provider_mismatch = opaque_continuation_snapshot(&opaque, &selection);
        let capabilities = provider_mismatch.completed_model_outputs[0]
            .provider_model
            .capabilities()
            .clone();
        provider_mismatch.completed_model_outputs[0].provider_model = ProviderModelReference::new(
            ModelTargetId::try_new("fixture").unwrap(),
            ProviderId::try_new("other").unwrap(),
            ProviderModelId::try_new("fixture-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities.clone(),
        );
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                provider_mismatch,
                &selection,
                estimator.clone(),
                opaque.clone(),
            )
            .await
        ));

        let mut model_mismatch = opaque_continuation_snapshot(&opaque, &selection);
        model_mismatch.completed_model_outputs[0].provider_model = ProviderModelReference::new(
            ModelTargetId::try_new("fixture").unwrap(),
            ProviderId::try_new("fixture").unwrap(),
            ProviderModelId::try_new("other-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            capabilities.clone(),
        );
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                model_mismatch,
                &selection,
                estimator.clone(),
                opaque.clone(),
            )
            .await
        ));

        let mut config_mismatch = opaque_continuation_snapshot(&opaque, &selection);
        config_mismatch.completed_model_outputs[0].provider_model = ProviderModelReference::new(
            ModelTargetId::try_new("fixture").unwrap(),
            ProviderId::try_new("fixture").unwrap(),
            ProviderModelId::try_new("fixture-model").unwrap(),
            TargetConfigurationVersion::try_new(2).unwrap(),
            capabilities,
        );
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                config_mismatch,
                &selection,
                estimator.clone(),
                opaque.clone(),
            )
            .await
        ));

        let (capability_disabled, capability_estimator) =
            target_options(100_000, 100, false, false);
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                opaque_continuation_snapshot(&opaque, &selection),
                &capability_disabled,
                capability_estimator,
                opaque.clone(),
            )
            .await
        ));
        let (native_disabled, native_estimator) = target_options(100_000, 100, true, false);
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                opaque_continuation_snapshot(&opaque, &selection),
                &native_disabled,
                native_estimator,
                opaque.clone(),
            )
            .await
        ));

        let mut not_predecessor = opaque_continuation_snapshot(&opaque, &selection);
        let later = model_source(
            &not_predecessor,
            not_predecessor.active_work.work_id,
            1,
            vec![NormalizedModelOutputItem::Text {
                text: "later completed invocation".to_owned(),
            }],
            false,
        );
        add_completed_model_boundary(&mut not_predecessor, &later);
        not_predecessor.completed_model_outputs.push(later);
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                not_predecessor,
                &selection,
                estimator.clone(),
                opaque.clone(),
            )
            .await
        ));

        for barrier in [
            ModelInvocationState::CancelledLocally,
            ModelInvocationState::ProviderOutcomeUnknown,
        ] {
            let mut snapshot = opaque_continuation_snapshot(&opaque, &selection);
            push_model_barrier(&mut snapshot, barrier);
            assert!(!has_opaque_continuation(
                &opaque_continuation_result(
                    snapshot,
                    &selection,
                    estimator.clone(),
                    opaque.clone(),
                )
                .await
            ));
        }

        let mut unknown_tool = opaque_continuation_snapshot(&opaque, &selection);
        let source = unknown_tool.completed_model_outputs[0].clone();
        unknown_tool
            .continuation_boundaries
            .push(ContextContinuationBoundary::Tool {
                tool_execution_id: ToolExecutionId::generate(),
                source_model_invocation_id: source.model_invocation_id,
                work_id: source.work_id,
                work_ordinal: source.work_ordinal,
                agent_step_no: source.agent_step_no,
                tool_ordinal: ToolOrdinal::try_new(1).unwrap(),
                state: ToolExecutionState::OutcomeUnknown,
                journal_offset: crate::domain::JournalOffset::try_new(
                    source.journal_offset.get() + 1,
                )
                .unwrap(),
            });
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(
                unknown_tool,
                &selection,
                estimator.clone(),
                opaque.clone(),
            )
            .await
        ));

        let mut opaque_only = opaque_continuation_snapshot(&opaque, &selection);
        opaque_only.completed_model_outputs[0]
            .normalized_output
            .items
            .remove(0);
        assert!(!has_opaque_continuation(
            &opaque_continuation_result(opaque_only, &selection, estimator, opaque).await
        ));
    }

    #[tokio::test]
    async fn compatible_opaque_continuation_is_verified_and_incompatible_is_excluded() {
        let opaque = b"opaque-continuation".to_vec();
        let digest = Sha256Digest::hash_bytes(&opaque);
        let artifact_id = ArtifactId::generate();
        let mut snapshot = base_snapshot();
        let (selection, estimator) = target(100_000, 100, true);
        let mut source = model_source(
            &snapshot,
            snapshot.active_work.work_id,
            1,
            vec![
                NormalizedModelOutputItem::Text {
                    text: "semantic history".to_owned(),
                },
                NormalizedModelOutputItem::ProviderOpaque {
                    provider_id: ProviderId::try_new("fixture").unwrap(),
                    item_type: "continuation_v1".to_owned(),
                    sha256: digest,
                    artifact_id,
                },
            ],
            false,
        );
        source.provider_model = selection.selected_target().reference().clone();
        source
            .provider_opaque_artifacts
            .push(ContextArtifactDescriptor {
                artifact_id,
                storage_key: ArtifactStorageKey::from_digest(digest),
                sha256: digest,
                captured_byte_count: CanonicalByteCount::try_new(opaque.len() as u64).unwrap(),
            });
        add_completed_model_boundary(&mut snapshot, &source);
        snapshot.completed_model_outputs.push(source);
        let work_id = snapshot.active_work.work_id;
        let store = Arc::new(MutableSourceStore(Mutex::new(snapshot.clone())));
        let assembler = ContextAssembler::new(
            store.clone(),
            Some(Arc::new(BytesArtifactStore(opaque))),
            Arc::new(FixedEstimator {
                identity: estimator,
                tokens: 1_000,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        );
        let result = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert!(
            result
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::ProviderOpaqueContinuation(_)))
        );

        let missing_artifact = ContextAssembler::new(
            store.clone(),
            None,
            Arc::new(FixedEstimator {
                identity: selection.selected_target().estimator().clone(),
                tokens: 1_000,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        );
        assert_eq!(
            missing_artifact
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::MissingArtifact
        );
        let corrupt_artifact = ContextAssembler::new(
            store.clone(),
            Some(Arc::new(BytesArtifactStore(b"corrupt".to_vec()))),
            Arc::new(FixedEstimator {
                identity: selection.selected_target().estimator().clone(),
                tokens: 1_000,
                returned_identity: None,
                fail: false,
            }),
            registry(),
            VersionedInstructionSnapshot::v0(),
            clock(),
        );
        assert_eq!(
            corrupt_artifact
                .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
                .await
                .unwrap_err()
                .kind(),
            ContextAssemblyErrorKind::CorruptArtifact
        );

        snapshot.completed_model_outputs[0].provider_model = ProviderModelReference::new(
            ModelTargetId::try_new("fixture").unwrap(),
            ProviderId::try_new("other").unwrap(),
            ProviderModelId::try_new("other-model").unwrap(),
            TargetConfigurationVersion::try_new(1).unwrap(),
            selection
                .selected_target()
                .reference()
                .capabilities()
                .clone(),
        );
        store.set(snapshot);
        let incompatible = assembler
            .assemble(work_id, &selection, &ContextAssemblyVersions::v0())
            .await
            .unwrap();
        assert!(
            !incompatible
                .request()
                .ordered_input_items()
                .iter()
                .any(|item| matches!(item, ModelInputItem::ProviderOpaqueContinuation(_)))
        );
    }

    #[test]
    fn compile_time_interfaces_do_not_expose_mutation_or_provider_invocation() {
        fn assert_store<T: ContextSourceStore + ?Sized>() {}
        fn assert_clock<T: Clock + ?Sized>() {}
        fn assert_estimator<T: TokenEstimator + ?Sized>() {}
        assert_store::<dyn ContextSourceStore>();
        assert_clock::<dyn Clock>();
        assert_estimator::<dyn TokenEstimator>();
        assert_eq!(
            MonotonicInstant::from_elapsed(Duration::ZERO).elapsed(),
            Duration::ZERO
        );
    }
}
