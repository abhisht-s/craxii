//! Stage 9 device credential values with an intentionally narrow secret surface.

use std::fmt;

use super::{DeviceId, Sha256Digest};

const BEARER_TOKEN_BYTES: usize = 32;
const BEARER_TOKEN_TEXT_BYTES: usize = BEARER_TOKEN_BYTES * 2;
pub const MAX_DEVICE_DISPLAY_NAME_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialValidationKind {
    InvalidBearerToken,
    InvalidDeviceDisplayName,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CredentialValidationError {
    kind: CredentialValidationKind,
}

impl CredentialValidationError {
    const fn new(kind: CredentialValidationKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> CredentialValidationKind {
        self.kind
    }
}

impl fmt::Display for CredentialValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            CredentialValidationKind::InvalidBearerToken => "invalid bearer credential",
            CredentialValidationKind::InvalidDeviceDisplayName => "invalid device display name",
        })
    }
}

impl fmt::Debug for CredentialValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialValidationError")
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::error::Error for CredentialValidationError {}

/// An exact V0 bearer secret. Raw text is private, non-serializable, and non-cloneable.
pub struct BearerToken {
    text: String,
}

impl BearerToken {
    /// Takes ownership only when the input is exact lowercase 64-character hexadecimal text.
    pub fn parse(text: String) -> Result<Self, CredentialValidationError> {
        if text.len() != BEARER_TOKEN_TEXT_BYTES
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CredentialValidationError::new(
                CredentialValidationKind::InvalidBearerToken,
            ));
        }
        Ok(Self { text })
    }

    /// Encodes trusted CSPRNG output with the exact lowercase V0 grammar.
    pub(crate) fn from_random_bytes(bytes: [u8; BEARER_TOKEN_BYTES]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = [0_u8; BEARER_TOKEN_TEXT_BYTES];
        for (index, byte) in bytes.into_iter().enumerate() {
            encoded[index * 2] = HEX[usize::from(byte >> 4)];
            encoded[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
        Self {
            text: String::from_utf8(encoded.to_vec())
                .expect("local lowercase hexadecimal encoding is valid UTF-8"),
        }
    }

    #[must_use]
    pub(crate) fn token_hash(&self) -> DeviceTokenHash {
        DeviceTokenHash(Sha256Digest::hash_bytes(self.text.as_bytes()))
    }

    /// Consumes the wrapper into the one-time issuance path owned by provisioning.
    pub(crate) fn into_issuance_text(self) -> String {
        self.text
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([REDACTED])")
    }
}

/// A semantically distinct SHA-256 digest of exact accepted bearer-text bytes.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DeviceTokenHash(Sha256Digest);

impl DeviceTokenHash {
    pub fn parse_canonical(input: &str) -> Result<Self, CredentialValidationError> {
        Sha256Digest::parse_canonical(input).map(Self).map_err(|_| {
            CredentialValidationError::new(CredentialValidationKind::InvalidBearerToken)
        })
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    #[must_use]
    pub(crate) fn canonical_text(self) -> String {
        self.0.to_string()
    }
}

impl fmt::Debug for DeviceTokenHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceTokenHash([REDACTED])")
    }
}

/// Exact safe operator-facing device label persisted without normalization.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct DeviceDisplayName(String);

impl DeviceDisplayName {
    pub fn try_new(value: String) -> Result<Self, CredentialValidationError> {
        if value.is_empty()
            || value.len() > MAX_DEVICE_DISPLAY_NAME_BYTES
            || value.chars().any(char::is_control)
            || value.trim() != value
        {
            return Err(CredentialValidationError::new(
                CredentialValidationKind::InvalidDeviceDisplayName,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeviceDisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DeviceDisplayName")
            .field(&self.0)
            .finish()
    }
}

/// Successful authentication carries only the durable device identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedDevice {
    device_id: DeviceId,
}

impl AuthenticatedDevice {
    #[must_use]
    pub(crate) const fn new(device_id: DeviceId) -> Self {
        Self { device_id }
    }

    #[must_use]
    pub const fn device_id(self) -> DeviceId {
        self.device_id
    }
}

/// Full-length comparison used only as defense in depth after digest-index lookup.
#[must_use]
pub fn device_token_hashes_equal(left: DeviceTokenHash, right: DeviceTokenHash) -> bool {
    let mut difference = 0_u8;
    for index in 0..32 {
        difference |= left.as_bytes()[index] ^ right.as_bytes()[index];
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_grammar_hashes_exact_text_and_redacts_every_format_surface() {
        let text = "0123456789abcdef".repeat(4);
        let token = BearerToken::parse(text.clone()).unwrap();
        assert_eq!(
            token.token_hash().canonical_text(),
            Sha256Digest::hash_bytes(text.as_bytes()).to_string()
        );
        assert_eq!(format!("{token:?}"), "BearerToken([REDACTED])");

        for rejected in [
            String::new(),
            "0".repeat(63),
            "0".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            format!(" {}", "0".repeat(64)),
            format!("{}\n", "0".repeat(64)),
            format!("bearer:{}", "0".repeat(64)),
        ] {
            let error = BearerToken::parse(rejected).unwrap_err();
            assert_eq!(error.kind(), CredentialValidationKind::InvalidBearerToken);
        }
    }

    #[test]
    fn random_bytes_have_exact_local_lowercase_hex_encoding() {
        let token = BearerToken::from_random_bytes([0xab; 32]);
        assert_eq!(token.into_issuance_text(), "ab".repeat(32));
    }

    #[test]
    fn device_names_preserve_exact_valid_text_and_reject_normalization_pressure() {
        let name = DeviceDisplayName::try_new("MacBook Pro é".to_owned()).unwrap();
        assert_eq!(name.as_str(), "MacBook Pro é");
        for rejected in [
            String::new(),
            " x".to_owned(),
            "x ".to_owned(),
            "x\ny".to_owned(),
            "é".repeat(65),
        ] {
            assert!(DeviceDisplayName::try_new(rejected).is_err());
        }
    }

    #[test]
    fn digest_comparison_checks_equal_length_bytes_without_text_access() {
        let first = BearerToken::parse("01".repeat(32)).unwrap().token_hash();
        let same = BearerToken::parse("01".repeat(32)).unwrap().token_hash();
        let different = BearerToken::parse("02".repeat(32)).unwrap().token_hash();
        assert!(device_token_hashes_equal(first, same));
        assert!(!device_token_hashes_equal(first, different));
        assert_eq!(format!("{first:?}"), "DeviceTokenHash([REDACTED])");
    }
}
