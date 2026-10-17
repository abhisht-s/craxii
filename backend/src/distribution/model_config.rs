//! Nonsecret immutable model pins for explicit trusted proxy composition. No endpoint,
//! provider credentials, environment discovery or fallback is represented here.
use super::{DistributionResult, invalid};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProxySettings {
    pub contract_version: u8,
    pub model_id: String,
    pub model_config_id: String,
    pub model_config_digest: String,
    pub price_config_id: String,
    pub price_config_digest: String,
    pub max_output_tokens: u32,
}
impl ModelProxySettings {
    pub fn parse(bytes: &[u8]) -> DistributionResult<Self> {
        let v = crate::distribution_contracts::raw_json::parse_bounded(
            bytes,
            crate::distribution_contracts::raw_json::Limits {
                bytes: 16384,
                nodes: 4096,
                depth: 16,
            },
        )
        .map_err(|_| invalid())?;
        let s: Self = serde_json::from_value(v).map_err(|_| invalid())?;
        s.validate()?;
        Ok(s)
    }
    pub fn validate(&self) -> DistributionResult<()> {
        if self.contract_version != 1
            || self.model_id.is_empty()
            || self.model_id.len() > 96
            || !self
                .model_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || !(1..=32768).contains(&self.max_output_tokens)
        {
            return Err(invalid());
        }
        for id in [&self.model_config_id, &self.price_config_id] {
            crate::distribution_contracts::primitives::uuid(id, Some(4)).map_err(|_| invalid())?;
        }
        for d in [&self.model_config_digest, &self.price_config_digest] {
            if d.len() != 64
                || !d
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn matches_profile(&self, p: &serde_json::Value) -> bool {
        p["model_config_id"] == self.model_config_id
            && p["model_config_digest"] == self.model_config_digest
            && p["price_config_id"] == self.price_config_id
            && p["price_config_digest"] == self.price_config_digest
    }
}

impl ModelProxySettings {
    pub fn target(&self) -> DistributionResult<crate::domain::ModelTarget> {
        use crate::domain::*;
        self.validate()?;
        let make = || -> Result<ModelTarget, super::DistributionError> {
            ModelTarget::try_new(ModelTargetInput {
                reference: ProviderModelReference::new(
                    ModelTargetId::try_new(&self.model_id).map_err(|_| invalid())?,
                    ProviderId::try_new("central_proxy").map_err(|_| invalid())?,
                    ProviderModelId::try_new(&self.model_id).map_err(|_| invalid())?,
                    TargetConfigurationVersion::try_new(1).map_err(|_| invalid())?,
                    ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
                        text_input: true,
                        text_output: true,
                        custom_tool_calling: true,
                        streaming: true,
                        ordered_output_items: true,
                        structured_output: true,
                        reasoning_continuation: false,
                        context_window_tokens: TokenCount::try_new(128000)
                            .map_err(|_| invalid())?,
                        max_output_tokens: TokenCount::try_new(i64::from(self.max_output_tokens))
                            .map_err(|_| invalid())?,
                    }),
                ),
                enabled: true,
                endpoint_reference: ModelConfigReference::named("central_worker_origin")
                    .map_err(|_| invalid())?,
                account_reference: ModelConfigReference::named("central").map_err(|_| invalid())?,
                requested_output_tokens: TokenCount::try_new(i64::from(
                    self.max_output_tokens.min(512),
                ))
                .map_err(|_| invalid())?,
                estimator: TokenEstimatorIdentity::try_new("terminal_fixed", 1)
                    .map_err(|_| invalid())?,
                provider_native_options: ProviderNativeOptions::new(false),
            })
            .map_err(|_| invalid())
        };
        make().map_err(|_| invalid())
    }
}
