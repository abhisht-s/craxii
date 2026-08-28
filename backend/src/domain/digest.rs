//! Canonical SHA-256 and bounded byte-count values.

use std::{fmt, str::FromStr};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use sha2::{Digest, Sha256};

use super::error::{DomainValidationError, DomainValidationKind};

/// An exact SHA-256 value with a lowercase 64-character hexadecimal boundary.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Constructs a digest from the exact 32 digest bytes produced by a trusted hasher.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Hashes the provided bytes with SHA-256.
    #[must_use]
    pub fn hash_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Parses exactly 64 lowercase hexadecimal characters.
    pub fn parse_canonical(input: &str) -> Result<Self, DomainValidationError> {
        if input.len() != 64 {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidDigest,
            ));
        }

        fn nibble(byte: u8) -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                _ => None,
            }
        }

        let mut bytes = [0_u8; 32];
        let (pairs, remainder) = input.as_bytes().as_chunks::<2>();
        debug_assert!(remainder.is_empty());
        for (index, pair) in pairs.iter().enumerate() {
            let high = nibble(pair[0])
                .ok_or_else(|| DomainValidationError::new(DomainValidationKind::InvalidDigest))?;
            let low = nibble(pair[1])
                .ok_or_else(|| DomainValidationError::new(DomainValidationKind::InvalidDigest))?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    /// Returns the exact digest bytes without exposing a mutable representation.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for Sha256Digest {
    type Err = DomainValidationError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse_canonical(input)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sha256Digest")
            .field(&self.to_string())
            .finish()
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DigestVisitor;

        impl<'de> Visitor<'de> for DigestVisitor {
            type Value = Sha256Digest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("exactly 64 lowercase hexadecimal SHA-256 characters")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Sha256Digest::parse_canonical(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DigestVisitor)
    }
}

/// A nonnegative byte count bounded to the shared SQLite/Swift signed-64-bit range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalByteCount(i64);

impl CanonicalByteCount {
    /// The greatest canonical byte count.
    pub const MAX: u64 = i64::MAX as u64;

    /// Constructs a count in `0..=i64::MAX` without truncation.
    pub const fn try_new(value: u64) -> Result<Self, DomainValidationError> {
        if value <= Self::MAX {
            Ok(Self(value as i64))
        } else {
            Err(DomainValidationError::new(
                DomainValidationKind::InvalidByteCount,
            ))
        }
    }

    /// Converts a platform byte length without truncation.
    pub fn try_from_usize(value: usize) -> Result<Self, DomainValidationError> {
        let value = u64::try_from(value)
            .map_err(|_| DomainValidationError::new(DomainValidationKind::InvalidByteCount))?;
        Self::try_new(value)
    }

    /// Returns the nonnegative count as `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0 as u64
    }

    /// Adds a count without overflow or truncation.
    pub fn checked_add(self, amount: u64) -> Result<Self, DomainValidationError> {
        self.get()
            .checked_add(amount)
            .ok_or_else(|| DomainValidationError::new(DomainValidationKind::ArithmeticOverflow))
            .and_then(|value| {
                Self::try_new(value).map_err(|_| {
                    DomainValidationError::new(DomainValidationKind::ArithmeticOverflow)
                })
            })
    }
}

impl TryFrom<u64> for CanonicalByteCount {
    type Error = DomainValidationError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl TryFrom<usize> for CanonicalByteCount {
    type Error = DomainValidationError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::try_from_usize(value)
    }
}

impl TryFrom<i64> for CanonicalByteCount {
    type Error = DomainValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map_err(|_| DomainValidationError::new(DomainValidationKind::InvalidByteCount))
            .and_then(Self::try_new)
    }
}

