//! Immutable startup model-target snapshot and deterministic no-fallback selection.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::Instant;

use crate::application::tool_registry::ToolRegistry;
use crate::bootstrap::config::{ModelProvider, ModelsConfig};
use crate::domain::model::RequiredModelCapabilities;
use crate::domain::{
    ModelCapabilitySnapshot, ModelCapabilitySnapshotInput, ModelConfigReference, ModelTarget,
    ModelTargetId, ModelTargetInput, ModelToolDefinition, NormalizedError, ProviderId,
    ProviderModelId, ProviderModelReference, ProviderNativeOptions, TargetConfigurationVersion,
    TokenCount, TokenEstimatorIdentity,
};

/// Immutable startup target catalog in ascending canonical target-ID order.
#[derive(Debug)]
pub struct ModelTargetSnapshot {
    default_target: ModelTargetId,
    targets: Box<[ModelTarget]>,
}

impl ModelTargetSnapshot {
    pub fn try_new(
        default_target: ModelTargetId,
        mut targets: Vec<ModelTarget>,
    ) -> Result<Self, ModelTargetSnapshotError> {
        targets.sort_by(|left, right| {
            left.reference()
                .model_target_id()
                .cmp(right.reference().model_target_id())
        });
        if targets.windows(2).any(|pair| {
            pair[0].reference().model_target_id() == pair[1].reference().model_target_id()
        }) {
            return Err(ModelTargetSnapshotError::DuplicateTarget);
        }
        Ok(Self {
            default_target,
            targets: targets.into_boxed_slice(),
        })
    }

    pub fn from_validated_config(config: &ModelsConfig) -> Result<Self, ModelTargetSnapshotError> {
        let default_target = ModelTargetId::try_new(config.default_target())
            .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?;
        let mut targets = Vec::with_capacity(config.targets().len());
        for configured in config.targets() {
            let provider_id = match configured.provider() {
                ModelProvider::OpenAi => ProviderId::try_new("openai"),
            }
            .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?;
            let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
                text_input: configured.capabilities().text_input(),
                text_output: configured.capabilities().text_output(),
                custom_tool_calling: configured.capabilities().custom_tool_calling(),
                streaming: configured.capabilities().streaming(),
                ordered_output_items: configured.capabilities().ordered_output_items(),
                structured_output: configured.capabilities().structured_output(),
                reasoning_continuation: configured.capabilities().reasoning_continuation(),
                context_window_tokens: positive_token(configured.context_window_tokens())?,
                max_output_tokens: positive_token(configured.max_output_tokens())?,
            });
            let reference = ProviderModelReference::new(
                ModelTargetId::try_new(configured.id())
                    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                provider_id,
                ProviderModelId::try_new(configured.provider_model_id())
                    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                TargetConfigurationVersion::try_new(
                    i64::try_from(configured.config_version())
                        .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                )
                .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                capabilities,
            );
            let estimator_version = parse_estimator_version(configured.token_estimator())?;
            targets.push(
                ModelTarget::try_new(ModelTargetInput {
                    reference,
                    enabled: configured.enabled(),
                    endpoint_reference: ModelConfigReference::endpoint(
                        configured.endpoint().as_str(),
                    )
                    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                    account_reference: ModelConfigReference::named(
                        configured.credential().as_str(),
                    )
                    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                    requested_output_tokens: positive_token(configured.requested_output_tokens())?,
                    estimator: TokenEstimatorIdentity::try_new(
                        configured.token_estimator(),
                        estimator_version,
                    )
                    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
                    provider_native_options: ProviderNativeOptions::new(
                        configured.reasoning_continuation_required(),
                    ),
                })
                .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
            );
        }
        Self::try_new(default_target, targets)
    }

    #[must_use]
    pub const fn default_target(&self) -> &ModelTargetId {
        &self.default_target
    }

    #[must_use]
    pub fn targets(&self) -> &[ModelTarget] {
        &self.targets
    }

    #[must_use]
    pub fn target(&self, id: &ModelTargetId) -> Option<&ModelTarget> {
        self.targets
            .binary_search_by(|target| target.reference().model_target_id().cmp(id))
            .ok()
            .map(|index| &self.targets[index])
    }

    #[must_use]
    pub fn ordered_target_ids(&self) -> Vec<ModelTargetId> {
        self.targets
            .iter()
            .map(|target| target.reference().model_target_id().clone())
            .collect()
    }
}

