//! HTTP adapter version 1. The immutable worker 1.0 messages are decoded individually.
use super::{DistributionResult, invalid, text};
use serde_json::Value;
pub fn poll_response<'a>(value: &'a Value, request: &str) -> DistributionResult<&'a Vec<Value>> {
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
    for item in items {
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
    }
    Ok(items)
}
