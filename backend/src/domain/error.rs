//! Safe local validation failures and the dependency-neutral normalized error model.

use std::fmt;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

macro_rules! stable_string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $literal:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Eq, PartialEq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Returns the exact stable serialized literal.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $literal),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(self.as_str(), formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct StableLiteralVisitor;

                impl<'de> de::Visitor<'de> for StableLiteralVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("an approved stable lowercase literal")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($literal => Ok($name::$variant)),+,
                            _ => Err(E::unknown_variant(value, &[$($literal),+])),
                        }
                    }
                }

                deserializer.deserialize_str(StableLiteralVisitor)
            }
        }
    };
}

stable_string_enum! {
    /// The exact V0 normalized failure categories.
    pub enum ErrorCategory {
        AuthenticationError => "authentication_error",
        ClientProtocolError => "client_protocol_error",
        IdempotencyError => "idempotency_error",
        StorageError => "storage_error",
        StateConflict => "state_conflict",
        ContextError => "context_error",
        ModelSelectionError => "model_selection_error",
        ProviderError => "provider_error",
        ToolValidationError => "tool_validation_error",
        AuthorityError => "authority_error",
        WorkstationError => "workstation_error",
        ArtifactError => "artifact_error",
        CancellationError => "cancellation_error",
        InternalInvariantError => "internal_invariant_error",
    }
}

stable_string_enum! {
    /// Advisory retry classifications; these values never authorize replay.
    pub enum Retryability {
        Never => "never",
        Bounded => "bounded",
        UserAction => "user_action",
        OperatorAction => "operator_action",
    }
}

stable_string_enum! {
    /// Whether an external action's terminal outcome is known.
    pub enum Certainty {
        Definite => "definite",
        OutcomeUnknown => "outcome_unknown",
    }
}

/// An opaque, allowlisted stable error code.
///
/// The public surface intentionally has no arbitrary string constructor. Later
/// owning stages may add explicit constants and extend the private allowlist.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// A domain constructor or validation failure at its explicitly mapped boundary.
    pub const DOMAIN_VALIDATION: Self = Self("domain_validation");
    /// The generic authentication code.
    pub const AUTHENTICATION_ERROR: Self = Self("authentication_error");
    /// The generic client protocol code.
    pub const CLIENT_PROTOCOL_ERROR: Self = Self("client_protocol_error");
    /// The generic idempotency code.
    pub const IDEMPOTENCY_ERROR: Self = Self("idempotency_error");
    /// The generic storage code.
    pub const STORAGE_ERROR: Self = Self("storage_error");
    /// The generic state-conflict code.
    pub const STATE_CONFLICT: Self = Self("state_conflict");
    /// The generic context code.
    pub const CONTEXT_ERROR: Self = Self("context_error");
    /// Mandatory eligible context plus requested output exceeds the selected target or byte limit.
    pub const CONTEXT_LIMIT_EXCEEDED: Self = Self("context_limit_exceeded");
    /// The generic model-selection code.
    pub const MODEL_SELECTION_ERROR: Self = Self("model_selection_error");
    /// The generic provider code.
    pub const PROVIDER_ERROR: Self = Self("provider_error");
    /// The generic tool-validation code.
    pub const TOOL_VALIDATION_ERROR: Self = Self("tool_validation_error");
    /// The generic authority code.
    pub const AUTHORITY_ERROR: Self = Self("authority_error");
    /// The generic workstation code.
    pub const WORKSTATION_ERROR: Self = Self("workstation_error");
    /// The generic artifact code.
    pub const ARTIFACT_ERROR: Self = Self("artifact_error");
    /// The generic cancellation code.
    pub const CANCELLATION_ERROR: Self = Self("cancellation_error");
    /// The generic internal-invariant code.
    pub const INTERNAL_INVARIANT_ERROR: Self = Self("internal_invariant_error");

    /// The exact Stage 3.3 allowlist in stable declaration order.
    pub const ALL: [Self; 16] = [
        Self::DOMAIN_VALIDATION,
        Self::AUTHENTICATION_ERROR,
        Self::CLIENT_PROTOCOL_ERROR,
        Self::IDEMPOTENCY_ERROR,
        Self::STORAGE_ERROR,
        Self::STATE_CONFLICT,
        Self::CONTEXT_ERROR,
        Self::CONTEXT_LIMIT_EXCEEDED,
        Self::MODEL_SELECTION_ERROR,
        Self::PROVIDER_ERROR,
        Self::TOOL_VALIDATION_ERROR,
        Self::AUTHORITY_ERROR,
        Self::WORKSTATION_ERROR,
        Self::ARTIFACT_ERROR,
        Self::CANCELLATION_ERROR,
        Self::INTERNAL_INVARIANT_ERROR,
    ];

    /// Returns the exact stable serialized code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    const fn for_category(category: ErrorCategory) -> Self {
        match category {
            ErrorCategory::AuthenticationError => Self::AUTHENTICATION_ERROR,
            ErrorCategory::ClientProtocolError => Self::CLIENT_PROTOCOL_ERROR,
            ErrorCategory::IdempotencyError => Self::IDEMPOTENCY_ERROR,
            ErrorCategory::StorageError => Self::STORAGE_ERROR,
            ErrorCategory::StateConflict => Self::STATE_CONFLICT,
            ErrorCategory::ContextError => Self::CONTEXT_ERROR,
            ErrorCategory::ModelSelectionError => Self::MODEL_SELECTION_ERROR,
            ErrorCategory::ProviderError => Self::PROVIDER_ERROR,
            ErrorCategory::ToolValidationError => Self::TOOL_VALIDATION_ERROR,
            ErrorCategory::AuthorityError => Self::AUTHORITY_ERROR,
            ErrorCategory::WorkstationError => Self::WORKSTATION_ERROR,
            ErrorCategory::ArtifactError => Self::ARTIFACT_ERROR,
            ErrorCategory::CancellationError => Self::CANCELLATION_ERROR,
            ErrorCategory::InternalInvariantError => Self::INTERNAL_INVARIANT_ERROR,
        }
    }

    fn approved(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.0 == value)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, formatter)
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ErrorCodeVisitor;

        impl de::Visitor<'_> for ErrorCodeVisitor {
            type Value = ErrorCode;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an explicitly approved stable error code")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                ErrorCode::approved(value).ok_or_else(|| {
                    E::unknown_variant(
                        value,
                        &[
                            "domain_validation",
                            "authentication_error",
                            "client_protocol_error",
                            "idempotency_error",
                            "storage_error",
                            "state_conflict",
                            "context_error",
                            "context_limit_exceeded",
                            "model_selection_error",
                            "provider_error",
                            "tool_validation_error",
                            "authority_error",
                            "workstation_error",
                            "artifact_error",
                            "cancellation_error",
                            "internal_invariant_error",
                        ],
                    )
                })
            }
        }

        deserializer.deserialize_str(ErrorCodeVisitor)
    }
}

