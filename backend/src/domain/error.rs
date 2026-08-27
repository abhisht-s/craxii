//! Minimal safe validation failures for canonical scalar construction.

use std::fmt;

/// The closed validation distinctions owned through Substage 3.2.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DomainValidationKind {
    /// UUID text is not the exact canonical lowercase hyphenated form.
    InvalidCanonicalUuid,
    /// A UUID is not a non-nil RFC-variant version 7 UUID.
    InvalidUuidVersionOrVariant,
    /// A committed sequence or ordinal is outside `1..=i64::MAX`.
    InvalidPositiveInteger,
    /// Checked arithmetic exceeded the value's upper bound.
    ArithmeticOverflow,
    /// Checked arithmetic would produce a negative duration.
    ArithmeticUnderflow,
    /// Timestamp text is not the exact canonical UTC microsecond form.
    InvalidCanonicalTimestamp,
    /// A trusted timestamp cannot be represented by the canonical format.
    TimestampOutOfRange,
    /// A digest is not exactly 64 lowercase hexadecimal characters.
    InvalidDigest,
    /// A byte count is outside `0..=i64::MAX`.
    InvalidByteCount,
    /// A text block is empty or otherwise violates the V0 text contract.
    InvalidText,
    /// Ordered message content violates the V1 block/count/combined-size contract.
    InvalidContent,
    /// The V0 primary-conversation topology is inconsistent.
    InvalidPrimaryConversation,
    /// Message role and immutable provenance fields do not match.
    InvalidMessageProvenance,
    /// Work input shape or the V0 one-trigger invariant is invalid.
    InvalidWorkInput,
    /// A workstation generation is outside `1..=i64::MAX`.
    InvalidWorkstationGeneration,
    /// A logical POSIX path is malformed or outside its canonical bound.
    InvalidLogicalPath,
    /// A bounded domain identifier/reference violates its exact grammar.
    InvalidBoundedIdentifier,
    /// A capability snapshot violates its V1 bounds or uniqueness rules.
    InvalidCapabilitySnapshot,
    /// An immutable evidence reference is structurally invalid.
    InvalidEvidenceReference,
}

/// A safe, typed scalar-validation failure.
///
/// This deliberately stores no rejected input, source error, path, content, or
/// provider detail. The full normalized error envelope belongs to Substage 3.3.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DomainValidationError {
    kind: DomainValidationKind,
}

impl DomainValidationError {
    /// The stable generic code frozen for scalar validation.
    pub const CODE: &'static str = "domain_validation";

    pub(crate) const fn new(kind: DomainValidationKind) -> Self {
        Self { kind }
    }

    /// Returns the stable safe code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        Self::CODE
    }

    /// Returns the closed validation kind.
    #[must_use]
    pub const fn kind(self) -> DomainValidationKind {
        self.kind
    }
}

impl fmt::Display for DomainValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            DomainValidationKind::InvalidCanonicalUuid => "invalid canonical UUID",
            DomainValidationKind::InvalidUuidVersionOrVariant => {
                "UUID must be a non-nil RFC-variant UUIDv7"
            }
            DomainValidationKind::InvalidPositiveInteger => {
                "value must be a positive signed 64-bit integer"
            }
            DomainValidationKind::ArithmeticOverflow => "checked arithmetic overflow",
            DomainValidationKind::ArithmeticUnderflow => "checked arithmetic underflow",
            DomainValidationKind::InvalidCanonicalTimestamp => "invalid canonical UTC timestamp",
            DomainValidationKind::TimestampOutOfRange => "timestamp is outside the canonical range",
            DomainValidationKind::InvalidDigest => "invalid canonical SHA-256 digest",
            DomainValidationKind::InvalidByteCount => {
                "byte count must fit a nonnegative signed 64-bit integer"
            }
            DomainValidationKind::InvalidText => "invalid V0 text block",
            DomainValidationKind::InvalidContent => "invalid V1 message content",
            DomainValidationKind::InvalidPrimaryConversation => {
                "invalid V0 primary-conversation topology"
            }
            DomainValidationKind::InvalidMessageProvenance => {
                "invalid committed-message provenance"
            }
            DomainValidationKind::InvalidWorkInput => "invalid V0 work input",
            DomainValidationKind::InvalidWorkstationGeneration => {
                "workstation generation must be a positive signed 64-bit integer"
            }
            DomainValidationKind::InvalidLogicalPath => "invalid logical POSIX path",
            DomainValidationKind::InvalidBoundedIdentifier => "invalid bounded domain identifier",
            DomainValidationKind::InvalidCapabilitySnapshot => {
                "invalid V1 workstation capability snapshot"
            }
            DomainValidationKind::InvalidEvidenceReference => {
                "invalid immutable evidence reference"
            }
        };
        formatter.write_str(message)
    }
}

