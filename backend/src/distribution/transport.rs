//! HTTP adapter version 1. The immutable worker 1.0 messages are decoded individually.
use super::{DistributionResult, WorkerEnvironment, invalid, text};
use serde_json::Value;
/// The poll's items. A development execution profile can never be valid in production (contract 1.0.0 fixes a
/// profile's environment to `development`), so in production such an item is dropped instead of failing the
/// whole poll; the delivery that names it is then refused on its own.
pub fn poll_response(
    value: &Value,
    request: &str,
    environment: WorkerEnvironment,
) -> DistributionResult<Vec<Value>> {
    let o = value.as_object().ok_or_else(invalid)?;
    if o.len() != 4
        || o.keys().any(|k| {
            !matches!(
                k.as_str(),
                "contract_version" | "request_id" | "items" | "retry_after_ms"
            )
        })
        || value["contract_version"] != 1
        || text(value, "request_id")? != request
        || value["retry_after_ms"].as_u64().is_none_or(|n| n > 20000)
    {
        return Err(invalid());
    }
    let items = value["items"].as_array().ok_or_else(invalid)?;
    if items.len() > 4 {
        return Err(invalid());
    }
    let mut kept = Vec::with_capacity(items.len());
    for item in items {
        if environment == WorkerEnvironment::Production && item["type"] == "execution_profile" {
            tracing::warn!(target: "craxii::distribution", event = "profile_dropped");
            continue;
        }
        crate::distribution_contracts::wire::decode_message(
            &crate::distribution_contracts::canonical::canonical_bytes(item)
                .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())?;
        if !matches!(
            item["type"].as_str(),
            Some("work_delivery" | "cancel_delivery" | "execution_profile")
        ) {
            return Err(invalid());
        }
        kept.push(item.clone());
    }
    Ok(kept)
}
