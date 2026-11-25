//! Exact version intersection; release eligibility is a separate explicit policy input.
use serde_json::{Value, json};
pub const CAPABILITIES: &[&str] = &[
    "admission_v1",
    "cancel_v1",
    "result_v1",
    "status_v1",
    "drain_v1",
    "compute_v1",
    "model_v1",
    "signed_request_v1",
];
pub fn negotiate(peer: &Value, policy: &Value) -> Value {
    let has = |field: &str, expected: &Value| {
        peer[field].as_array().is_some_and(|a| a.contains(expected))
    };
    let rejection = |reason: &str| json!({"accepted":false,"reason":reason});
    if !has("supported_versions", &json!({"major":1,"minor":0})) {
        return rejection("unsupported_version");
    }
    if !has("canonicalization_versions", &json!("craxii.cjson/1")) {
        return rejection("unsupported_canonicalization");
    }
    if !has("signature_versions", &json!("craxii.signature/1")) {
        return rejection("unsupported_signature");
    }
    let advertised = peer["capabilities"].as_array();
    let required = policy["requiredCapabilities"].as_array();
    if advertised.is_none_or(|a| {
        a.iter()
            .any(|c| c.as_str().is_none_or(|s| !CAPABILITIES.contains(&s)))
    }) || required.is_none_or(|a| {
        a.iter().any(|c| {
            !has("capabilities", c) || c.as_str().is_none_or(|s| !CAPABILITIES.contains(&s))
        })
    }) {
        return rejection("unsupported_capability");
    }
    if peer["release_id"]
        .as_str()
        .is_none_or(|id| policy["eligibleReleases"][id] != peer["release_sha256"])
    {
        return rejection("revoked_authority");
    }
    json!({"accepted":true,"version":{"major":1,"minor":0}})
}
pub fn validate_selection(selected: &Value, expected: &Value) -> bool {
    selected == expected
}
/// Conservative structural classification; changed semantics still require explicit review.
pub fn classify_schema_change(before: &Value, after: &Value) -> &'static str {
    fn compare(b: &Value, a: &Value, additive: &mut bool) -> bool {
        let Some(old) = b.as_object() else {
            return b == a;
        };
        let Some(next) = a.as_object() else {
            return false;
        };
        for (key, value) in old {
            if key == "properties" {
                let Some(bp) = value.as_object() else {
                    return false;
                };
                let Some(ap) = next.get(key).and_then(Value::as_object) else {
                    return false;
                };
                for (name, shape) in bp {
                    if !compare(shape, ap.get(name).unwrap_or(&Value::Null), additive) {
                        return false;
                    }
                }
                if ap.keys().any(|k| !bp.contains_key(k)) {
                    *additive = true;
                }
            } else if !compare(value, next.get(key).unwrap_or(&Value::Null), additive) {
                return false;
            }
        }
        !next.keys().any(|k| !old.contains_key(k))
    }
    let mut additive = false;
    if before == after {
        return "identical";
    }
    if !compare(before, after, &mut additive) {
        "breaking"
    } else if additive {
        "requires_negotiated_minor"
    } else {
        "identical"
    }
}