/// An allowlisted static message safe for user/client-facing error surfaces.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SafeMessage(&'static str);

impl SafeMessage {
    /// The exact client-boundary validation message.
    pub const CLIENT_VALIDATION: Self = Self("The supplied value is invalid.");
    /// The generic authentication message.
    pub const AUTHENTICATION: Self = Self("Authentication is required.");
    /// The generic client-protocol message.
    pub const CLIENT_PROTOCOL: Self = Self("The request is invalid.");
    /// The generic idempotency message.
    pub const IDEMPOTENCY: Self = Self("The request conflicts with an earlier request.");
    /// The generic storage message.
    pub const STORAGE: Self = Self("A storage operation failed.");
    /// The generic state-conflict message.
    pub const STATE_CONFLICT: Self =
        Self("The requested operation conflicts with the current state.");
    /// The generic context message.
    pub const CONTEXT: Self = Self("The requested context cannot be processed.");
    /// The generic model-selection message.
    pub const MODEL_SELECTION: Self = Self("No suitable model is currently available.");
    /// The generic provider message.
    pub const PROVIDER: Self = Self("The model provider request failed.");
    /// The generic tool-validation message.
    pub const TOOL_VALIDATION: Self = Self("The tool request is invalid.");
    /// The generic authority message.
    pub const AUTHORITY: Self = Self("The requested operation is not permitted.");
    /// The generic workstation message.
    pub const WORKSTATION: Self = Self("The workstation operation failed.");
    /// The generic artifact message.
    pub const ARTIFACT: Self = Self("The artifact operation failed.");
    /// The generic cancellation message.
    pub const CANCELLATION: Self = Self("The operation could not be confirmed as cancelled.");
    /// The generic internal-invariant message.
    pub const INTERNAL_INVARIANT: Self = Self("An internal consistency error occurred.");

    /// Returns the approved static message.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for SafeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl fmt::Debug for SafeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, formatter)
    }
}

impl Serialize for SafeMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceStatusKind {
    ProviderHttp,
    OsErrno,
}

/// A safe structured numeric status from a provider or operating system boundary.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SourceStatus {
    kind: SourceStatusKind,
    code: i32,
}

impl SourceStatus {
    /// Constructs an HTTP provider status when `code` is in `100..=599`.
    #[must_use]
    pub const fn provider_http(code: u16) -> Option<Self> {
        if code >= 100 && code <= 599 {
            Some(Self {
                kind: SourceStatusKind::ProviderHttp,
                code: code as i32,
            })
        } else {
            None
        }
    }

    /// Constructs an OS errno when `code` is in `1..=i32::MAX`.
    #[must_use]
    pub const fn os_errno(code: i32) -> Option<Self> {
        if code >= 1 {
            Some(Self {
                kind: SourceStatusKind::OsErrno,
                code,
            })
        } else {
            None
        }
    }

    /// Returns the exact stable source-status kind literal.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self.kind {
            SourceStatusKind::ProviderHttp => "provider_http",
            SourceStatusKind::OsErrno => "os_errno",
        }
    }

    /// Returns the validated structured numeric code.
    #[must_use]
    pub const fn code(self) -> i32 {
        self.code
    }
}

impl fmt::Display for SourceStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind(), self.code)
    }
}

impl fmt::Debug for SourceStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceStatus")
            .field("kind", &self.kind())
            .field("code", &self.code)
            .finish()
    }
}

