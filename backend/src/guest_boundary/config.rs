use super::{BoundaryError::GuestConfigInvalid, BoundaryResult};
use crate::distribution_contracts::{
    canonical::{canonical_with_limits, sha256},
    raw_json::{Limits, parse_bounded},
};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::OnceLock;
pub const VERSION: u32 = 1;
pub const SCHEMA_SHA256: &str = "b99ae1fe8965f34974b90e323cc79d9b68c48b79df3384fd8d6f7bc0b1560102";
pub const FIXTURES_SHA256: &str =
    "87f5b3bad1ef0182d7a2c54ba1c5d2300bf00fc81c86dd81956ce1032db7211e";
pub const LIMITS: Limits = Limits {
    bytes: 16384,
    nodes: 4096,
    depth: 16,
};
static SCHEMA: &str = include_str!("../../../ops/v0.0.1/guest-bootstrap.schema.json");
static VALIDATOR: OnceLock<Option<jsonschema::Validator>> = OnceLock::new();
#[derive(Clone)]
pub struct GuestConfig {
    value: Value,
}
impl GuestConfig {
    pub fn decode(bytes: &[u8]) -> BoundaryResult<Self> {
        let value = parse_bounded(bytes, LIMITS).map_err(|_| GuestConfigInvalid)?;
        let validator = VALIDATOR
            .get_or_init(|| {
                if sha256(SCHEMA.as_bytes()) != SCHEMA_SHA256 {
                    return None;
                }
                let schema: Value = serde_json::from_str(SCHEMA).ok()?;
                jsonschema::options()
                    .with_draft(jsonschema::Draft::Draft7)
                    .build(&schema)
                    .ok()
            })
            .as_ref()
            .ok_or(GuestConfigInvalid)?;
        if !validator.is_valid(&value) {
            return Err(GuestConfigInvalid);
        }
        let c = Self { value };
        c.check()?;
        Ok(c)
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn text(&self, name: &str) -> &str {
        self.value[name].as_str().expect("validated guest field")
    }
    pub fn synthetic(&self) -> bool {
        self.value["provenance"]["kind"] == "synthetic_fixture"
    }
    pub fn canonical(&self) -> BoundaryResult<Vec<u8>> {
        canonical_with_limits(&self.value, LIMITS).map_err(|_| GuestConfigInvalid)
    }
    pub fn digest(&self) -> BoundaryResult<String> {
        Ok(sha256(&self.canonical()?))
    }
    fn check(&self) -> BoundaryResult<()> {
        let v = &self.value;
        let generation = self
            .text("generation")
            .parse::<i64>()
            .map_err(|_| GuestConfigInvalid)?;
        let az = self.text("availability_zone");
        if generation <= 0 || &az[..az.len() - 1] != self.text("region") {
            return Err(GuestConfigInvalid);
        }
        for cidr in v["private_networks"].as_array().ok_or(GuestConfigInvalid)? {
            if !canonical_cidr(cidr.as_str().ok_or(GuestConfigInvalid)?) {
                return Err(GuestConfigInvalid);
            }
        }
        for key in [
            "customer_id",
            "logical_agent_id",
            "disk_lineage_id",
            "filesystem_uuid",
        ] {
            if v["marker"][key] != v[key] {
                return Err(GuestConfigInvalid);
            }
        }
        for field in ["marker", "hardening", "resources"] {
            if v[format!("{field}_sha256")]
                != sha256(
                    &canonical_with_limits(&v[field], LIMITS).map_err(|_| GuestConfigInvalid)?,
                )
            {
                return Err(GuestConfigInvalid);
            }
        }
        match self.text("mode") {
            "initialize_blank" => {
                for (a, b) in [
                    ("birth_volume_id", "volume_id"),
                    ("birth_installation_id", "installation_id"),
                    ("birth_operation_id", "bootstrap_operation_id"),
                ] {
                    if v["marker"][a] != v[b] {
                        return Err(GuestConfigInvalid);
                    }
                }
            }
            "adopt_existing" if v["marker"]["birth_volume_id"] != v["volume_id"] => {
                return Err(GuestConfigInvalid);
            }
            _ => (),
        }
        if self.text("mode") == "adopt_restored" {
            if v["restore"].is_null()
                || v["restore"]["disk_lineage_id"] != v["disk_lineage_id"]
                || v["restore"]["source_volume_id"] == v["volume_id"]
            {
                return Err(GuestConfigInvalid);
            }
        } else if !v["restore"].is_null() {
            return Err(GuestConfigInvalid);
        }
        let mut binding = v.clone();
        binding
            .as_object_mut()
            .ok_or(GuestConfigInvalid)?
            .remove("provenance");
        if v["provenance"]["binding_sha256"]
            != sha256(&canonical_with_limits(&binding, LIMITS).map_err(|_| GuestConfigInvalid)?)
        {
            return Err(GuestConfigInvalid);
        }
        Ok(())
    }
}
pub fn canonical_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(ip) = address.parse::<IpAddr>() else {
        return false;
    };
    let Ok(bits) = prefix.parse::<u32>() else {
        return false;
    };
    if prefix != bits.to_string() || address != ip.to_string() {
        return false;
    }
    match ip {
        IpAddr::V4(ip) if bits <= 32 => u32::from(ip) as u128 & ((1u128 << (32 - bits)) - 1) == 0,
        IpAddr::V6(ip) if bits <= 128 => {
            if address.contains('.') || address.contains('%') {
                return false;
            }
            let mask = if bits == 0 {
                u128::MAX
            } else {
                (1u128 << (128 - bits)) - 1
            };
            u128::from(ip) & mask == 0
        }
        _ => false,
    }
}
