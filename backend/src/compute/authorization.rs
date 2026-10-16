use super::{ComputeResult, denied};
use crate::distribution_contracts::{
    canonical::{canonical_bytes, sha256},
    primitives::{binary, decimal, timestamp_micros, uuid},
    raw_json::{CONTROL_LIMITS, parse_bounded},
    signing::{SignaturePurpose, signature_input},
    wire::check_semantics,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
/// Protected configuration supplies these identities, never the model or signed file itself.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub customer_id: String,
    pub logical_agent_id: String,
    pub installation_id: String,
    pub generation: String,
    pub authorization_epoch: String,
    pub power_operation_id: String,
    pub period_id: String,
    pub public_key: String,
}
impl Binding {
    pub fn validate(&self) -> ComputeResult<()> {
        for id in [
            &self.customer_id,
            &self.logical_agent_id,
            &self.installation_id,
            &self.power_operation_id,
            &self.period_id,
        ] {
            uuid(id, Some(4)).map_err(|_| denied())?;
        }
        decimal(&self.generation, "Dp").map_err(|_| denied())?;
        decimal(&self.authorization_epoch, "Dp").map_err(|_| denied())?;
        binary(&self.public_key, Some(32)).map_err(|_| denied())?;
        Ok(())
    }
    fn check(&self, v: &Value) -> ComputeResult<()> {
        self.validate()?;
        for (name, expected) in [
            ("customer_id", &self.customer_id),
            ("logical_agent_id", &self.logical_agent_id),
            ("installation_id", &self.installation_id),
            ("generation", &self.generation),
            ("authorization_epoch", &self.authorization_epoch),
            ("power_operation_id", &self.power_operation_id),
            ("period_id", &self.period_id),
        ] {
            if text(v, name)? != expected {
                return Err(denied());
            }
        }
        Ok(())
    }
}
pub(crate) fn text<'a>(v: &'a Value, k: &str) -> ComputeResult<&'a str> {
    v[k].as_str().ok_or_else(denied)
}
pub(crate) fn micros(v: &Value, k: &str) -> ComputeResult<i128> {
    timestamp_micros(text(v, k)?).map_err(|_| denied())
}
/// CLOCK_BOOTTIME includes suspend; UTC uses the frozen contract's year-1 epoch.
#[derive(Clone, Debug)]
pub struct ClockSample {
    pub boot_id: String,
    pub ticks_us: u64,
    pub utc_us: i128,
    pub trusted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deadline {
    pub boot_id: String,
    #[serde(with = "tick_decimal")]
    pub ticks_cutoff_us: u64,
    pub utc_cutoff_us: String,
    #[serde(with = "tick_decimal")]
    pub sampled_ticks_us: u64,
    pub sampled_utc_us: String,
    pub draining: bool,
    // Boot-wide greatest observed UTC - CLOCK_BOOTTIME offset. Never reset on renewal.
    // Absent only in legacy receipts, which are usable as consumed-permit history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_offset_us: Option<String>,
}
impl Deadline {
    fn from_signed(v: &Value, c: &ClockSample, require_trust: bool) -> ComputeResult<Self> {
        let issued = micros(v, "issued_at")?;
        let expires = micros(v, "expires_at")?;
        let elapsed = decimal(text(v, "max_elapsed_ms")?, "Dp").map_err(|_| denied())?;
        if require_trust && !c.trusted
            || elapsed > 14_400_000
            || c.utc_us < issued
            || c.utc_us >= expires
            || expires <= issued
            || elapsed * 1000 > expires - issued
        {
            return Err(denied());
        }
        // Charge elapsed time since central issuance too. Accepting late never restores a TTL.
        let end = expires.min(issued.checked_add(elapsed * 1000).ok_or_else(denied)?);
        let remaining = end - c.utc_us;
        if remaining <= 0 {
            return Err(denied());
        }
        let cutoff = c
            .ticks_us
            .checked_add(u64::try_from(remaining).map_err(|_| denied())?)
            .ok_or_else(denied)?;
        Ok(Self {
            boot_id: c.boot_id.clone(),
            ticks_cutoff_us: cutoff,
            utc_cutoff_us: end.to_string(),
            sampled_ticks_us: c.ticks_us,
            sampled_utc_us: c.utc_us.to_string(),
            draining: false,
            projection_offset_us: Some((c.utc_us - i128::from(c.ticks_us)).to_string()),
        })
    }
    fn offset(&self) -> ComputeResult<i128> {
        self.projection_offset_us
            .as_ref()
            .ok_or_else(denied)?
            .parse()
            .map_err(|_| denied())
    }
    /// Preserve the strongest boot-wide projection, including any earlier shortening.
    pub(crate) fn inherit_projection(&mut self, old: &Self) -> ComputeResult<()> {
        let old_end = old.utc_cutoff_us.parse::<i128>().map_err(|_| denied())?;
        let old_bound_offset = old_end
            .checked_sub(i128::from(old.ticks_cutoff_us))
            .ok_or_else(denied)?;
        let offset = self.offset()?.max(old.offset()?).max(old_bound_offset);
        let advance = offset.checked_sub(self.offset()?).ok_or_else(denied)?;
        self.ticks_cutoff_us = self
            .ticks_cutoff_us
            .checked_sub(u64::try_from(advance).map_err(|_| denied())?)
            .ok_or_else(denied)?;
        self.projection_offset_us = Some(offset.to_string());
        Ok(())
    }
    /// Root persists forward observations even without a renewal. Rollback never restores time.
    pub fn observe(&mut self, c: &ClockSample) -> ComputeResult<bool> {
        self.remaining_us(c)?;
        let offset = c.utc_us - i128::from(c.ticks_us);
        let advance = offset.checked_sub(self.offset()?).ok_or_else(denied)?;
        if advance <= 0 {
            return Ok(false);
        }
        self.ticks_cutoff_us = self
            .ticks_cutoff_us
            .checked_sub(u64::try_from(advance).map_err(|_| denied())?)
            .ok_or_else(denied)?;
        self.projection_offset_us = Some(offset.to_string());
        Ok(true)
    }
    pub fn remaining_us(&self, c: &ClockSample) -> ComputeResult<u64> {
        let seen = self.sampled_utc_us.parse::<i128>().map_err(|_| denied())?;
        let end = self.utc_cutoff_us.parse::<i128>().map_err(|_| denied())?;
        if self.boot_id != c.boot_id
            || c.ticks_us < self.sampled_ticks_us
            || c.utc_us < seen
            || c.utc_us >= end
            || c.ticks_us >= self.ticks_cutoff_us
        {
            return Err(denied());
        }
        // An offset change cannot buy time. Large backward drift fails even if UTC did
        // not cross the original sample; forward changes may expire early.
        let offset = c.utc_us - i128::from(c.ticks_us);
        if self.offset()?.checked_sub(offset).ok_or_else(denied)? > 5_000_000 {
            return Err(denied());
        }
        Ok((self.ticks_cutoff_us - c.ticks_us)
            .min(u64::try_from(end - c.utc_us).map_err(|_| denied())?))
    }
}
/// Opaque verified grant consumed by claim and tool gates. No constructor from a duration.
#[derive(Clone)]
pub struct RunningAuthority {
    grant: Value,
    pub(crate) deadline: Deadline,
    pub(crate) digest: String,
}
impl RunningAuthority {
    pub fn verify(bytes: &[u8], binding: &Binding, c: &ClockSample) -> ComputeResult<Self> {
        if bytes.len() > 16384 {
            return Err(denied());
        }
        let message = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        validate_running_message(&message)?;
        if message["type"] != "compute_authorization" || message["environment"] != "development" {
            return Err(denied());
        }
        let g = &message["body"]["grant"];
        for field in [
            "customer_id",
            "logical_agent_id",
            "installation_id",
            "generation",
        ] {
            if message["authority"][field] != g[field] {
                return Err(denied());
            }
        }
        verify_signature(g, &message["body"], binding, false)?;
        binding.check(g)?;
        if g["audience"] != "craxii.worker:development"
            || g["boot_id"] != c.boot_id
            || g["environment"] != "development"
        {
            return Err(denied());
        }
        let deadline = Deadline::from_signed(g, c, true)?;
        Ok(Self {
            grant: g.clone(),
            deadline,
            digest: sha256(&canonical_bytes(g).map_err(|_| denied())?),
        })
    }
    pub fn grant_id(&self) -> ComputeResult<&str> {
        text(&self.grant, "grant_id")
    }
    pub fn grant(&self) -> &Value {
        &self.grant
    }
    pub fn check_work(
        &self,
        c: &ClockSample,
        work_id: &str,
        duration_ms: u64,
        margins_ms: u64,
    ) -> ComputeResult<()> {
        if !c.trusted
            || self.deadline.draining
            || duration_ms == 0
            || duration_ms > 900000
            || !self.grant["scopes"]
                .as_array()
                .ok_or_else(denied)?
                .iter()
                .any(|v| v == "bounded_execute")
            || !self.grant["permitted_work_ids"]
                .as_array()
                .ok_or_else(denied)?
                .iter()
                .any(|v| v == work_id)
        {
            return Err(denied());
        }
        let required = duration_ms
            .checked_add(margins_ms)
            .and_then(|v| v.checked_mul(1000))
            .ok_or_else(denied)?;
        if self.deadline.remaining_us(c)? <= required {
            return Err(denied());
        }
        Ok(())
    }
    /// Restore the root-accepted absolute timer; never now + TTL on worker restart.
    pub fn restore_deadline(&mut self, persisted: &Deadline, c: &ClockSample) -> ComputeResult<()> {
        persisted.remaining_us(c)?;
        if persisted.boot_id != self.deadline.boot_id
            || persisted.utc_cutoff_us != self.deadline.utc_cutoff_us
        {
            return Err(denied());
        }
        let cutoff = self.deadline.ticks_cutoff_us.min(persisted.ticks_cutoff_us);
        self.deadline = persisted.clone();
        self.deadline.ticks_cutoff_us = cutoff;
        Ok(())
    }
}
pub(crate) fn verify_signature(
    v: &Value,
    envelope: &Value,
    binding: &Binding,
    bootstrap: bool,
) -> ComputeResult<()> {
    let key = binary(&binding.public_key, Some(32)).map_err(|_| denied())?;
    if envelope["key_id"] != sha256(&key) {
        return Err(denied());
    }
    let preimage = if bootstrap {
        let mut b = b"CRAXII-COMPUTE-BOOTSTRAP\x001\x00".to_vec();
        b.extend(canonical_bytes(v).map_err(|_| denied())?);
        b
    } else {
        signature_input(SignaturePurpose::ComputeAuthorization, v).map_err(|_| denied())?
    };
    aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ED25519, key)
        .verify(
            &preimage,
            &binary(text(envelope, "signature")?, Some(64)).map_err(|_| denied())?,
        )
        .map_err(|_| denied())
}
#[derive(Clone)]
pub struct BootstrapAuthority {
    pub(crate) permit: Value,
    pub(crate) deadline: Deadline,
}
impl BootstrapAuthority {
    pub fn verify(bytes: &[u8], binding: &Binding, c: &ClockSample) -> ComputeResult<Self> {
        let v = parse_bounded(bytes, CONTROL_LIMITS).map_err(|_| denied())?;
        let p = &v["permit"];
        let expected = [
            "contract_version",
            "environment",
            "audience",
            "permit_id",
            "run_id",
            "power_operation_id",
            "period_id",
            "compute_reservation_id",
            "customer_id",
            "logical_agent_id",
            "installation_id",
            "generation",
            "authorization_epoch",
            "issued_at",
            "expires_at",
            "max_elapsed_ms",
            "permitted_scope",
            "request_digest",
            "policy_digest",
        ];
        let obj = p.as_object().ok_or_else(denied)?;
        if obj.len() != expected.len()
            || expected.iter().any(|k| !obj.contains_key(*k))
            || v.as_object().ok_or_else(denied)?.len() != 3
            || p["contract_version"] != 1
            || p["environment"] != "development"
            || p["audience"] != "craxii.compute-watchdog"
            || p["permitted_scope"] != "bootstrap_only"
        {
            return Err(denied());
        }
        for k in ["permit_id", "run_id", "compute_reservation_id"] {
            uuid(text(p, k)?, Some(4)).map_err(|_| denied())?;
        }
        for k in ["request_digest", "policy_digest"] {
            if !crate::distribution_contracts::primitives::format_valid("Digest", text(p, k)?) {
                return Err(denied());
            }
        }
        verify_signature(p, &v, binding, true)?;
        binding.check(p)?;
        let deadline = Deadline::from_signed(p, c, false)?;
        Ok(Self {
            permit: p.clone(),
            deadline,
        })
    }
}