impl Serialize for SourceStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SourceStatus", 2)?;
        state.serialize_field("kind", self.kind())?;
        state.serialize_field("code", &self.code)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for SourceStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "kind", deny_unknown_fields)]
        enum WireStatus {
            #[serde(rename = "provider_http")]
            ProviderHttp { code: i64 },
            #[serde(rename = "os_errno")]
            OsErrno { code: i64 },
        }

        match WireStatus::deserialize(deserializer)? {
            WireStatus::ProviderHttp { code } => u16::try_from(code)
                .ok()
                .and_then(Self::provider_http)
                .ok_or_else(|| de::Error::custom("provider HTTP status must be in 100..=599")),
            WireStatus::OsErrno { code } => i32::try_from(code)
                .ok()
                .and_then(Self::os_errno)
                .ok_or_else(|| de::Error::custom("OS errno must be in 1..=i32::MAX")),
        }
    }
}

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

#[derive(Clone, Copy)]
enum InternalDetailKind {
    InvalidCanonicalUuid,
    InvalidUuidVersionOrVariant,
    InvalidPositiveInteger,
    ArithmeticOverflow,
    ArithmeticUnderflow,
    InvalidCanonicalTimestamp,
    TimestampOutOfRange,
    InvalidDigest,
    InvalidByteCount,
    InvalidText,
    InvalidContent,
    InvalidPrimaryConversation,
    InvalidMessageProvenance,
    InvalidWorkInput,
    InvalidWorkstationGeneration,
    InvalidLogicalPath,
    InvalidBoundedIdentifier,
    InvalidCapabilitySnapshot,
    InvalidEvidenceReference,
}

/// Opaque trace-only sanitized diagnostic metadata.
///
/// Construction is crate-private. This type deliberately implements none of
/// Display, Debug, Serialize, or Deserialize and exposes no raw-value getter.
pub struct InternalDetail(InternalDetailKind);

impl InternalDetail {
    const fn from_domain_validation(kind: DomainValidationKind) -> Self {
        let detail_kind = match kind {
            DomainValidationKind::InvalidCanonicalUuid => InternalDetailKind::InvalidCanonicalUuid,
            DomainValidationKind::InvalidUuidVersionOrVariant => {
                InternalDetailKind::InvalidUuidVersionOrVariant
            }
            DomainValidationKind::InvalidPositiveInteger => {
                InternalDetailKind::InvalidPositiveInteger
            }
            DomainValidationKind::ArithmeticOverflow => InternalDetailKind::ArithmeticOverflow,
            DomainValidationKind::ArithmeticUnderflow => InternalDetailKind::ArithmeticUnderflow,
            DomainValidationKind::InvalidCanonicalTimestamp => {
                InternalDetailKind::InvalidCanonicalTimestamp
            }
            DomainValidationKind::TimestampOutOfRange => InternalDetailKind::TimestampOutOfRange,
            DomainValidationKind::InvalidDigest => InternalDetailKind::InvalidDigest,
            DomainValidationKind::InvalidByteCount => InternalDetailKind::InvalidByteCount,
            DomainValidationKind::InvalidText => InternalDetailKind::InvalidText,
            DomainValidationKind::InvalidContent => InternalDetailKind::InvalidContent,
            DomainValidationKind::InvalidPrimaryConversation => {
                InternalDetailKind::InvalidPrimaryConversation
            }
            DomainValidationKind::InvalidMessageProvenance => {
                InternalDetailKind::InvalidMessageProvenance
            }
            DomainValidationKind::InvalidWorkInput => InternalDetailKind::InvalidWorkInput,
            DomainValidationKind::InvalidWorkstationGeneration => {
                InternalDetailKind::InvalidWorkstationGeneration
            }
            DomainValidationKind::InvalidLogicalPath => InternalDetailKind::InvalidLogicalPath,
            DomainValidationKind::InvalidBoundedIdentifier => {
                InternalDetailKind::InvalidBoundedIdentifier
            }
            DomainValidationKind::InvalidCapabilitySnapshot => {
                InternalDetailKind::InvalidCapabilitySnapshot
            }
            DomainValidationKind::InvalidEvidenceReference => {
                InternalDetailKind::InvalidEvidenceReference
            }
        };
        Self(detail_kind)
    }
}

/// A dependency-neutral, safe normalized failure classification.
///
/// Raw source failures are never retained. Internal trace detail has no public
/// accessor and is excluded from Display, Debug, serialization, and equality.
pub struct NormalizedError {
    category: ErrorCategory,
    code: ErrorCode,
    retryability: Retryability,
    certainty: Certainty,
    safe_message: SafeMessage,
    source_status: Option<SourceStatus>,
    internal_detail: Option<InternalDetail>,
}

impl NormalizedError {
    const fn generic(
        category: ErrorCategory,
        retryability: Retryability,
        certainty: Certainty,
        safe_message: SafeMessage,
        source_status: Option<SourceStatus>,
    ) -> Self {
        Self {
            category,
            code: ErrorCode::for_category(category),
            retryability,
            certainty,
            safe_message,
            source_status,
            internal_detail: None,
        }
    }

    /// Maps a precise local validation failure at a client input/protocol boundary.
    #[must_use]
    pub const fn from_client_validation(error: DomainValidationError) -> Self {
        Self {
            category: ErrorCategory::ClientProtocolError,
            code: ErrorCode::DOMAIN_VALIDATION,
            retryability: Retryability::Never,
            certainty: Certainty::Definite,
            safe_message: SafeMessage::CLIENT_VALIDATION,
            source_status: None,
            internal_detail: Some(InternalDetail::from_domain_validation(error.kind())),
        }
    }

