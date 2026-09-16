//! Provider-neutral human identity and channel persistence foundations.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::{
    ChannelAccountId, ConversationBindingId, ConversationId, CraxiiId, DomainValidationError,
    DomainValidationKind, ExternalIdentityId, InboundDeliveryId, MessageId, Sha256Digest, UserId,
    UtcTimestamp, WorkId,
};

/// Maximum byte length of an open channel-provider key.
pub const MAX_CHANNEL_PROVIDER_ID_BYTES: usize = 64;
/// Maximum byte length of every opaque provider-owned identifier.
pub const MAX_EXTERNAL_ID_BYTES: usize = 255;

macro_rules! external_identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Preserves an exact, trimmed, bounded UTF-8 provider value.
            pub fn try_new(value: impl Into<String>) -> Result<Self, DomainValidationError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_EXTERNAL_ID_BYTES
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(DomainValidationError::new(
                        DomainValidationKind::InvalidBoundedIdentifier,
                    ));
                }
                Ok(Self(value))
            }

            /// Returns the exact value for routing and persistence only.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&"[REDACTED]")
                    .finish()
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::try_new(value).map_err(de::Error::custom)
            }
        }
    };
}

/// An open, adapter-registered provider key such as a future channel name.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChannelProviderId(String);

impl ChannelProviderId {
    /// Validates 1..=64 ASCII bytes using the open `[a-z0-9._-]` grammar.
    pub fn try_new(value: impl Into<String>) -> Result<Self, DomainValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CHANNEL_PROVIDER_ID_BYTES
            || !value.is_ascii()
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidBoundedIdentifier,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ChannelProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ChannelProviderId")
            .field(&self.0)
            .finish()
    }
}

impl Serialize for ChannelProviderId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ChannelProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

external_identifier!(
    ExternalAccountId,
    "An opaque external channel-account identifier."
);
external_identifier!(
    ExternalSubjectId,
    "An opaque external human subject identifier."
);
external_identifier!(
    ExternalConversationId,
    "An opaque external conversation/destination identifier."
);
external_identifier!(ExternalThreadId, "An opaque external thread identifier.");
external_identifier!(ExternalEventId, "An opaque external event identifier.");
external_identifier!(ExternalMessageId, "An opaque external message identifier.");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserLifecycle {
    Active,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAccountLifecycle {
    Active,
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalIdentityLifecycle {
    Active,
    Revoked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBindingLifecycle {
    Active,
    Revoked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundReceiptState {
    Received,
    Classified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundClassification {
    Message,
    Control,
    Rejected,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub user_id: UserId,
    pub craxii_id: CraxiiId,
    pub lifecycle: UserLifecycle,
    pub created_at: UtcTimestamp,
}

impl User {
    #[must_use]
    pub const fn active(user_id: UserId, craxii_id: CraxiiId, created_at: UtcTimestamp) -> Self {
        Self {
            user_id,
            craxii_id,
            lifecycle: UserLifecycle::Active,
            created_at,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelAccount {
    pub channel_account_id: ChannelAccountId,
    pub craxii_id: CraxiiId,
    pub provider_id: ChannelProviderId,
    pub external_account_id: ExternalAccountId,
    pub lifecycle: ChannelAccountLifecycle,
    pub created_at: UtcTimestamp,
    pub disabled_at: Option<UtcTimestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalIdentity {
    pub external_identity_id: ExternalIdentityId,
    pub channel_account_id: ChannelAccountId,
    pub craxii_id: CraxiiId,
    pub user_id: UserId,
    pub external_subject_id: ExternalSubjectId,
    pub lifecycle: ExternalIdentityLifecycle,
    pub created_at: UtcTimestamp,
    pub revoked_at: Option<UtcTimestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationBinding {
    pub conversation_binding_id: ConversationBindingId,
    pub channel_account_id: ChannelAccountId,
    pub external_identity_id: ExternalIdentityId,
    pub craxii_id: CraxiiId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub lifecycle: ConversationBindingLifecycle,
    pub created_at: UtcTimestamp,
    pub revoked_at: Option<UtcTimestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InboundDelivery {
    pub inbound_delivery_id: InboundDeliveryId,
    pub channel_account_id: ChannelAccountId,
    pub craxii_id: CraxiiId,
    pub external_event_id: ExternalEventId,
    pub external_message_id: Option<ExternalMessageId>,
    pub external_subject_id: ExternalSubjectId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub material_sha256: Sha256Digest,
    pub provider_occurred_at: Option<UtcTimestamp>,
    pub received_at: UtcTimestamp,
    pub receipt_state: InboundReceiptState,
    pub classification: Option<InboundClassification>,
    pub external_identity_id: Option<ExternalIdentityId>,
    pub conversation_binding_id: Option<ConversationBindingId>,
    pub user_id: Option<UserId>,
    pub conversation_id: Option<ConversationId>,
    pub message_id: Option<MessageId>,
    pub work_id: Option<WorkId>,
    pub control_target_work_id: Option<WorkId>,
    pub classified_at: Option<UtcTimestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_key_is_open_but_strict() {
        assert_eq!(
            ChannelProviderId::try_new("future.channel-1")
                .unwrap()
                .as_str(),
            "future.channel-1"
        );
        for rejected in ["", "Upper", "with space", "slash/value"] {
            assert!(ChannelProviderId::try_new(rejected).is_err());
        }
    }

    #[test]
    fn external_identifiers_preserve_exact_value_and_redact_debug() {
        let value = ExternalSubjectId::try_new("opaque 用户 42").unwrap();
        assert_eq!(value.as_str(), "opaque 用户 42");
        assert_eq!(format!("{value:?}"), "ExternalSubjectId(\"[REDACTED]\")");
        for rejected in ["", " leading", "trailing ", "control\nvalue"] {
            assert!(ExternalSubjectId::try_new(rejected).is_err());
        }
    }
}