// The root watchdog must not initialize every unrelated worker/model/result validator
// while its independent health timer is running. Compile only this frozen definition
// and its fixed primitive dependency; bytes and semantic checks remain authoritative.
fn running_validator() -> ComputeResult<&'static jsonschema::Validator> {
    static VALIDATOR: std::sync::OnceLock<ComputeResult<jsonschema::Validator>> =
        std::sync::OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let compute: Value=serde_json::from_str(include_str!("../../../contracts/distribution/1.0.0/schemas/compute.schema.json")).map_err(|_|denied())?;
        let primitives: Value=serde_json::from_str(include_str!("../../../contracts/distribution/1.0.0/schemas/primitives.schema.json")).map_err(|_|denied())?;
        let registry=jsonschema::Registry::new().add("urn:craxii:distribution:1.0.0:compute",compute).map_err(|_|denied())?.add("urn:craxii:distribution:1.0.0:primitives",primitives).map_err(|_|denied())?.prepare().map_err(|_|denied())?;
        let mut options=jsonschema::draft7::options().offline().with_registry(&registry).should_validate_formats(true).should_ignore_unknown_formats(false);
        for kind in ["U4","U7","Uuid","D0","Dp","U64","ChatId","Timestamp","Digest","Binary32","Binary64"] {
            options=options.with_format(format!("craxii_{kind}"),move |v|crate::distribution_contracts::primitives::format_valid(kind,v));
        }
        for max in [128usize,65536] { options=options.with_format(format!("craxii_utf8_{max}"),move |v|v.len()<=max); }
        options.build(&serde_json::json!({"$ref":"urn:craxii:distribution:1.0.0:compute#/definitions/ComputeAuthorization"})).map_err(|_|denied())
    }).as_ref().map_err(|e|*e)
}
pub fn prepare_verifier() -> ComputeResult<()> {
    running_validator().map(|_| ())
}
fn validate_running_message(value: &Value) -> ComputeResult<()> {
    let validator = running_validator()?;
    canonical_bytes(value).map_err(|_| denied())?;
    if !validator.is_valid(value) {
        return Err(denied());
    }
    check_semantics(value).map_err(|_| denied())
}

// Receipt v2 uses decimal strings: the bounded wire parser intentionally accepts
// only i31 JSON numbers. Boot microseconds exceed that range after 35 minutes.
mod tick_decimal {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(ticks: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&ticks.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(text) => {
                let ticks: u64 = text.parse().map_err(D::Error::custom)?;
                if ticks.to_string() != text {
                    return Err(D::Error::custom("invalid_ticks"));
                }
                Ok(ticks)
            }
            // Only authenticated legacy history can use numeric ticks.
            serde_json::Value::Number(n) => {
                n.as_u64().ok_or_else(|| D::Error::custom("invalid_ticks"))
            }
            _ => Err(D::Error::custom("invalid_ticks")),
        }
    }
}