    /// Constructs a generic authentication failure.
    #[must_use]
    pub const fn authentication() -> Self {
        Self::generic(
            ErrorCategory::AuthenticationError,
            Retryability::UserAction,
            Certainty::Definite,
            SafeMessage::AUTHENTICATION,
            None,
        )
    }

    /// Constructs a generic client-protocol failure.
    #[must_use]
    pub const fn client_protocol() -> Self {
        Self::generic(
            ErrorCategory::ClientProtocolError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::CLIENT_PROTOCOL,
            None,
        )
    }

    /// Constructs a generic idempotency failure.
    #[must_use]
    pub const fn idempotency() -> Self {
        Self::generic(
            ErrorCategory::IdempotencyError,
            Retryability::UserAction,
            Certainty::Definite,
            SafeMessage::IDEMPOTENCY,
            None,
        )
    }

    /// Constructs a generic definite storage failure.
    #[must_use]
    pub const fn storage(source_status: Option<SourceStatus>) -> Self {
        Self::generic(
            ErrorCategory::StorageError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::STORAGE,
            source_status,
        )
    }

    /// Constructs a generic definite state conflict.
    ///
    /// `bounded` permits later policy to re-evaluate state; it does not authorize replay.
    #[must_use]
    pub const fn state_conflict() -> Self {
        Self::generic(
            ErrorCategory::StateConflict,
            Retryability::Bounded,
            Certainty::Definite,
            SafeMessage::STATE_CONFLICT,
            None,
        )
    }

    /// Constructs a generic context failure.
    #[must_use]
    pub const fn context() -> Self {
        Self::generic(
            ErrorCategory::ContextError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::CONTEXT,
            None,
        )
    }

    /// Constructs the exact definite, nonretryable Stage 16 full-history limit failure.
    #[must_use]
    pub const fn context_limit_exceeded() -> Self {
        Self {
            category: ErrorCategory::ContextError,
            code: ErrorCode::CONTEXT_LIMIT_EXCEEDED,
            retryability: Retryability::Never,
            certainty: Certainty::Definite,
            safe_message: SafeMessage::CONTEXT,
            source_status: None,
            internal_detail: None,
        }
    }

    /// Constructs a generic model-selection failure.
    #[must_use]
    pub const fn model_selection() -> Self {
        Self::generic(
            ErrorCategory::ModelSelectionError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::MODEL_SELECTION,
            None,
        )
    }

    /// Constructs an unknown provider failure with conservative `never` retryability.
    ///
    /// The caller must choose certainty from the request-send boundary.
    #[must_use]
    pub const fn provider(certainty: Certainty, source_status: Option<SourceStatus>) -> Self {
        Self::generic(
            ErrorCategory::ProviderError,
            Retryability::Never,
            certainty,
            SafeMessage::PROVIDER,
            source_status,
        )
    }

    /// Constructs an explicitly classified bounded provider failure.
    ///
    /// This classification remains advisory and does not authorize replay.
    #[must_use]
    pub const fn provider_bounded(
        certainty: Certainty,
        source_status: Option<SourceStatus>,
    ) -> Self {
        Self::generic(
            ErrorCategory::ProviderError,
            Retryability::Bounded,
            certainty,
            SafeMessage::PROVIDER,
            source_status,
        )
    }

    /// Constructs a generic tool-validation failure.
    #[must_use]
    pub const fn tool_validation() -> Self {
        Self::generic(
            ErrorCategory::ToolValidationError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::TOOL_VALIDATION,
            None,
        )
    }

    /// Constructs a generic authority failure.
    #[must_use]
    pub const fn authority() -> Self {
        Self::generic(
            ErrorCategory::AuthorityError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::AUTHORITY,
            None,
        )
    }

    /// Constructs an unknown workstation failure with conservative `never` retryability.
    ///
    /// The caller must choose certainty from the dispatch boundary.
    #[must_use]
    pub const fn workstation(certainty: Certainty, source_status: Option<SourceStatus>) -> Self {
        Self::generic(
            ErrorCategory::WorkstationError,
            Retryability::Never,
            certainty,
            SafeMessage::WORKSTATION,
            source_status,
        )
    }

    /// Constructs an explicitly classified workstation failure.
    ///
    /// Retryability remains advisory and never authorizes an automatic replay.
    #[must_use]
    pub const fn workstation_classified(
        retryability: Retryability,
        certainty: Certainty,
        source_status: Option<SourceStatus>,
    ) -> Self {
        Self::generic(
            ErrorCategory::WorkstationError,
            retryability,
            certainty,
            SafeMessage::WORKSTATION,
            source_status,
        )
    }

    /// Constructs a generic artifact failure with caller-selected certainty.
    #[must_use]
    pub const fn artifact(certainty: Certainty, source_status: Option<SourceStatus>) -> Self {
        Self::generic(
            ErrorCategory::ArtifactError,
            Retryability::OperatorAction,
            certainty,
            SafeMessage::ARTIFACT,
            source_status,
        )
    }

    /// Constructs a generic cancellation failure with caller-selected certainty.
    #[must_use]
    pub const fn cancellation(certainty: Certainty) -> Self {
        Self::generic(
            ErrorCategory::CancellationError,
            Retryability::Never,
            certainty,
            SafeMessage::CANCELLATION,
            None,
        )
    }