impl Serialize for CanonicalByteCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for CanonicalByteCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ByteCountVisitor;

        impl<'de> Visitor<'de> for ByteCountVisitor {
            type Value = CanonicalByteCount;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a nonnegative signed-64-bit JSON byte count")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                CanonicalByteCount::try_from(value).map_err(E::custom)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                CanonicalByteCount::try_new(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(ByteCountVisitor)
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{DefaultHasher, Hash, Hasher};

    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_known_fixture_display_hash_and_serde_are_exact() {
        let digest = Sha256Digest::hash_bytes(b"abc");
        assert_eq!(digest.to_string(), ABC_SHA256);
        assert_eq!(digest.as_bytes().len(), 32);
        assert_eq!(
            format!("{digest:?}"),
            format!("Sha256Digest(\"{ABC_SHA256}\")")
        );

        let parsed: Sha256Digest = ABC_SHA256.parse().unwrap();
        assert_eq!(parsed, digest);
        let mut left = DefaultHasher::new();
        digest.hash(&mut left);
        let mut right = DefaultHasher::new();
        parsed.hash(&mut right);
        assert_eq!(left.finish(), right.finish());

        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, format!("\"{ABC_SHA256}\""));
        assert_eq!(serde_json::from_str::<Sha256Digest>(&json).unwrap(), digest);
    }

    #[test]
    fn digest_rejects_uppercase_wrong_length_nonhex_and_whitespace() {
        let rejected = [
            ABC_SHA256.to_uppercase(),
            ABC_SHA256[..63].to_owned(),
            format!("{ABC_SHA256}0"),
            format!("{}g", &ABC_SHA256[..63]),
            format!(" {ABC_SHA256}"),
        ];

        for input in rejected {
            let error = input.parse::<Sha256Digest>().expect_err("must reject");
            assert_eq!(error.kind(), DomainValidationKind::InvalidDigest);
            let json = serde_json::to_string(&input).unwrap();
            assert!(serde_json::from_str::<Sha256Digest>(&json).is_err());
        }
    }

    #[test]
    fn byte_count_accepts_exact_bounds_and_numeric_serde() {
        let zero = CanonicalByteCount::try_new(0).unwrap();
        let maximum = CanonicalByteCount::try_new(CanonicalByteCount::MAX).unwrap();
        assert_eq!(zero.get(), 0);
        assert_eq!(maximum.get(), i64::MAX as u64);
        assert!(zero < maximum);
        assert_eq!(serde_json::to_string(&zero).unwrap(), "0");
        assert_eq!(
            serde_json::to_string(&maximum).unwrap(),
            i64::MAX.to_string()
        );
        assert_eq!(
            serde_json::from_str::<CanonicalByteCount>(&i64::MAX.to_string()).unwrap(),
            maximum
        );
    }

    #[test]
    fn byte_count_conversions_and_arithmetic_never_overflow_or_truncate() {
        assert_eq!(CanonicalByteCount::try_from_usize(17).unwrap().get(), 17);
        assert_eq!(CanonicalByteCount::try_from(18_u64).unwrap().get(), 18);
        assert_eq!(CanonicalByteCount::try_from(19_i64).unwrap().get(), 19);
        assert_eq!(
            CanonicalByteCount::try_from(-1_i64).unwrap_err().kind(),
            DomainValidationKind::InvalidByteCount
        );
        assert_eq!(
            CanonicalByteCount::try_new(CanonicalByteCount::MAX + 1)
                .unwrap_err()
                .kind(),
            DomainValidationKind::InvalidByteCount
        );
        assert_eq!(
            CanonicalByteCount::try_new(CanonicalByteCount::MAX)
                .unwrap()
                .checked_add(1)
                .unwrap_err()
                .kind(),
            DomainValidationKind::ArithmeticOverflow
        );
        assert!(serde_json::from_str::<CanonicalByteCount>("-1").is_err());
        assert!(serde_json::from_str::<CanonicalByteCount>("9223372036854775808").is_err());
        assert!(serde_json::from_str::<CanonicalByteCount>("\"0\"").is_err());

        if usize::BITS > 63 {
            assert_eq!(
                CanonicalByteCount::try_from_usize(usize::MAX)
                    .unwrap_err()
                    .kind(),
                DomainValidationKind::InvalidByteCount
            );
        }
    }
}
