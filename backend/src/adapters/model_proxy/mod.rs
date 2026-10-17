//! Central spending transport through the existing protected worker client. No provider
//! secret discovery, paid-provider fallback, context assembly or inference retry lives here.
mod status;
mod stream;
mod wire;
use crate::domain::*;
use crate::ports::{clock::Clock, model_provider::*};
use crate::worker_auth::client::WorkerClient;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

fn invalid() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::InvalidRequest,
        ProviderOutcomeCertainty::DefinitelyNotSent,
    )
}
fn unknown() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::ProviderOutcomeUnknown,
        ProviderOutcomeCertainty::ProviderOutcomeUnknown,
    )
}
/// Trusted current compute gate, rechecked before transmission and every stream read.
pub trait ModelComputeAuthority: Send + Sync {
    fn check(&self, work: &str, grant: &str) -> Result<(), ProviderError>;
}
#[cfg(target_os = "linux")]
impl ModelComputeAuthority for crate::compute::worker::InstalledCompute {
    fn check(&self, work: &str, grant: &str) -> Result<(), ProviderError> {
        self.check_work(work, grant, 1).map_err(|_| invalid())
    }
}
#[derive(Clone)]
pub struct ProxyBinding {
    receipt: Value,
    profile: Value,
    local_work_id: String,
    epoch: String,
    settings: crate::distribution::model_config::ModelProxySettings,
}
impl std::fmt::Debug for ProxyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyBinding([REDACTED])")
    }
}
impl ProxyBinding {
    /// Inputs are the original committed SQLite receipt and centrally verified profile,
    /// supplied by trusted composition. Never use model content as a binding source.
    pub fn target(&self) -> Result<ModelTarget, ProviderError> {
        self.settings.target().map_err(|_| invalid())
    }
    pub fn new(
        receipt: Value,
        profile: Value,
        epoch: String,
        settings: crate::distribution::model_config::ModelProxySettings,
    ) -> Result<Self, ProviderError> {
        crate::distribution_contracts::wire::decode_message(
            &crate::distribution_contracts::canonical::canonical_bytes(&profile)
                .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?;
        settings.validate().map_err(|_| invalid())?;
        let p = &profile["body"]["profile"];
        if !settings.matches_profile(p) {
            return Err(invalid());
        }
        crate::distribution_contracts::wire::validate_definition("work", "Receipt", &receipt)
            .map_err(|_| invalid())?;
        if profile["type"] != "execution_profile"
            || p["kind"] != "development_fixture"
            || p["deterministic_program_id"] != "proxy_loop_v1"
            || p["work_id"] != receipt["work_id"]
            || p["material_digest"] != receipt["material_digest"]
            || p["installation_id"] != receipt["origin_installation_id"]
            || p["generation"] != receipt["origin_generation"]
        {
            return Err(invalid());
        }
        crate::distribution_contracts::primitives::decimal(&epoch, "Dp").map_err(|_| invalid())?;
        let local_work_id = wire::text(&receipt, "local_work_id")?;
        Ok(Self {
            receipt,
            profile,
            local_work_id,
            epoch,
            settings,
        })
    }
}
pub struct CentralModelProvider {
    client: Arc<tokio::sync::Mutex<WorkerClient>>,
    binding: ProxyBinding,
    target: ModelTarget,
    clock: Arc<dyn Clock>,
    compute: Arc<dyn ModelComputeAuthority>,
    requests: tokio::sync::Mutex<std::collections::BTreeMap<String, Value>>,
}
impl CentralModelProvider {
    pub fn new(
        client: Arc<tokio::sync::Mutex<WorkerClient>>,
        binding: ProxyBinding,
        target: ModelTarget,
        clock: Arc<dyn Clock>,
        compute: Arc<dyn ModelComputeAuthority>,
    ) -> Result<Self, ProviderError> {
        if target.reference().provider_id().as_str() != "central_proxy"
            || target.reference().model_target_id().as_str() != binding.settings.model_id
            || target.requested_output_tokens().get()
                > i64::from(binding.settings.max_output_tokens)
        {
            return Err(invalid());
        }
        Ok(Self {
            client,
            binding,
            target,
            clock,
            compute,
            requests: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
        })
    }
}
impl ModelProvider for CentralModelProvider {
    fn provider_id(&self) -> &ProviderId {
        self.target.reference().provider_id()
    }
    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError> {
        if target.identity() != self.target.identity() {
            return Err(invalid());
        }
        Ok(self.target.reference().capabilities().clone())
    }
    fn invoke_stream(
        &self,
        i: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>> {
        Box::pin(async move {
            self.capabilities(i.request.target())?;
            let p = &self.binding.profile["body"]["profile"];
            let work = wire::text(p, "work_id")?;
            let grant = wire::text(p, "compute_grant_id")?;
            self.compute.check(&work, &grant)?;
            if i.control.cancellation().is_cancelled() {
                return Err(invalid());
            }
            let remaining = i
                .control
                .absolute_deadline()
                .checked_duration_since(self.clock.monotonic_now())
                .ok_or_else(invalid)?
                .min(Duration::from_secs(30));
            let deadline = self
                .clock
                .monotonic_now()
                .checked_add(remaining)
                .ok_or_else(invalid)?;
            let mut transmitted_request = None;
            let open = async {
                let mut client = self.client.lock().await;
                let retry =
                    if let Some(prior) = i.control.durable_attempt().and_then(|a| a.retry_of()) {
                        let previous = self
                            .requests
                            .lock()
                            .await
                            .get(&prior.to_string())
                            .cloned()
                            .ok_or_else(unknown)?;
                        let state = client
                            .model_status(&previous)
                            .await
                            .map_err(|_| unknown())?;
                        let status = status::AttemptStatus::parse(&state, &previous)?;
                        Some((
                            prior.to_string(),
                            status.retry_evidence().ok_or_else(unknown)?,
                        ))
                    } else {
                        None
                    };
                let q = wire::invocation(
                    &i,
                    &self.binding,
                    client.current_authority().map_err(|_| invalid())?,
                    retry,
                )?;
                {
                    let mut requests = self.requests.lock().await;
                    if requests.len() >= 3 {
                        return Err(unknown());
                    }
                    requests.insert(
                        wire::text(&q["body"]["invocation"], "attempt_id")?,
                        q.clone(),
                    );
                }
                transmitted_request = Some(q.clone());
                let response = client.open_model(&q).await;
                Ok::<_, ProviderError>((q, response))
            };
            let opened = tokio::select! {biased;()=i.control.cancellation().cancelled()=>return Err(unknown()),r=tokio::time::timeout(remaining.min(Duration::from_secs(5)),open)=>r};
            let (q, response) = match opened {
                Ok(result) => result?,
                Err(_) => (
                    transmitted_request.ok_or_else(unknown)?,
                    Err(crate::worker_auth::AuthError::Transport),
                ),
            };
            let response = match response {
                Ok(response)
                    if response.status().is_success()
                        && response
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            == Some("application/x-ndjson") =>
                {
                    response
                }
                _ => {
                    let budget = deadline
                        .checked_duration_since(self.clock.monotonic_now())
                        .ok_or_else(unknown)?;
                    tokio::select! {biased;()=i.control.cancellation().cancelled()=>return Err(unknown()),r=tokio::time::timeout(budget,status::recover(self.client.clone(), &q))=>r.map_err(|_|unknown())??}
                }
            };
            Ok(Box::new(stream::ProxyStream::new(
                response,
                q,
                self.client.clone(),
                i,
                self.clock.clone(),
                deadline,
                self.compute.clone(),
                work,
                grant,
            )) as Box<dyn ModelProviderStream>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distribution_contracts::canonical::{
        DigestDomain, canonical_bytes, digest_material,
    };
    use serde_json::json;
    pub(super) fn request() -> ModelRequest {
        let caps = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(128000).unwrap(),
            max_output_tokens: TokenCount::try_new(1024).unwrap(),
        });
        let target = ModelTarget::try_new(ModelTargetInput {
            reference: ProviderModelReference::new(
                ModelTargetId::try_new("synthetic-agent").unwrap(),
                ProviderId::try_new("central_proxy").unwrap(),
                ProviderModelId::try_new("synthetic-model").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                caps,
            ),
            enabled: true,
            endpoint_reference: ModelConfigReference::named("central_worker_origin").unwrap(),
            account_reference: ModelConfigReference::named("central").unwrap(),
            requested_output_tokens: TokenCount::try_new(64).unwrap(),
            estimator: TokenEstimatorIdentity::try_new("synthetic", 1).unwrap(),
            provider_native_options: ProviderNativeOptions::new(false),
        })
        .unwrap();
        ModelRequest::try_new(ModelRequestInput {
            logical_invocation_id: LogicalInvocationId::generate(),
            target,
            ordered_input_items: vec![
                ModelInputItem::message(
                    ModelInputRole::User,
                    vec![ModelTextPart::try_new("😀\0exact").unwrap()],
                )
                .unwrap(),
            ],
            instructions: vec![ModelTextPart::try_new("bounded instruction").unwrap()],
            tool_definitions: vec![],
            requested_output_limit: TokenCount::try_new(64).unwrap(),
            tool_choice_policy: ModelToolChoicePolicy::None,
            provider_native_options: ProviderNativeOptions::new(false),
            context_manifest_id: ContextManifestId::generate(),
        })
        .unwrap()
    }
    #[test]
    fn neutral_request_preserves_exact_text_and_has_its_own_digest_domain() {
        let r = request();
        let v = wire::request(&r).unwrap();
        assert_eq!(v["ordered_input_items"][0]["content_parts"][0], "😀\0exact");
        assert_eq!(v["requested_output_tokens"], "64");
        assert_ne!(
            digest_material(DigestDomain::ModelInvocation, &v).unwrap(),
            r.canonical_sha256().to_string()
        );
        assert!(
            canonical_bytes(&v)
                .unwrap()
                .windows(6)
                .any(|b| b == b"\\u0000")
        );
    }
    #[test]
    fn durable_context_is_opt_in_and_retains_gateway_physical_identity() {
        let r = request();
        let a = ModelAttemptReference::new(ModelAttemptReferenceInput {
            logical_invocation_id: r.logical_invocation_id(),
            model_invocation_id: ModelInvocationId::generate(),
            work_id: WorkId::generate(),
            runtime_instance_id: RuntimeInstanceId::generate(),
            context_manifest_id: r.context_manifest_id(),
            agent_step_no: AgentStepNo::try_new(1).unwrap(),
            attempt_no: AttemptNo::try_new(1).unwrap(),
            provider_model: r.target().reference().clone(),
            retry_of: None,
        });
        let c = ModelInvocationControl::try_new(
            ProviderCancellationToken::new(),
            crate::ports::clock::MonotonicInstant::from_elapsed(Duration::from_secs(30)),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(c.durable_attempt().is_none());
        let bound = c.with_durable_attempt(a.clone());
        assert_eq!(bound.durable_attempt(), Some(&a));
        assert_eq!(
            bound
                .clone()
                .durable_attempt()
                .unwrap()
                .model_invocation_id(),
            a.model_invocation_id()
        );
    }
    #[test]
    fn central_usage_maps_disjoint_billing_categories_into_native_inclusive_usage() {
        let v = json!({"usage_status":"reported","categories":[{"category":"input","quantity":"10"},{"category":"cached_input","quantity":"2"},{"category":"output","quantity":"3"},{"category":"reasoning","quantity":"1"},{"category":"request","quantity":"1"},{"category":"other","quantity":"0"}]});
        let u = wire::usage(&v).unwrap().unwrap();
        assert_eq!(u.input_tokens(), 12);
        assert_eq!(u.output_tokens(), 4);
        assert_eq!(u.total_tokens(), 16);
        let mut duplicate = v;
        duplicate["categories"][1]["category"] = json!("input");
        assert!(wire::usage(&duplicate).is_err());
        assert!(
            wire::usage(&json!({"usage_status":"unknown"}))
                .unwrap()
                .is_none()
        );
    }
}