fn positive_token(value: u64) -> Result<TokenCount, ModelTargetSnapshotError> {
    TokenCount::try_new(
        i64::try_from(value).map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?,
    )
    .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)
}

fn parse_estimator_version(value: &str) -> Result<u64, ModelTargetSnapshotError> {
    let (_, suffix) = value
        .rsplit_once("_v")
        .ok_or(ModelTargetSnapshotError::InvalidConfiguredTarget)?;
    let version = suffix
        .parse::<u64>()
        .map_err(|_| ModelTargetSnapshotError::InvalidConfiguredTarget)?;
    if version == 0 || version > i64::MAX as u64 {
        return Err(ModelTargetSnapshotError::InvalidConfiguredTarget);
    }
    Ok(version)
}

/// Safe startup snapshot failure with no raw configuration content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelTargetSnapshotError {
    DuplicateTarget,
    InvalidConfiguredTarget,
}

impl Display for ModelTargetSnapshotError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateTarget => "duplicate model target",
            Self::InvalidConfiguredTarget => "invalid configured model target",
        })
    }
}

impl std::error::Error for ModelTargetSnapshotError {}

/// Stable reason for the one deterministic V0 selection branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSelectionReason {
    Explicit,
    ConfiguredDefault,
}

/// Exact target chosen by the pure policy. No wall-clock value enters this contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelSelectionResult {
    selected_target: ModelTarget,
    reason: ModelSelectionReason,
    considered_target_ids: Vec<ModelTargetId>,
    required_capabilities: RequiredModelCapabilities,
    target_configuration_version: TargetConfigurationVersion,
}

impl ModelSelectionResult {
    #[must_use]
    pub const fn selected_target(&self) -> &ModelTarget {
        &self.selected_target
    }

    #[must_use]
    pub const fn reason(&self) -> ModelSelectionReason {
        self.reason
    }

    #[must_use]
    pub fn considered_target_ids(&self) -> &[ModelTargetId] {
        &self.considered_target_ids
    }

    #[must_use]
    pub const fn required_capabilities(&self) -> RequiredModelCapabilities {
        self.required_capabilities
    }

    #[must_use]
    pub const fn target_configuration_version(&self) -> TargetConfigurationVersion {
        self.target_configuration_version
    }
}

/// Safe deterministic selection failure distinctions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSelectionErrorKind {
    ExplicitTargetMissing,
    ExplicitTargetDisabled,
    ExplicitTargetIncapable,
    DefaultTargetMissing,
    DefaultTargetDisabled,
    DefaultTargetIncapable,
}

/// Normalized model-selection error retaining only a closed reason code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelSelectionError(ModelSelectionErrorKind);

impl ModelSelectionError {
    #[must_use]
    pub const fn kind(self) -> ModelSelectionErrorKind {
        self.0
    }

    #[must_use]
    pub const fn normalized(self) -> NormalizedError {
        NormalizedError::model_selection()
    }
}

impl Display for ModelSelectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("model selection failed")
    }
}

impl std::error::Error for ModelSelectionError {}

/// Pure deterministic V0 selector over one immutable startup snapshot.
#[derive(Clone, Debug)]
pub struct ModelSelectionPolicy {
    snapshot: Arc<ModelTargetSnapshot>,
}

impl ModelSelectionPolicy {
    #[must_use]
    pub const fn new(snapshot: Arc<ModelTargetSnapshot>) -> Self {
        Self { snapshot }
    }

    #[must_use]
    pub const fn snapshot(&self) -> &Arc<ModelTargetSnapshot> {
        &self.snapshot
    }

