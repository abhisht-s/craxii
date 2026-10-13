//! craxii.cjson/1 and domain-separated SHA-256; legacy digests remain independent.
use super::{
    ContractResult, invalid,
    raw_json::{LARGE_LIMITS, Limits},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug)]
pub enum DigestDomain {
    Work,
    Cancel,
    Result,
    ModelInvocation,
    AuthenticatedRequest,
    ExecutionProfile,
    ComputeGrant,
    NormalizedIngress,
    NeutralEvidence,
}
impl DigestDomain {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Cancel => "cancel",
            Self::Result => "result",
            Self::ModelInvocation => "model-invocation",
            Self::AuthenticatedRequest => "authenticated-request",
            Self::ExecutionProfile => "execution-profile",
            Self::ComputeGrant => "compute-grant",
            Self::NormalizedIngress => "normalized-ingress",
            Self::NeutralEvidence => "neutral-evidence",
        }
    }
    pub fn parse(s: &str) -> ContractResult<Self> {
        match s {
            "work" => Ok(Self::Work),
            "cancel" => Ok(Self::Cancel),
            "result" => Ok(Self::Result),
            "model-invocation" => Ok(Self::ModelInvocation),
            "authenticated-request" => Ok(Self::AuthenticatedRequest),
            "execution-profile" => Ok(Self::ExecutionProfile),
            "compute-grant" => Ok(Self::ComputeGrant),
            "normalized-ingress" => Ok(Self::NormalizedIngress),
            "neutral-evidence" => Ok(Self::NeutralEvidence),
            _ => Err(invalid()),
        }
    }
}
fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 32 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
pub fn canonical_with_limits(value: &Value, limits: Limits) -> ContractResult<Vec<u8>> {
    fn write(
        v: &Value,
        out: &mut String,
        depth: usize,
        nodes: &mut usize,
        limits: Limits,
    ) -> ContractResult<()> {
        *nodes += 1;
        if depth > limits.depth || *nodes > limits.nodes {
            return Err(invalid());
        }
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::String(s) => {
                if s.len() > limits.bytes {
                    return Err(invalid());
                }
                quote(s, out)
            }
            Value::Number(n) => {
                let n = n
                    .as_u64()
                    .filter(|n| *n <= 2_147_483_647)
                    .ok_or_else(invalid)?;
                out.push_str(&n.to_string());
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(x, out, depth + 1, nodes, limits)?;
                }
                out.push(']');
            }
            Value::Object(o) => {
                let mut keys: Vec<_> = o.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if k.is_empty()
                        || !k.bytes().all(|c| (32..=126).contains(&c))
                        || matches!(k.as_str(), "__proto__" | "constructor" | "prototype")
                    {
                        return Err(invalid());
                    }
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    write(&o[k], out, depth + 1, nodes, limits)?;
                }
                out.push('}');
            }
        }
        if out.len() > limits.bytes {
            return Err(invalid());
        }
        Ok(())
    }
    let mut out = String::new();
    write(value, &mut out, 0, &mut 0, limits)?;
    Ok(out.into_bytes())
}
pub fn canonical_bytes(value: &Value) -> ContractResult<Vec<u8>> {
    canonical_with_limits(value, LARGE_LIMITS)
}
pub fn sha256(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn digest_material(domain: DigestDomain, value: &Value) -> ContractResult<String> {
    let mut bytes = format!("CRAXII-DIGEST\0{}\0{}\0", 1, domain.as_str()).into_bytes();
    bytes.extend(canonical_bytes(value)?);
    Ok(sha256(&bytes))
}
pub fn worst_case_cost(categories: &[Value]) -> ContractResult<String> {
    use super::primitives::decimal;
    if categories.is_empty() || categories.len() > 32 {
        return Err(invalid());
    }
    let mut total = 0u128;
    for c in categories {
        let q = decimal(c["quantity"].as_str().ok_or_else(invalid)?, "D0")? as u128;
        let n = decimal(c["numerator"].as_str().ok_or_else(invalid)?, "D0")? as u128;
        let d = decimal(c["denominator"].as_str().ok_or_else(invalid)?, "Dp")? as u128;
        let p = q
            .checked_mul(n)
            .and_then(|v| v.checked_add(d - 1))
            .ok_or_else(invalid)?;
        total = total.checked_add(p / d).ok_or_else(invalid)?;
        if total > i64::MAX as u128 {
            return Err(invalid());
        }
    }
    Ok(total.to_string())
}
