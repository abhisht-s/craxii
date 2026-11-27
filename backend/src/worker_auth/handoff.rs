//! B6.6 (decision 37): the one-time enrollment handoff the VM's root first-boot step leaves next to the identity
//! (`enrollment.json`, the worker's, 0600). It holds first boot's enrollment credential (one use, 5 minutes; the
//! root step fetches a fresh one just before the worker starts) and AWS's identity document and signature, which
//! the enrollment proof carries. The worker deletes it once bound.
use super::{
    AuthError, AuthResult,
    state::{Disposition, IdentityState},
};
use crate::distribution_contracts::primitives::uuid;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    contract_version: u32,
    enrollment_id: String,
    enrollment_credential: String,
    expected_instance_ref: String,
    instance_identity_document_b64: String,
    instance_identity_signature_b64: String,
}

pub struct EnrollmentHandoff {
    pub enrollment_id: String,
    pub enrollment_credential: String,
    pub expected_instance_ref: String,
    pub document: Vec<u8>,
    pub signature: Vec<u8>,
}
impl std::fmt::Debug for EnrollmentHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EnrollmentHandoff([REDACTED])")
    }
}
impl EnrollmentHandoff {
    pub fn parse(bytes: &[u8]) -> AuthResult<Self> {
        let invalid = |_| AuthError::InvalidContext;
        let raw: Raw = serde_json::from_slice(bytes).map_err(|_| AuthError::InvalidContext)?;
        let credential = raw.enrollment_credential.as_bytes();
        if raw.contract_version != 1
            || credential.len() != 43
            || !credential
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_')
        {
            return Err(AuthError::InvalidContext);
        }
        uuid(&raw.enrollment_id, Some(4)).map_err(invalid)?;
        uuid(&raw.expected_instance_ref, Some(4)).map_err(invalid)?;
        let document = STANDARD
            .decode(&raw.instance_identity_document_b64)
            .map_err(|_| AuthError::InvalidContext)?;
        let signature = STANDARD
            .decode(&raw.instance_identity_signature_b64)
            .map_err(|_| AuthError::InvalidContext)?;
        // The same bounds the enrollment proof enforces when it is resumed.
        if document.is_empty()
            || document.len() > 16384
            || signature.is_empty()
            || signature.len() > 8192
        {
            return Err(AuthError::InvalidContext);
        }
        Ok(Self {
            enrollment_id: raw.enrollment_id,
            enrollment_credential: raw.enrollment_credential,
            expected_instance_ref: raw.expected_instance_ref,
            document,
            signature,
        })
    }
}

/// An enrollment left pending by an interruption, given first boot's handoff: true when it is the same enrollment,
/// which resumes by replaying its exact proof (the control plane accepts that even after the credential was used).
/// A different one means first boot issued a new credential, which it does only while nothing is bound (revoking the
/// old one), so the old proof never landed: it is dropped and the worker enrolls afresh.
pub fn settle_pending(state: &mut IdentityState, handoff: &EnrollmentHandoff) -> bool {
    match &state.pending_signed {
        Some(signed) if signed["enrollment_id"] == handoff.enrollment_id.as_str() => true,
        Some(_) => {
            state.pending_signed = None;
            state.disposition = Disposition::Unbound;
            false
        }
        None => false,
    }
}