    pub fn select(
        &self,
        explicit: Option<&ModelTargetId>,
        required: RequiredModelCapabilities,
    ) -> Result<ModelSelectionResult, ModelSelectionError> {
        let span = tracing::info_span!(
            "model_selection",
            explicit = explicit.is_some(),
            requested_target = explicit.map(ModelTargetId::as_str),
            selected_target = tracing::field::Empty,
            provider = tracing::field::Empty,
            model = tracing::field::Empty,
            selection_reason = tracing::field::Empty,
            duration_micros = tracing::field::Empty,
            result_class = tracing::field::Empty,
        );
        let started = Instant::now();
        let result = span.in_scope(|| match explicit {
            Some(explicit_id) => self.select_exact(
                explicit_id,
                required,
                ModelSelectionReason::Explicit,
                ModelSelectionErrorKind::ExplicitTargetMissing,
                ModelSelectionErrorKind::ExplicitTargetDisabled,
                ModelSelectionErrorKind::ExplicitTargetIncapable,
            ),
            None => self.select_exact(
                self.snapshot.default_target(),
                required,
                ModelSelectionReason::ConfiguredDefault,
                ModelSelectionErrorKind::DefaultTargetMissing,
                ModelSelectionErrorKind::DefaultTargetDisabled,
                ModelSelectionErrorKind::DefaultTargetIncapable,
            ),
        });
        match &result {
            Ok(selection) => {
                let selected = selection.selected_target().reference();
                span.record("selected_target", selected.model_target_id().as_str());
                span.record("provider", selected.provider_id().as_str());
                span.record("model", selected.provider_model_id().as_str());
                span.record(
                    "selection_reason",
                    match selection.reason() {
                        ModelSelectionReason::Explicit => "explicit",
                        ModelSelectionReason::ConfiguredDefault => "configured_default",
                    },
                );
                span.record("result_class", "selected");
            }
            Err(error) => {
                span.record("result_class", format!("{:?}", error.kind()).as_str());
            }
        }
        span.record(
            "duration_micros",
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
        result
    }

    fn select_exact(
        &self,
        target_id: &ModelTargetId,
        required: RequiredModelCapabilities,
        reason: ModelSelectionReason,
        missing: ModelSelectionErrorKind,
        disabled: ModelSelectionErrorKind,
        incapable: ModelSelectionErrorKind,
    ) -> Result<ModelSelectionResult, ModelSelectionError> {
        let target = self
            .snapshot
            .target(target_id)
            .ok_or(ModelSelectionError(missing))?;
        if !target.enabled() {
            return Err(ModelSelectionError(disabled));
        }
        if !required.satisfied_by(target.reference().capabilities()) {
            return Err(ModelSelectionError(incapable));
        }
        Ok(ModelSelectionResult {
            selected_target: target.clone(),
            reason,
            considered_target_ids: self.snapshot.ordered_target_ids(),
            required_capabilities: required,
            target_configuration_version: target.reference().target_configuration_version(),
        })
    }
}

/// Stable provider-neutral projection of the immutable Stage 14 tool inventory.
pub fn project_model_tool_definitions(
    registry: &ToolRegistry,
) -> Result<Vec<ModelToolDefinition>, crate::domain::ModelContractError> {
    registry
        .definitions()
        .iter()
        .map(|definition| {
            ModelToolDefinition::try_new(
                definition.name().clone(),
                definition.implementation_version().clone(),
                definition.schema_version(),
                definition.description(),
                definition.input_schema().clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::config;
    use crate::domain::{ErrorCategory, ModelCapabilitySnapshotInput};

    const LOCAL_CONFIG: &str = include_str!("../../tests/fixtures/config/valid/local.toml");

    fn configured_snapshot() -> Arc<ModelTargetSnapshot> {
        let config = config::parse(LOCAL_CONFIG).unwrap();
        Arc::new(ModelTargetSnapshot::from_validated_config(config.models()).unwrap())
    }

    fn required() -> RequiredModelCapabilities {
        RequiredModelCapabilities {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: false,
            reasoning_continuation: false,
            required_output_tokens: TokenCount::try_new(1_024).unwrap(),
        }
    }

    fn custom_target(
        id: &str,
        enabled: bool,
        capabilities: ModelCapabilitySnapshot,
    ) -> ModelTarget {
        ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new(id).unwrap(),
                ProviderId::try_new("fixture").unwrap(),
                ProviderModelId::try_new(format!("{id}-model")).unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
            enabled,
            endpoint_reference: ModelConfigReference::endpoint("https://fixture.invalid/v1")
                .unwrap(),
            account_reference: ModelConfigReference::named("account").unwrap(),
            requested_output_tokens: TokenCount::try_new(100).unwrap(),
            estimator: TokenEstimatorIdentity::try_new("fixture_v1", 1).unwrap(),
            provider_native_options: ProviderNativeOptions::new(false),
        })
        .unwrap()
    }

    fn capabilities() -> ModelCapabilitySnapshot {
        ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            context_window_tokens: TokenCount::try_new(10_000).unwrap(),
            max_output_tokens: TokenCount::try_new(1_000).unwrap(),
        })
    }