    /// Constructs a trusted impossible/contradictory-state failure.
    #[must_use]
    pub const fn internal_invariant() -> Self {
        Self::generic(
            ErrorCategory::InternalInvariantError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::INTERNAL_INVARIANT,
            None,
        )
    }

    /// Returns the normalized category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    /// Returns the allowlisted stable code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the advisory retry classification.
    #[must_use]
    pub const fn retryability(&self) -> Retryability {
        self.retryability
    }

    /// Returns the outcome certainty classification.
    #[must_use]
    pub const fn certainty(&self) -> Certainty {
        self.certainty
    }

    /// Returns the approved safe message.
    #[must_use]
    pub const fn safe_message(&self) -> SafeMessage {
        self.safe_message
    }

    /// Returns the optional safe structured source status.
    #[must_use]
    pub const fn source_status(&self) -> Option<SourceStatus> {
        self.source_status
    }
}

impl fmt::Display for NormalizedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.safe_message, formatter)
    }
}

impl fmt::Debug for NormalizedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _internal_detail_is_deliberately_omitted =
            self.internal_detail.as_ref().map(|detail| &detail.0);
        formatter
            .debug_struct("NormalizedError")
            .field("category", &self.category)
            .field("code", &self.code)
            .field("retryability", &self.retryability)
            .field("certainty", &self.certainty)
            .field("safe_message", &self.safe_message)
            .field("source_status", &self.source_status)
            .finish()
    }
}

impl Serialize for NormalizedError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.source_status.is_some() { 6 } else { 5 };
        let mut state = serializer.serialize_struct("NormalizedError", field_count)?;
        state.serialize_field("category", &self.category)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("retryability", &self.retryability)?;
        state.serialize_field("certainty", &self.certainty)?;
        state.serialize_field("safe_message", &self.safe_message)?;
        if let Some(source_status) = self.source_status {
            state.serialize_field("source_status", &source_status)?;
        }
        state.end()
    }
}

impl PartialEq for NormalizedError {
    fn eq(&self, other: &Self) -> bool {
        self.category == other.category
            && self.code == other.code
            && self.retryability == other.retryability
            && self.certainty == other.certainty
            && self.safe_message == other.safe_message
            && self.source_status == other.source_status
    }
}

impl Eq for NormalizedError {}

