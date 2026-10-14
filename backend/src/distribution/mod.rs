//! Receipt-only outbound distribution. No scheduler, model provider or tool runner is composed.
pub mod startup;
pub mod transport;
use crate::distribution_contracts::{
    canonical::{canonical_bytes, sha256},
    primitives::{binary, timestamp_micros},
    signing::{SignaturePurpose, signature_input},
    wire::decode_message,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DistributionError {
    Invalid,
    Conflict,
    Storage,
    Reconciliation,
    Expired,
    Sqlite,
    Journal,
    Recovery,
    Artifacts,
}
pub type DistributionResult<T> = Result<T, DistributionError>;
pub(crate) fn invalid() -> DistributionError {
    DistributionError::Invalid
}
pub(crate) fn text<'a>(v: &'a Value, k: &str) -> DistributionResult<&'a str> {
    v[k].as_str().ok_or_else(invalid)
}
pub fn request_id() -> DistributionResult<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| invalid())?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    Ok(uuid::Uuid::from_bytes(b).to_string())
}
pub(crate) fn now() -> DistributionResult<i128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DistributionError::Expired)?
        .as_micros() as i128
        + 62_135_596_800_000_000i128)
}
pub(crate) fn timestamp() -> DistributionResult<crate::domain::UtcTimestamp> {
    crate::domain::UtcTimestamp::from_offset_datetime(time::OffsetDateTime::now_utc())
        .map_err(|_| invalid())
}
/// Constructor restricted to the authenticated client; admission rechecks after SQLite waits.
pub struct ValidatedDelivery {
    pub(crate) message: Value,
    pub(crate) profile: Option<Value>,
    pub(crate) profile_key: String,
    pub(crate) deadline: Instant,
    pub(crate) clock_seen: SystemTime,
}
impl ValidatedDelivery {
    pub fn check_fresh(&self) -> DistributionResult<()> {
        let b = &self.message["body"];
        let t = now()?;
        let expires = timestamp_micros(text(b, "expires_at")?).map_err(|_| invalid())?;
        if Instant::now() >= self.deadline || SystemTime::now() < self.clock_seen || t >= expires {
            return Err(DistributionError::Expired);
        }
        Ok(())
    }
    pub(crate) fn verify_profile(&self) -> DistributionResult<()> {
        let delivery = &self.message;
        let body = &delivery["body"];
        let profile = self
            .profile
            .as_ref()
            .ok_or(DistributionError::Reconciliation)?;
        decode_message(&canonical_bytes(profile).map_err(|_| invalid())?).map_err(|_| invalid())?;
        let p = &profile["body"]["profile"];
        let a = &delivery["authority"];
        let m = &body["material"];
        let expected=sha256(&canonical_bytes(&json!({"program":"receipt_only_v1","tools":[],"model_dispatch":false,"tool_dispatch":false})).map_err(|_|invalid())?);
        if profile["authority"] != *a
            || p["kind"] != "development_fixture"
            || p["environment"] != "development"
            || p["audience"] != "craxii.worker:development"
            || p["deterministic_program_id"] != "receipt_only_v1"
            || p["allowed_tool_ids"] != json!([])
            || p["max_tool_duration_ms"] != "1"
            || p["config_digest"] != expected
            || p["model_config_digest"] != expected
            || p["price_config_digest"] != expected
        {
            return Err(invalid());
        }
        for k in [
            "customer_id",
            "logical_agent_id",
            "installation_id",
            "generation",
        ] {
            if p[k] != a[k] {
                return Err(invalid());
            }
        }
        if p["work_id"] != m["work_id"]
            || p["material_digest"] != body["material_digest"]
            || body["execution_authorization"]["kind"] != "development_fixture"
            || body["execution_authorization"]["profile_id"] != p["profile_id"]
            || body["execution_authorization"]["profile_digest"]
                != sha256(&canonical_bytes(p).map_err(|_| invalid())?)
        {
            return Err(invalid());
        }
        let t = now()?;
        if t < timestamp_micros(text(p, "issued_at")?).map_err(|_| invalid())?
            || t >= timestamp_micros(text(p, "expires_at")?).map_err(|_| invalid())?
        {
            return Err(DistributionError::Expired);
        }
        let key = binary(&self.profile_key, Some(32)).map_err(|_| invalid())?;
        if profile["body"]["key_id"] != sha256(&key) {
            return Err(invalid());
        }
        aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ED25519, key)
            .verify(
                &signature_input(SignaturePurpose::ExecutionProfile, p).map_err(|_| invalid())?,
                &binary(text(&profile["body"], "signature")?, Some(64)).map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?;
        Ok(())
    }
    pub(crate) fn from_authenticated(
        message: Value,
        profile: Option<Value>,
        profile_key: String,
        authority: &Value,
        boot: &str,
        power: &str,
    ) -> DistributionResult<Self> {
        decode_message(&canonical_bytes(&message).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
        if !matches!(
            message["type"].as_str(),
            Some("work_delivery" | "cancel_delivery")
        ) || message["authority"] != *authority
            || message["environment"] != "development"
            || message["body"]["boot_id"] != boot
            || message["body"]["power_operation_id"] != power
        {
            return Err(invalid());
        }
        let issued =
            timestamp_micros(text(&message["body"], "issued_at")?).map_err(|_| invalid())?;
        let expiry =
            timestamp_micros(text(&message["body"], "expires_at")?).map_err(|_| invalid())?;
        let t = now()?;
        if expiry <= t || t < issued || expiry - issued > 60_000_000 {
            return Err(DistributionError::Expired);
        }
        let result = Self {
            message,
            profile,
            profile_key,
            deadline: Instant::now()
                + Duration::from_micros(u64::try_from(expiry - t).map_err(|_| invalid())?),
            clock_seen: SystemTime::now(),
        };
        result.check_fresh()?;
        Ok(result)
    }
}