    #[test]
    fn valid_config_snapshot_is_sorted_immutable_and_preserves_exact_inventory() {
        let snapshot = configured_snapshot();
        assert_eq!(snapshot.default_target().as_str(), "primary");
        assert_eq!(
            snapshot
                .ordered_target_ids()
                .iter()
                .map(ModelTargetId::as_str)
                .collect::<Vec<_>>(),
            ["primary", "secondary"]
        );
        let primary = snapshot
            .target(&ModelTargetId::try_new("primary").unwrap())
            .unwrap();
        assert!(primary.enabled());
        assert_eq!(primary.reference().provider_id().as_str(), "openai");
        assert_eq!(
            primary.reference().provider_model_id().as_str(),
            "fixture-primary-model"
        );
        assert_eq!(primary.reference().target_configuration_version().get(), 1);
        assert_eq!(
            primary
                .reference()
                .capabilities()
                .context_window_tokens()
                .get(),
            128_000
        );
        assert_eq!(
            primary.reference().capabilities().max_output_tokens().get(),
            16_384
        );
        assert_eq!(primary.requested_output_tokens().get(), 8_192);
        assert_eq!(primary.estimator().id(), "conservative_v1");
        assert_eq!(primary.estimator().version(), 1);
        assert_eq!(primary.account_reference().as_str(), "openai_primary");
        assert!(
            !snapshot
                .target(&ModelTargetId::try_new("secondary").unwrap())
                .unwrap()
                .enabled()
        );
    }

    #[test]
    fn snapshot_sorts_input_and_rejects_duplicates_and_malformed_identity() {
        let first = custom_target("a", true, capabilities());
        let second = custom_target("b", true, capabilities());
        let snapshot =
            ModelTargetSnapshot::try_new(ModelTargetId::try_new("a").unwrap(), vec![second, first])
                .unwrap();
        assert_eq!(
            snapshot
                .ordered_target_ids()
                .iter()
                .map(ModelTargetId::as_str)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let duplicate = custom_target("a", true, capabilities());
        assert_eq!(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("a").unwrap(),
                vec![custom_target("a", true, capabilities()), duplicate]
            )
            .unwrap_err(),
            ModelTargetSnapshotError::DuplicateTarget
        );
        assert!(ModelTargetId::try_new("Bad target").is_err());
    }

    #[test]
    fn configured_default_and_explicit_enabled_target_are_exact() {
        let policy = ModelSelectionPolicy::new(configured_snapshot());
        let default = policy.select(None, required()).unwrap();
        assert_eq!(
            default
                .selected_target()
                .reference()
                .model_target_id()
                .as_str(),
            "primary"
        );
        assert_eq!(default.reason(), ModelSelectionReason::ConfiguredDefault);
        let explicit = policy
            .select(
                Some(&ModelTargetId::try_new("primary").unwrap()),
                required(),
            )
            .unwrap();
        assert_eq!(explicit.reason(), ModelSelectionReason::Explicit);
        assert_eq!(explicit.target_configuration_version().get(), 1);
        assert_eq!(explicit.required_capabilities(), required());
    }