impl fmt::Debug for DomainValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DomainValidationError")
            .field("code", &Self::CODE)
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::error::Error for DomainValidationError {}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{DomainValidationError, DomainValidationKind};
    use crate::domain::{MessageId, Sha256Digest, UtcTimestamp};

    #[test]
    fn code_and_closed_kinds_are_exact() {
        let kinds = [
            DomainValidationKind::InvalidCanonicalUuid,
            DomainValidationKind::InvalidUuidVersionOrVariant,
            DomainValidationKind::InvalidPositiveInteger,
            DomainValidationKind::ArithmeticOverflow,
            DomainValidationKind::ArithmeticUnderflow,
            DomainValidationKind::InvalidCanonicalTimestamp,
            DomainValidationKind::TimestampOutOfRange,
            DomainValidationKind::InvalidDigest,
            DomainValidationKind::InvalidByteCount,
            DomainValidationKind::InvalidText,
            DomainValidationKind::InvalidContent,
            DomainValidationKind::InvalidPrimaryConversation,
            DomainValidationKind::InvalidMessageProvenance,
            DomainValidationKind::InvalidWorkInput,
            DomainValidationKind::InvalidWorkstationGeneration,
            DomainValidationKind::InvalidLogicalPath,
            DomainValidationKind::InvalidBoundedIdentifier,
            DomainValidationKind::InvalidCapabilitySnapshot,
            DomainValidationKind::InvalidEvidenceReference,
        ];

        for kind in kinds {
            let error = DomainValidationError::new(kind);
            assert_eq!(error.code(), "domain_validation");
            assert_eq!(error.kind(), kind);
        }
    }

    fn assert_parser_error_is_redacted(
        error: DomainValidationError,
        expected_kind: DomainValidationKind,
        rejected_input: &str,
        sentinel: &str,
    ) {
        assert!(rejected_input.contains(sentinel));
        assert_eq!(error.code(), "domain_validation");
        assert_eq!(error.kind(), expected_kind);

        let display = error.to_string();
        let debug = format!("{error:?}");
        let kind = format!("{:?}", error.kind());

        assert!(!display.contains(sentinel));
        assert!(!debug.contains(sentinel));
        assert!(!error.code().contains(sentinel));
        assert!(!kind.contains(sentinel));
        assert!(error.source().is_none());
    }

    #[test]
    fn parser_produced_errors_do_not_retain_or_expose_rejected_input() {
        let uuid_sentinel = "uuid-rejected-sentinel-81a6f390";
        let uuid_input = format!("invalid-{uuid_sentinel}");
        let uuid_error = uuid_input.parse::<MessageId>().expect_err("must reject");
        assert_parser_error_is_redacted(
            uuid_error,
            DomainValidationKind::InvalidCanonicalUuid,
            &uuid_input,
            uuid_sentinel,
        );

        let timestamp_sentinel = "timestamp-rejected-sentinel-5c92e147";
        let timestamp_input = format!("2026-08-27T12:34:{timestamp_sentinel}Z");
        let timestamp_error = timestamp_input
            .parse::<UtcTimestamp>()
            .expect_err("must reject");
        assert_parser_error_is_redacted(
            timestamp_error,
            DomainValidationKind::InvalidCanonicalTimestamp,
            &timestamp_input,
            timestamp_sentinel,
        );

        let digest_sentinel = "digest-rejected-sentinel-b3472a0c";
        let digest_input = format!("0123456789abcdef{digest_sentinel}0123456789abcdef");
        let digest_error = digest_input
            .parse::<Sha256Digest>()
            .expect_err("must reject");
        assert_parser_error_is_redacted(
            digest_error,
            DomainValidationKind::InvalidDigest,
            &digest_input,
            digest_sentinel,
        );
    }
}
