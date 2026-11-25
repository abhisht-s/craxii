//! Central UUIDv4 identities are deliberately distinct from local UUIDv7 identities.
use super::{ContractResult, invalid};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::{Uuid, Variant};
pub fn uuid(value: &str, version: Option<u8>) -> ContractResult<()> {
    let id = Uuid::parse_str(value).map_err(|_| invalid())?;
    if value.len() != 36
        || id.hyphenated().to_string() != value
        || id.is_nil()
        || id.get_variant() != Variant::RFC4122
        || version.is_some_and(|v| id.get_version_num() != usize::from(v))
        || !(1..=8).contains(&id.get_version_num())
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn decimal(value: &str, kind: &str) -> ContractResult<i128> {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    if value.is_empty()
        || value.len() > 20
        || unsigned.is_empty()
        || !unsigned.bytes().all(|c| c.is_ascii_digit())
        || (unsigned.len() > 1 && unsigned.starts_with('0'))
        || (value.starts_with('-') && kind != "ChatId")
        || ((kind == "Dp" || kind == "ChatId") && unsigned == "0")
    {
        return Err(invalid());
    }
    let n = value.parse::<i128>().map_err(|_| invalid())?;
    let max = if kind == "U64" {
        u64::MAX as i128
    } else {
        i64::MAX as i128
    };
    if n > max || n < i64::MIN as i128 {
        return Err(invalid());
    }
    Ok(n)
}
pub fn timestamp_micros(value: &str) -> ContractResult<i128> {
    if value.len() != 27 || value.starts_with("0000") {
        return Err(invalid());
    }
    let t = crate::domain::UtcTimestamp::parse_canonical(value).map_err(|_| invalid())?;
    Ok(t.to_offset_datetime().unix_timestamp_nanos() / 1000 + 62_135_596_800_000_000i128)
}
pub fn binary(value: &str, length: Option<usize>) -> ContractResult<Vec<u8>> {
    let b = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    if URL_SAFE_NO_PAD.encode(&b) != value || length.is_some_and(|n| b.len() != n) {
        return Err(invalid());
    }
    Ok(b)
}
pub fn format_valid(name: &str, value: &str) -> bool {
    match name {
        "U4" => uuid(value, Some(4)).is_ok(),
        "U7" => uuid(value, Some(7)).is_ok(),
        "Uuid" => uuid(value, None).is_ok(),
        "Timestamp" => timestamp_micros(value).is_ok(),
        "Binary32" => binary(value, Some(32)).is_ok(),
        "Binary64" => binary(value, Some(64)).is_ok(),
        "Digest" => {
            value.len() == 64
                && value
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }
        "D0" | "Dp" | "U64" | "ChatId" => decimal(value, name).is_ok(),
        _ => false,
    }
}
macro_rules! central_id {
    ($name:ident) => {
        #[derive(Clone, Eq, PartialEq, Serialize)]
        pub struct $name(String);
        impl $name {
            pub fn parse(value: &str) -> ContractResult<Self> {
                uuid(value, Some(4))?;
                Ok(Self(value.into()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(|_| serde::de::Error::custom("invalid_contract"))
            }
        }
    };
}
central_id!(CentralCustomerId);
central_id!(LogicalAgentId);
central_id!(InstallationId);
central_id!(CanonicalWorkId);
central_id!(CanonicalConversationId);
macro_rules! primitive {
    ($name:ident,$kind:literal) => {
        #[derive(Clone, Eq, PartialEq, Serialize)]
        pub struct $name(String);
        impl $name {
            pub fn parse(value: &str) -> ContractResult<Self> {
                if !format_valid($kind, value) {
                    return Err(invalid());
                }
                Ok(Self(value.into()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(|_| serde::de::Error::custom("invalid_contract"))
            }
        }
    };
}
primitive!(U4, "U4");
primitive!(U7, "U7");
primitive!(WireUuid, "Uuid");
primitive!(DecimalI64, "D0");
primitive!(PositiveI64, "Dp");
primitive!(DecimalU64, "U64");
primitive!(ChatId, "ChatId");
primitive!(WireTimestamp, "Timestamp");
primitive!(WireDigest, "Digest");
primitive!(Binary32, "Binary32");
primitive!(Binary64, "Binary64");
/// Session material is never available through Debug or Display.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionCredential(Binary32);
impl std::fmt::Debug for SessionCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