    #[test]
    fn explicit_missing_disabled_and_incapable_never_fall_back() {
        let policy = ModelSelectionPolicy::new(configured_snapshot());
        assert_eq!(
            policy
                .select(
                    Some(&ModelTargetId::try_new("missing").unwrap()),
                    required()
                )
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::ExplicitTargetMissing
        );
        assert_eq!(
            policy
                .select(
                    Some(&ModelTargetId::try_new("secondary").unwrap()),
                    required()
                )
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::ExplicitTargetDisabled
        );
        let incapable_capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_output: false,
            ..capabilities_input()
        });
        let no_fallback_snapshot = Arc::new(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("capable-default").unwrap(),
                vec![
                    custom_target("capable-default", true, capabilities()),
                    custom_target("explicit-weak", true, incapable_capabilities),
                ],
            )
            .unwrap(),
        );
        let no_fallback = ModelSelectionPolicy::new(no_fallback_snapshot);
        assert_eq!(
            no_fallback
                .select(
                    Some(&ModelTargetId::try_new("explicit-weak").unwrap()),
                    required()
                )
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::ExplicitTargetIncapable
        );
    }

    #[test]
    fn missing_disabled_and_incapable_defaults_are_distinct() {
        let missing = Arc::new(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("missing").unwrap(),
                vec![custom_target("a", true, capabilities())],
            )
            .unwrap(),
        );
        assert_eq!(
            ModelSelectionPolicy::new(missing)
                .select(None, required())
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::DefaultTargetMissing
        );
        let disabled = Arc::new(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("a").unwrap(),
                vec![custom_target("a", false, capabilities())],
            )
            .unwrap(),
        );
        assert_eq!(
            ModelSelectionPolicy::new(disabled)
                .select(None, required())
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::DefaultTargetDisabled
        );
        let incapable_caps = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_output: false,
            ..capabilities_input()
        });
        let incapable = Arc::new(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("a").unwrap(),
                vec![custom_target("a", true, incapable_caps)],
            )
            .unwrap(),
        );
        assert_eq!(
            ModelSelectionPolicy::new(incapable)
                .select(None, required())
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::DefaultTargetIncapable
        );
    }

    fn capabilities_input() -> ModelCapabilitySnapshotInput {
        ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            context_window_tokens: TokenCount::try_new(10_000).unwrap(),
            max_output_tokens: TokenCount::try_new(1_000).unwrap(),
        }
    }

    #[test]
    fn every_required_capability_and_output_budget_mismatch_fails() {
        let mut cases = Vec::new();
        for index in 0..7 {
            let mut input = capabilities_input();
            match index {
                0 => input.text_input = false,
                1 => input.text_output = false,
                2 => input.custom_tool_calling = false,
                3 => input.streaming = false,
                4 => input.ordered_output_items = false,
                5 => input.structured_output = false,
                6 => input.reasoning_continuation = false,
                _ => unreachable!(),
            }
            cases.push(ModelCapabilitySnapshot::new(input));
        }
        let all_required = RequiredModelCapabilities {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: true,
            required_output_tokens: TokenCount::try_new(1_000).unwrap(),
        };
        for available in cases {
            let snapshot = Arc::new(
                ModelTargetSnapshot::try_new(
                    ModelTargetId::try_new("a").unwrap(),
                    vec![custom_target("a", true, available)],
                )
                .unwrap(),
            );
            assert_eq!(
                ModelSelectionPolicy::new(snapshot)
                    .select(None, all_required)
                    .unwrap_err()
                    .kind(),
                ModelSelectionErrorKind::DefaultTargetIncapable
            );
        }
        let snapshot = Arc::new(
            ModelTargetSnapshot::try_new(
                ModelTargetId::try_new("a").unwrap(),
                vec![custom_target("a", true, capabilities())],
            )
            .unwrap(),
        );
        let too_large = RequiredModelCapabilities {
            required_output_tokens: TokenCount::try_new(1_001).unwrap(),
            ..all_required
        };
        assert_eq!(
            ModelSelectionPolicy::new(snapshot)
                .select(None, too_large)
                .unwrap_err()
                .kind(),
            ModelSelectionErrorKind::DefaultTargetIncapable
        );
    }

    #[test]
    fn considered_targets_are_always_deterministic_and_selection_error_is_normalized() {
        let policy = ModelSelectionPolicy::new(configured_snapshot());
        let selected = policy.select(None, required()).unwrap();
        assert_eq!(
            selected
                .considered_target_ids()
                .iter()
                .map(ModelTargetId::as_str)
                .collect::<Vec<_>>(),
            ["primary", "secondary"]
        );
        let error = policy
            .select(
                Some(&ModelTargetId::try_new("missing").unwrap()),
                required(),
            )
            .unwrap_err();
        assert_eq!(
            error.normalized().category(),
            ErrorCategory::ModelSelectionError
        );
    }

    #[test]
    fn semantic_target_changes_alter_the_single_global_config_fingerprint() {
        let original = config::parse(LOCAL_CONFIG).unwrap();
        let changed = config::parse(&LOCAL_CONFIG.replacen(
            "fixture-primary-model",
            "fixture-primary-model-v2",
            1,
        ))
        .unwrap();
        assert_ne!(original.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn stage14_tool_projection_preserves_registry_order_and_stable_schema_facts() {
        let registry = ToolRegistry::v0(crate::application::tool_registry::ToolSemanticPolicy {
            read_file_default_bytes: 1_048_576,
            read_file_max_bytes: 8_388_608,
            run_shell_command_max_bytes: 65_536,
            run_shell_default_timeout_ms: 120_000,
            run_shell_max_timeout_ms: 900_000,
        })
        .unwrap();
        let definitions = project_model_tool_definitions(&registry).unwrap();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name().as_str())
                .collect::<Vec<_>>(),
            ["read_file", "run_shell"]
        );
    }
}
