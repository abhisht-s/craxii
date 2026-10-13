//! Signature preimages only. No key generation, nonce trust, session or enrollment service.
use super::{
    ContractResult,
    canonical::{canonical_bytes, sha256},
    invalid,
    primitives::binary,
};
use serde_json::Value;
#[derive(Clone, Copy)]
pub enum SignaturePurpose {
    Enrollment,
    Session,
    SensitiveRequest,
    ComputeAuthorization,
    ExecutionProfile,
}
impl SignaturePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enrollment => "enrollment",
            Self::Session => "session",
            Self::SensitiveRequest => "sensitive_request",
            Self::ComputeAuthorization => "compute_authorization",
            Self::ExecutionProfile => "execution_profile",
        }
    }
    pub fn parse(s: &str) -> ContractResult<Self> {
        match s {
            "enrollment" => Ok(Self::Enrollment),
            "session" => Ok(Self::Session),
            "sensitive_request" => Ok(Self::SensitiveRequest),
            "compute_authorization" => Ok(Self::ComputeAuthorization),
            "execution_profile" => Ok(Self::ExecutionProfile),
            _ => Err(invalid()),
        }
    }
}
pub fn signature_input(purpose: SignaturePurpose, value: &Value) -> ContractResult<Vec<u8>> {
    let mut bytes = format!("CRAXII-SIGNATURE\0{}\0{}\0", 1, purpose.as_str()).into_bytes();
    bytes.extend(canonical_bytes(value)?);
    Ok(bytes)
}
pub fn key_id(public_key: &str) -> ContractResult<String> {
    Ok(sha256(&binary(public_key, Some(32))?))
}
