//! Translation of native model semantics into the unchanged worker 1.0 model contract.
use super::{ProxyBinding, invalid};
use crate::domain::model::ModelUsage;
use crate::domain::*;
use crate::ports::model_provider::*;
use serde_json::{Value, json};

fn parts(p: &[ModelTextPart]) -> Vec<&str> {
    p.iter().map(ModelTextPart::as_str).collect()
}
pub(super) fn request(r: &ModelRequest) -> Result<Value, ProviderError> {
    let mut items = Vec::new();
    for item in r.ordered_input_items() {
        items.push(match item {
  ModelInputItem::Message{role,content_parts}=>json!({"kind":"message","role":role.as_str(),"content_parts":parts(content_parts)}),
  ModelInputItem::PriorAssistant{content_parts}=>json!({"kind":"prior_assistant","content_parts":parts(content_parts)}),
  ModelInputItem::HistoricalRefusal{content_parts}=>json!({"kind":"historical_refusal","content_parts":parts(content_parts)}),
  ModelInputItem::HistoricalReasoningSummary{content_parts}=>json!({"kind":"historical_reasoning_summary","content_parts":parts(content_parts)}),
  ModelInputItem::ToolCall(c)=>json!({"kind":"tool_call","call_id":c.call_id().as_str(),"name":c.name().as_str(),"raw_arguments":c.raw_arguments()}),
  ModelInputItem::ToolResult{call_id,result}=>json!({"kind":"tool_result","call_id":call_id.as_str(),"result_json":serde_json::to_string(result).map_err(|_|invalid())?}),
  ModelInputItem::StructuredData{data}=>json!({"kind":"structured_data","data_json":serde_json::to_string(data).map_err(|_|invalid())?}),
  ModelInputItem::SyntheticRuntimeStatus{status,details}=>json!({"kind":"synthetic_runtime_status","status":status.as_str(),"details_json":serde_json::to_string(details).map_err(|_|invalid())?}),
  ModelInputItem::ProviderOpaqueContinuation(p)=>json!({"kind":"provider_opaque_continuation","provider_namespace":p.provider_id().as_str(),"type_label":p.type_label(),"opaque":p.opaque(),"opaque_sha256":p.sha256().to_string()}),
 });
    }
    let mut tools = Vec::new();
    for t in r.tool_definitions() {
        let native: Value = serde_json::from_slice(&t.canonical_bytes()).map_err(|_| invalid())?;
        // Native implementation versions are descriptive strings; the transport catalog
        // revision is one, with the full original description retained inside schema_json.
        let mut schema = t.input_schema().clone();
        if let Some(o) = schema.as_object_mut() {
            o.insert("description".into(), json!(t.description().as_str()));
        } else {
            return Err(invalid());
        }
        if native["schema_version"] != 1 {
            return Err(invalid());
        }
        tools.push(json!({"name":t.name().as_str(),"version":"1","schema_version":1,"schema_json":serde_json::to_string(&schema).map_err(|_|invalid())?}));
    }
    Ok(
        json!({"instructions":parts(r.instructions()),"ordered_input_items":items,"tool_definitions":tools,
  "requested_output_tokens":r.requested_output_limit().get().to_string(),"tool_choice":match r.tool_choice_policy(){ModelToolChoicePolicy::None=>"none",ModelToolChoicePolicy::Automatic=>"automatic"},
  "parallel_tool_calls":false,"reasoning_continuation":r.provider_native_options().reasoning_continuation()}),
    )
}
pub(super) fn invocation(
    i: &ModelProviderInvocation,
    b: &ProxyBinding,
    authority: Value,
    retry: Option<(String, String)>,
) -> Result<Value, ProviderError> {
    let a = i.control.durable_attempt().ok_or_else(invalid)?;
    if a.work_id().to_string() != b.local_work_id
        || a.model_invocation_id().to_string().is_empty()
        || a.logical_invocation_id() != i.request.logical_invocation_id()
        || a.context_manifest_id() != i.request.context_manifest_id()
        || a.attempt_no().get() != i64::from(i.attempt.get())
        || a.retry_of().is_some() != retry.is_some()
    {
        return Err(invalid());
    }
    let p = &b.profile["body"]["profile"];
    let mut v = json!({"invocation_version":1,"environment":"development","customer_id":authority["customer_id"],"logical_agent_id":authority["logical_agent_id"],
  "work_id":p["work_id"],"period_id":p["period_id"],"model_config_id":p["model_config_id"],"price_config_id":p["price_config_id"],
  "receipt_id":b.receipt["receipt_id"],"local_work_id":b.local_work_id,"logical_invocation_id":a.logical_invocation_id().to_string(),
  "attempt_id":a.model_invocation_id().to_string(),"context_manifest_id":a.context_manifest_id().to_string(),"material_digest":p["material_digest"],
  "attempt_ordinal":i.attempt.get().to_string(),"model_config_digest":p["model_config_digest"],"price_config_digest":p["price_config_digest"],"request":request(&i.request)?});
    if let Some((prior, evidence)) = retry {
        if a.retry_of().map(|x| x.to_string()) != Some(prior.clone()) {
            return Err(invalid());
        }
        v["retry_of_attempt_id"] = json!(prior);
        v["retry_authorization_id"] = json!(evidence);
    }
    let hash = crate::distribution_contracts::canonical::digest_material(
        crate::distribution_contracts::canonical::DigestDomain::ModelInvocation,
        &v,
    )
    .map_err(|_| invalid())?;
    let q = json!({"protocol":"craxii.worker","version":{"major":1,"minor":0},"type":"model_invoke","request_id":crate::distribution::request_id().map_err(|_|invalid())?,
  "environment":"development","authority":authority,"body":{"invocation":v,"invocation_digest":hash,"compute_grant_id":p["compute_grant_id"],"authorization_epoch":b.epoch,"profile_id":p["profile_id"]}});
    crate::distribution_contracts::wire::decode_message(
        &crate::distribution_contracts::canonical::canonical_bytes(&q).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    Ok(q)
}
pub(super) fn text(v: &Value, k: &str) -> Result<String, ProviderError> {
    v[k].as_str().map(str::to_owned).ok_or_else(invalid)
}
pub(super) fn item(v: &Value) -> Result<ModelOutputItem, ProviderError> {
    let p = || -> Result<Vec<ModelTextPart>, ProviderError> {
        v["content_parts"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|x| ModelTextPart::try_new(x.as_str().ok_or_else(invalid)?).map_err(|_| invalid()))
            .collect()
    };
    Ok(match v["kind"].as_str() {
        Some("text") => ModelOutputItem::text(p()?).map_err(|_| invalid())?,
        Some("refusal") => ModelOutputItem::refusal(p()?).map_err(|_| invalid())?,
        Some("reasoning_summary") => {
            ModelOutputItem::reasoning_summary(p()?).map_err(|_| invalid())?
        }
        Some("tool_call") => ModelOutputItem::ToolCall(call(v)?),
        Some("structured_data") => {
            ModelOutputItem::structured_data(json_text(v, "data_json")?).map_err(|_| invalid())?
        }
        Some("provider_opaque" | "unknown_provider_item") => {
            let p = opaque(v)?;
            if v["kind"] == "provider_opaque" {
                ModelOutputItem::ProviderOpaque(p)
            } else {
                ModelOutputItem::UnknownProviderItem(p)
            }
        }
        _ => return Err(invalid()),
    })
}
pub(super) fn call(v: &Value) -> Result<CanonicalModelToolCall, ProviderError> {
    CanonicalModelToolCall::try_new(
        ModelToolCallId::try_new(text(v, "call_id")?).map_err(|_| invalid())?,
        text(v, "name")?,
        text(v, "raw_arguments")?,
    )
    .map_err(|_| invalid())
}
pub(super) fn opaque(v: &Value) -> Result<ProviderOpaqueEvidence, ProviderError> {
    let evidence = ProviderOpaqueEvidence::try_new(
        ProviderId::try_new(text(v, "provider_namespace")?).map_err(|_| invalid())?,
        text(v, "type_label")?,
        text(v, "opaque")?,
    )
    .map_err(|_| invalid())?;
    if evidence.sha256().to_string() != text(v, "opaque_sha256")? {
        return Err(invalid());
    }
    Ok(evidence)
}
pub(super) fn json_text(v: &Value, k: &str) -> Result<Value, ProviderError> {
    crate::distribution_contracts::raw_json::parse_bounded(
        text(v, k)?.as_bytes(),
        crate::distribution_contracts::raw_json::Limits {
            bytes: 65536,
            nodes: 4096,
            depth: 32,
        },
    )
    .map_err(|_| invalid())
}
pub(super) fn usage(v: &Value) -> Result<Option<ModelUsage>, ProviderError> {
    if matches!(v["usage_status"].as_str(), Some("unknown" | "unavailable")) {
        return Ok(None);
    }
    if v["usage_status"] != "reported" {
        return Err(invalid());
    }
    let mut values = std::collections::BTreeMap::new();
    for c in v["categories"].as_array().ok_or_else(invalid)? {
        let k = text(c, "category")?;
        let q = text(c, "quantity")?.parse::<u64>().map_err(|_| invalid())?;
        if values.insert(k, q).is_some() {
            return Err(invalid());
        }
    }
    if values.len() != 6 {
        return Err(invalid());
    }
    let get = |k: &str| values.get(k).copied().ok_or_else(invalid);
    get("request")?;
    get("other")?;
    let input = get("input")?
        .checked_add(get("cached_input")?)
        .ok_or_else(invalid)?;
    let output = get("output")?
        .checked_add(get("reasoning")?)
        .ok_or_else(invalid)?;
    Ok(Some(
        ModelUsage::try_new(
            input,
            get("cached_input")?,
            output,
            get("reasoning")?,
            input.checked_add(output).ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?,
    ))
}