impl std::error::Error for NormalizedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde::{Serialize, de::DeserializeOwned};

    use super::{
        Certainty, DomainValidationError, DomainValidationKind, ErrorCategory, ErrorCode,
        InternalDetail, InternalDetailKind, NormalizedError, Retryability, SafeMessage,
        SourceStatus,
    };
    use crate::domain::{MessageId, Sha256Digest, UtcTimestamp};

    const ALL_VALIDATION_KINDS: [DomainValidationKind; 19] = [
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

    fn assert_string_vocabulary<T>(values: &[(T, &str)])
    where
        T: Copy + DeserializeOwned + Eq + Serialize + std::fmt::Debug,
    {
        for (value, literal) in values {
            assert_eq!(
                serde_json::to_string(value).expect("serialize"),
                format!("\"{literal}\"")
            );
            assert!(literal.is_ascii());
            assert_eq!(
                serde_json::from_str::<T>(&format!("\"{literal}\""))
                    .expect("deserialize approved literal"),
                *value
            );
        }
    }

    #[test]
    fn category_retryability_and_certainty_vocabularies_are_exact() {
        let categories = [
            (ErrorCategory::AuthenticationError, "authentication_error"),
            (ErrorCategory::ClientProtocolError, "client_protocol_error"),
            (ErrorCategory::IdempotencyError, "idempotency_error"),
            (ErrorCategory::StorageError, "storage_error"),
            (ErrorCategory::StateConflict, "state_conflict"),
            (ErrorCategory::ContextError, "context_error"),
            (ErrorCategory::ModelSelectionError, "model_selection_error"),
            (ErrorCategory::ProviderError, "provider_error"),
            (ErrorCategory::ToolValidationError, "tool_validation_error"),
            (ErrorCategory::AuthorityError, "authority_error"),
            (ErrorCategory::WorkstationError, "workstation_error"),
            (ErrorCategory::ArtifactError, "artifact_error"),
            (ErrorCategory::CancellationError, "cancellation_error"),
            (
                ErrorCategory::InternalInvariantError,
                "internal_invariant_error",
            ),
        ];
        assert_eq!(categories.len(), 14);
        assert_string_vocabulary(&categories);
        for (category, literal) in categories {
            assert_eq!(category.as_str(), literal);
            assert_eq!(category.to_string(), literal);
        }
        assert!(serde_json::from_str::<ErrorCategory>("\"provider\"").is_err());
        assert!(serde_json::from_str::<ErrorCategory>("\"ProviderError\"").is_err());

        let retryability = [
            (Retryability::Never, "never"),
            (Retryability::Bounded, "bounded"),
            (Retryability::UserAction, "user_action"),
            (Retryability::OperatorAction, "operator_action"),
        ];
        assert_eq!(retryability.len(), 4);
        assert_string_vocabulary(&retryability);
        assert!(serde_json::from_str::<Retryability>("\"retry\"").is_err());

        let certainty = [
            (Certainty::Definite, "definite"),
            (Certainty::OutcomeUnknown, "outcome_unknown"),
        ];
        assert_eq!(certainty.len(), 2);
        assert_string_vocabulary(&certainty);
        assert!(serde_json::from_str::<Certainty>("\"unknown\"").is_err());
    }

    #[test]
    fn error_code_allowlist_is_exact_and_deserialization_is_closed() {
        let expected = [
            "domain_validation",
            "authentication_error",
            "client_protocol_error",
            "idempotency_error",
            "storage_error",
            "state_conflict",
            "context_error",
            "context_limit_exceeded",
            "model_selection_error",
            "provider_error",
            "tool_validation_error",
            "authority_error",
            "workstation_error",
            "artifact_error",
            "cancellation_error",
            "internal_invariant_error",
        ];

        assert_eq!(ErrorCode::ALL.len(), 16);
        for (code, literal) in ErrorCode::ALL.into_iter().zip(expected) {
            assert_eq!(code.as_str(), literal);
            assert!(literal.is_ascii());
            assert!(literal.len() <= 64);
            assert_eq!(
                serde_json::to_string(&code).expect("serialize"),
                format!("\"{literal}\"")
            );
            assert_eq!(
                serde_json::from_str::<ErrorCode>(&format!("\"{literal}\""))
                    .expect("approved code"),
                code
            );
        }

        for rejected in [
            "DOMAIN_VALIDATION",
            "Provider_Error",
            "arbitrary_snake_case",
            "raw_provider_native_code",
            "cancelled",
            "timeout",
        ] {
            assert!(
                serde_json::from_str::<ErrorCode>(&format!("\"{rejected}\"")).is_err(),
                "unexpectedly accepted {rejected}"
            );
        }
    }

    #[test]
    fn safe_message_allowlist_has_the_exact_fixed_v0_text() {
        let messages = [
            (
                SafeMessage::CLIENT_VALIDATION,
                "The supplied value is invalid.",
            ),
            (SafeMessage::AUTHENTICATION, "Authentication is required."),
            (SafeMessage::CLIENT_PROTOCOL, "The request is invalid."),
            (
                SafeMessage::IDEMPOTENCY,
                "The request conflicts with an earlier request.",
            ),
            (SafeMessage::STORAGE, "A storage operation failed."),
            (
                SafeMessage::STATE_CONFLICT,
                "The requested operation conflicts with the current state.",
            ),
            (
                SafeMessage::CONTEXT,
                "The requested context cannot be processed.",
            ),
            (
                SafeMessage::MODEL_SELECTION,
                "No suitable model is currently available.",
            ),
            (SafeMessage::PROVIDER, "The model provider request failed."),
            (SafeMessage::TOOL_VALIDATION, "The tool request is invalid."),
            (
                SafeMessage::AUTHORITY,
                "The requested operation is not permitted.",
            ),
            (
                SafeMessage::WORKSTATION,
                "The workstation operation failed.",
            ),
            (SafeMessage::ARTIFACT, "The artifact operation failed."),
            (
                SafeMessage::CANCELLATION,
                "The operation could not be confirmed as cancelled.",
            ),
            (
                SafeMessage::INTERNAL_INVARIANT,
                "An internal consistency error occurred.",
            ),
        ];

        assert_eq!(messages.len(), 15);
        for (message, expected) in messages {
            assert_eq!(message.as_str(), expected);
            assert_eq!(message.to_string(), expected);
            assert_eq!(
                serde_json::to_string(&message).expect("serialize"),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn source_status_bounds_shapes_and_golden_json_are_exact() {
        let http_100 = SourceStatus::provider_http(100).expect("lower bound");
        let http_599 = SourceStatus::provider_http(599).expect("upper bound");
        assert!(SourceStatus::provider_http(99).is_none());
        assert!(SourceStatus::provider_http(600).is_none());
        assert_eq!(http_100.kind(), "provider_http");
        assert_eq!(http_599.code(), 599);

        let errno_1 = SourceStatus::os_errno(1).expect("lower bound");
        let errno_max = SourceStatus::os_errno(i32::MAX).expect("upper bound");
        assert!(SourceStatus::os_errno(0).is_none());
        assert!(SourceStatus::os_errno(-1).is_none());
        assert_eq!(errno_1.kind(), "os_errno");
        assert_eq!(errno_max.code(), i32::MAX);

        assert_eq!(
            serde_json::to_string(&SourceStatus::provider_http(429).expect("valid"))
                .expect("serialize"),
            r#"{"kind":"provider_http","code":429}"#
        );
        assert_eq!(
            serde_json::to_string(&SourceStatus::os_errno(13).expect("valid")).expect("serialize"),
            r#"{"kind":"os_errno","code":13}"#
        );
        assert_eq!(
            serde_json::from_str::<SourceStatus>(r#"{"kind":"provider_http","code":100}"#)
                .expect("valid lower bound"),
            http_100
        );
        assert_eq!(
            serde_json::from_str::<SourceStatus>(&format!(
                r#"{{"kind":"os_errno","code":{}}}"#,
                i32::MAX
            ))
            .expect("valid upper bound"),
            errno_max
        );

        for rejected in [
            r#"{"kind":"provider_http","code":99}"#,
            r#"{"kind":"provider_http","code":600}"#,
            r#"{"kind":"os_errno","code":0}"#,
            r#"{"kind":"os_errno","code":-1}"#,
            r#"{"kind":"os_errno","code":2147483648}"#,
            r#"{"kind":"provider_native","code":429}"#,
            r#"{"kind":"PROVIDER_HTTP","code":429}"#,
            r#"{"kind":"provider_http","code":429,"body":"provider-body-sentinel"}"#,
            r#"{"kind":"os_errno","code":13,"path":"/private/path-sentinel"}"#,
            r#"{"kind":"provider_http","code":429,"reason":"Too Many Requests"}"#,
            r#"{"kind":"os_errno","code":13,"message":"permission denied"}"#,
            r#"{"kind":"provider_http","code":429,"http_status":503}"#,
            r#"{"kind":"os_errno","code":13,"exit_code":1}"#,
            r#"{"kind":"os_errno","code":13,"signal":9}"#,
        ] {
            assert!(
                serde_json::from_str::<SourceStatus>(rejected).is_err(),
                "unexpectedly accepted {rejected}"
            );
        }
    }

    #[test]
    fn code_and_closed_kinds_are_exact() {
        for kind in ALL_VALIDATION_KINDS {
            let error = DomainValidationError::new(kind);
            assert_eq!(error.code(), "domain_validation");
            assert_eq!(error.kind(), kind);
        }
    }

    #[test]
    fn every_precise_validation_kind_has_the_explicit_client_projection() {
        for kind in ALL_VALIDATION_KINDS {
            let local = DomainValidationError::new(kind);
            assert_eq!(local.kind(), kind);

            let normalized = NormalizedError::from_client_validation(local);
            assert_eq!(normalized.category(), ErrorCategory::ClientProtocolError);
            assert_eq!(normalized.code(), ErrorCode::DOMAIN_VALIDATION);
            assert_eq!(normalized.retryability(), Retryability::Never);
            assert_eq!(normalized.certainty(), Certainty::Definite);
            assert_eq!(normalized.safe_message(), SafeMessage::CLIENT_VALIDATION);
            assert_eq!(
                normalized.safe_message().as_str(),
                "The supplied value is invalid."
            );
            assert_eq!(normalized.source_status(), None);
            assert!(normalized.internal_detail.is_some());
        }
    }

    fn assert_generic(
        error: NormalizedError,
        category: ErrorCategory,
        retryability: Retryability,
        certainty: Certainty,
        safe_message: SafeMessage,
        source_status: Option<SourceStatus>,
    ) {
        assert_eq!(error.category(), category);
        assert_eq!(error.code().as_str(), category.as_str());
        assert_eq!(error.retryability(), retryability);
        assert_eq!(error.certainty(), certainty);
        assert_eq!(error.safe_message(), safe_message);
        assert_eq!(error.source_status(), source_status);
        assert!(error.internal_detail.is_none());
    }

    #[test]
    fn generic_category_constructors_have_exact_conservative_policy() {
        assert_generic(
            NormalizedError::authentication(),
            ErrorCategory::AuthenticationError,
            Retryability::UserAction,
            Certainty::Definite,
            SafeMessage::AUTHENTICATION,
            None,
        );
        assert_generic(
            NormalizedError::client_protocol(),
            ErrorCategory::ClientProtocolError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::CLIENT_PROTOCOL,
            None,
        );
        assert_generic(
            NormalizedError::idempotency(),
            ErrorCategory::IdempotencyError,
            Retryability::UserAction,
            Certainty::Definite,
            SafeMessage::IDEMPOTENCY,
            None,
        );
        let errno = Some(SourceStatus::os_errno(28).expect("valid errno"));
        assert_generic(
            NormalizedError::storage(errno),
            ErrorCategory::StorageError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::STORAGE,
            errno,
        );
        assert_generic(
            NormalizedError::state_conflict(),
            ErrorCategory::StateConflict,
            Retryability::Bounded,
            Certainty::Definite,
            SafeMessage::STATE_CONFLICT,
            None,
        );
        assert_generic(
            NormalizedError::context(),
            ErrorCategory::ContextError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::CONTEXT,
            None,
        );
        assert_generic(
            NormalizedError::model_selection(),
            ErrorCategory::ModelSelectionError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::MODEL_SELECTION,
            None,
        );
        assert_generic(
            NormalizedError::tool_validation(),
            ErrorCategory::ToolValidationError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::TOOL_VALIDATION,
            None,
        );
        assert_generic(
            NormalizedError::authority(),
            ErrorCategory::AuthorityError,
            Retryability::Never,
            Certainty::Definite,
            SafeMessage::AUTHORITY,
            None,
        );
        assert_generic(
            NormalizedError::internal_invariant(),
            ErrorCategory::InternalInvariantError,
            Retryability::OperatorAction,
            Certainty::Definite,
            SafeMessage::INTERNAL_INVARIANT,
            None,
        );
    }

    #[test]
    fn external_boundary_certainty_and_provider_retryability_are_explicit() {
        let provider_status = Some(SourceStatus::provider_http(503).expect("valid HTTP status"));
        for certainty in [Certainty::Definite, Certainty::OutcomeUnknown] {
            assert_generic(
                NormalizedError::provider(certainty, provider_status),
                ErrorCategory::ProviderError,
                Retryability::Never,
                certainty,
                SafeMessage::PROVIDER,
                provider_status,
            );
            assert_generic(
                NormalizedError::workstation(certainty, None),
                ErrorCategory::WorkstationError,
                Retryability::Never,
                certainty,
                SafeMessage::WORKSTATION,
                None,
            );
            assert_generic(
                NormalizedError::artifact(certainty, None),
                ErrorCategory::ArtifactError,
                Retryability::OperatorAction,
                certainty,
                SafeMessage::ARTIFACT,
                None,
            );
            assert_generic(
                NormalizedError::cancellation(certainty),
                ErrorCategory::CancellationError,
                Retryability::Never,
                certainty,
                SafeMessage::CANCELLATION,
                None,
            );
        }

        assert_generic(
            NormalizedError::provider_bounded(Certainty::Definite, provider_status),
            ErrorCategory::ProviderError,
            Retryability::Bounded,
            Certainty::Definite,
            SafeMessage::PROVIDER,
            provider_status,
        );

        let storage = NormalizedError::storage(None);
        let artifact = NormalizedError::artifact(Certainty::OutcomeUnknown, None);
        let invariant = NormalizedError::internal_invariant();
        assert_eq!(storage.category(), ErrorCategory::StorageError);
        assert_eq!(artifact.category(), ErrorCategory::ArtifactError);
        assert_ne!(storage.category(), invariant.category());
        assert_ne!(artifact.category(), invariant.category());
    }

    #[test]
    fn normalized_serialization_display_debug_and_source_are_strictly_safe() {
        let sentinels = [
            "Bearer-fake-token-sentinel-a1b2c3",
            "/private/absolute/path-sentinel-d4e5f6",
            "provider-body-sentinel-g7h8i9",
            "SELECT secret FROM credentials-sentinel-j1k2l3",
            "command-stdout-stderr-sentinel-m4n5o6",
            "user-text-sentinel-p7q8r9",
            "backtrace-sentinel-s1t2u3",
        ];
        let rejected = format!("rejected-{}", sentinels.join("-"));
        let local = rejected.parse::<MessageId>().expect_err("must reject");
        let error = NormalizedError::from_client_validation(local);

        let display = error.to_string();
        let debug = format!("{error:?}");
        let json = serde_json::to_string(&error).expect("serialize normalized error");

        assert_eq!(display, "The supplied value is invalid.");
        assert_eq!(
            json,
            r#"{"category":"client_protocol_error","code":"domain_validation","retryability":"never","certainty":"definite","safe_message":"The supplied value is invalid."}"#
        );
        assert!(debug.contains("client_protocol_error"));
        assert!(debug.contains("domain_validation"));
        assert!(debug.contains("The supplied value is invalid."));
        assert!(!debug.contains("internal_detail"));
        assert!(!json.contains("internal_detail"));
        assert!(!json.contains("source_status"));
        assert!(error.source().is_none());

        for sentinel in sentinels {
            assert!(rejected.contains(sentinel));
            assert!(!display.contains(sentinel));
            assert!(!debug.contains(sentinel));
            assert!(!json.contains(sentinel));
        }

        let provider = NormalizedError::provider(
            Certainty::OutcomeUnknown,
            Some(SourceStatus::provider_http(429).expect("valid status")),
        );
        assert_eq!(
            serde_json::to_string(&provider).expect("serialize provider error"),
            r#"{"category":"provider_error","code":"provider_error","retryability":"never","certainty":"outcome_unknown","safe_message":"The model provider request failed.","source_status":{"kind":"provider_http","code":429}}"#
        );
    }

    fn equality_fixture() -> NormalizedError {
        NormalizedError {
            category: ErrorCategory::ProviderError,
            code: ErrorCode::PROVIDER_ERROR,
            retryability: Retryability::Never,
            certainty: Certainty::Definite,
            safe_message: SafeMessage::PROVIDER,
            source_status: Some(SourceStatus::provider_http(429).expect("valid status")),
            internal_detail: None,
        }
    }

    #[test]
    fn semantic_equality_compares_every_safe_field_and_ignores_internal_detail() {
        assert_eq!(equality_fixture(), equality_fixture());

        let mut different = equality_fixture();
        different.category = ErrorCategory::WorkstationError;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.code = ErrorCode::WORKSTATION_ERROR;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.retryability = Retryability::Bounded;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.certainty = Certainty::OutcomeUnknown;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.safe_message = SafeMessage::WORKSTATION;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.source_status = None;
        assert_ne!(equality_fixture(), different);

        let mut different = equality_fixture();
        different.source_status = Some(SourceStatus::provider_http(500).expect("valid status"));
        assert_ne!(equality_fixture(), different);

        let first_detail = NormalizedError::from_client_validation(DomainValidationError::new(
            DomainValidationKind::InvalidCanonicalUuid,
        ));
        let second_detail = NormalizedError::from_client_validation(DomainValidationError::new(
            DomainValidationKind::InvalidDigest,
        ));
        assert!(matches!(
            first_detail.internal_detail,
            Some(InternalDetail(InternalDetailKind::InvalidCanonicalUuid))
        ));
        assert!(matches!(
            second_detail.internal_detail,
            Some(InternalDetail(InternalDetailKind::InvalidDigest))
        ));
        assert_eq!(first_detail, second_detail);
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
