//! Provider-neutral durable outbound-delivery values.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    ChannelAccountId, ChannelProviderId, ConversationBindingId, CraxiiId, ExternalConversationId,
    ExternalMessageId, ExternalThreadId, InboundDeliveryId, MessageId, OutboundDeliveryAttemptId,
    OutboundDeliveryId, RuntimeInstanceId, Sha256Digest, UtcTimestamp, WorkId,
};

pub const MIN_DELIVERY_TEXT_UTF8_BYTES: usize = 64;
pub const MAX_DELIVERY_TEXT_UTF8_BYTES: usize = 196_606;
pub const MAX_DELIVERY_PARTS: u16 = 64;
pub const MAX_DELIVERY_FAILURE_CODE_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundDeliveryState {
    Queued,
    Dispatching,
    RetryWait,
    Accepted,
    PermanentFailure,
    OutcomeUnknown,
}

impl OutboundDeliveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Dispatching => "dispatching",
            Self::RetryWait => "retry_wait",
            Self::Accepted => "accepted",
            Self::PermanentFailure => "permanent_failure",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::PermanentFailure | Self::OutcomeUnknown
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAcknowledgementOutcome {
    Applied,
    NoOp,
}

impl ControlAcknowledgementOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NoOp => "no_op",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeliverySource {
    AssistantMessage {
        message_id: MessageId,
        work_id: WorkId,
    },
    ControlAcknowledgement {
        inbound_delivery_id: InboundDeliveryId,
        outcome: ControlAcknowledgementOutcome,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFailureClass {
    AdapterUnavailable,
    BindingRevoked,
    ChannelAccountDisabled,
    UnsupportedPayload,
    ProfileUnavailable,
    PayloadTooLarge,
    PriorPartPermanentFailure,
    PriorPartOutcomeUnknown,
    RetryExhausted,
    DeliveryDeadlineExceeded,
    ProviderRetryable,
    ProviderPermanent,
    ProviderOutcomeUnknown,
    StaleDispatch,
    ShutdownInterrupted,
    StorageInconsistent,
}

impl DeliveryFailureClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdapterUnavailable => "adapter_unavailable",
            Self::BindingRevoked => "binding_revoked",
            Self::ChannelAccountDisabled => "channel_account_disabled",
            Self::UnsupportedPayload => "unsupported_payload",
            Self::ProfileUnavailable => "profile_unavailable",
            Self::PayloadTooLarge => "payload_too_large",
            Self::PriorPartPermanentFailure => "prior_part_permanent_failure",
            Self::PriorPartOutcomeUnknown => "prior_part_outcome_unknown",
            Self::RetryExhausted => "retry_exhausted",
            Self::DeliveryDeadlineExceeded => "delivery_deadline_exceeded",
            Self::ProviderRetryable => "provider_retryable",
            Self::ProviderPermanent => "provider_permanent",
            Self::ProviderOutcomeUnknown => "provider_outcome_unknown",
            Self::StaleDispatch => "stale_dispatch",
            Self::ShutdownInterrupted => "shutdown_interrupted",
            Self::StorageInconsistent => "storage_inconsistent",
        }
    }
}

/// A bounded, sanitized adapter code. It is evidence, never a raw provider error.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct DeliveryFailureCode(String);

impl DeliveryFailureCode {
    pub fn try_new(value: impl Into<String>) -> Result<Self, DeliveryValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_DELIVERY_FAILURE_CODE_BYTES
            || !value.is_ascii()
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(DeliveryValidationError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeliveryFailureCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DeliveryFailureCode")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryFailure {
    class: DeliveryFailureClass,
    code: Option<DeliveryFailureCode>,
}

impl DeliveryFailure {
    #[must_use]
    pub const fn classified(class: DeliveryFailureClass) -> Self {
        Self { class, code: None }
    }

    pub fn provider(
        class: DeliveryFailureClass,
        code: Option<DeliveryFailureCode>,
    ) -> Result<Self, DeliveryValidationError> {
        if !matches!(
            class,
            DeliveryFailureClass::ProviderRetryable
                | DeliveryFailureClass::ProviderPermanent
                | DeliveryFailureClass::ProviderOutcomeUnknown
        ) {
            return Err(DeliveryValidationError);
        }
        Ok(Self { class, code })
    }

    #[must_use]
    pub const fn class(&self) -> DeliveryFailureClass {
        self.class
    }

    #[must_use]
    pub const fn code(&self) -> Option<&DeliveryFailureCode> {
        self.code.as_ref()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ChannelDeliveryProfile {
    provider_id: ChannelProviderId,
    max_text_utf8_bytes: usize,
    max_parts: u16,
}

impl ChannelDeliveryProfile {
    pub fn try_new(
        provider_id: ChannelProviderId,
        max_text_utf8_bytes: usize,
        max_parts: u16,
    ) -> Result<Self, DeliveryValidationError> {
        if !(MIN_DELIVERY_TEXT_UTF8_BYTES..=MAX_DELIVERY_TEXT_UTF8_BYTES)
            .contains(&max_text_utf8_bytes)
            || !(1..=MAX_DELIVERY_PARTS).contains(&max_parts)
        {
            return Err(DeliveryValidationError);
        }
        Ok(Self {
            provider_id,
            max_text_utf8_bytes,
            max_parts,
        })
    }

    #[must_use]
    pub const fn provider_id(&self) -> &ChannelProviderId {
        &self.provider_id
    }

    #[must_use]
    pub const fn max_text_utf8_bytes(&self) -> usize {
        self.max_text_utf8_bytes
    }

    #[must_use]
    pub const fn max_parts(&self) -> u16 {
        self.max_parts
    }
}

impl fmt::Debug for ChannelDeliveryProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelDeliveryProfile")
            .field("provider_id", &self.provider_id)
            .field("max_text_utf8_bytes", &self.max_text_utf8_bytes)
            .field("max_parts", &self.max_parts)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OutboundDelivery {
    pub outbound_delivery_id: OutboundDeliveryId,
    pub craxii_id: CraxiiId,
    pub conversation_binding_id: ConversationBindingId,
    pub channel_account_id: ChannelAccountId,
    pub provider_id: ChannelProviderId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub source: DeliverySource,
    pub payload_text: String,
    pub payload_sha256: Sha256Digest,
    pub part_ordinal: u16,
    pub part_count: u16,
    pub state: OutboundDeliveryState,
    pub attempt_count: u16,
    pub delivery_deadline_at: UtcTimestamp,
    pub failure: Option<DeliveryFailure>,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
}

impl fmt::Debug for OutboundDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundDelivery")
            .field("outbound_delivery_id", &self.outbound_delivery_id)
            .field("craxii_id", &self.craxii_id)
            .field("conversation_binding_id", &self.conversation_binding_id)
            .field("channel_account_id", &self.channel_account_id)
            .field("provider_id", &self.provider_id)
            .field("source", &self.source)
            .field("payload_sha256", &self.payload_sha256)
            .field("part_ordinal", &self.part_ordinal)
            .field("part_count", &self.part_count)
            .field("state", &self.state)
            .field("attempt_count", &self.attempt_count)
            .field("delivery_deadline_at", &self.delivery_deadline_at)
            .field("failure", &self.failure)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("routing_and_text", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundDeliveryAttempt {
    pub outbound_delivery_attempt_id: OutboundDeliveryAttemptId,
    pub outbound_delivery_id: OutboundDeliveryId,
    pub runtime_instance_id: RuntimeInstanceId,
    pub attempt_number: u16,
    pub prior_state: OutboundDeliveryState,
    pub dispatch_material_sha256: Sha256Digest,
    pub started_at: UtcTimestamp,
}

#[derive(Clone, Eq, PartialEq)]
pub struct PreparedChannelDispatch {
    pub outbound_delivery_id: OutboundDeliveryId,
    pub outbound_delivery_attempt_id: OutboundDeliveryAttemptId,
    pub attempt_number: u16,
    pub channel_account_id: ChannelAccountId,
    pub provider_id: ChannelProviderId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub text: String,
    pub payload_sha256: Sha256Digest,
    pub dispatch_material_sha256: Sha256Digest,
    pub part_ordinal: u16,
    pub part_count: u16,
}

impl fmt::Debug for PreparedChannelDispatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedChannelDispatch")
            .field("outbound_delivery_id", &self.outbound_delivery_id)
            .field(
                "outbound_delivery_attempt_id",
                &self.outbound_delivery_attempt_id,
            )
            .field("attempt_number", &self.attempt_number)
            .field("channel_account_id", &self.channel_account_id)
            .field("provider_id", &self.provider_id)
            .field("payload_sha256", &self.payload_sha256)
            .field("dispatch_material_sha256", &self.dispatch_material_sha256)
            .field("part_ordinal", &self.part_ordinal)
            .field("part_count", &self.part_count)
            .field("routing_and_text", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ChannelDispatchResult {
    Accepted {
        external_message_id: Option<ExternalMessageId>,
    },
    RetryableFailure {
        failure: DeliveryFailure,
        retry_after: Option<Duration>,
    },
    PermanentFailure {
        failure: DeliveryFailure,
    },
    OutcomeUnknown {
        failure: DeliveryFailure,
    },
}

impl fmt::Debug for ChannelDispatchResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted {
                external_message_id,
            } => formatter
                .debug_struct("Accepted")
                .field(
                    "external_message_id_present",
                    &external_message_id.is_some(),
                )
                .finish(),
            Self::RetryableFailure {
                failure,
                retry_after,
            } => formatter
                .debug_struct("RetryableFailure")
                .field("failure", failure)
                .field("retry_after", retry_after)
                .finish(),
            Self::PermanentFailure { failure } => formatter
                .debug_struct("PermanentFailure")
                .field("failure", failure)
                .finish(),
            Self::OutcomeUnknown { failure } => formatter
                .debug_struct("OutcomeUnknown")
                .field("failure", failure)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryValidationError;

impl fmt::Display for DeliveryValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid delivery value")
    }
}

impl std::error::Error for DeliveryValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_failure_codes_are_strict() {
        let provider = ChannelProviderId::try_new("fake.provider").unwrap();
        assert!(ChannelDeliveryProfile::try_new(provider.clone(), 64, 1).is_ok());
        assert!(ChannelDeliveryProfile::try_new(provider.clone(), 63, 1).is_err());
        assert!(ChannelDeliveryProfile::try_new(provider, 64, 65).is_err());
        assert!(DeliveryFailureCode::try_new("rate_limited-1.0").is_ok());
        for rejected in ["", "UPPER", "has space", "slash/value"] {
            assert!(DeliveryFailureCode::try_new(rejected).is_err());
        }
    }

    #[test]
    fn prepared_dispatch_debug_redacts_sensitive_material() {
        let dispatch = PreparedChannelDispatch {
            outbound_delivery_id: OutboundDeliveryId::generate(),
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            attempt_number: 1,
            channel_account_id: ChannelAccountId::generate(),
            provider_id: ChannelProviderId::try_new("fake").unwrap(),
            external_conversation_id: ExternalConversationId::try_new("private-destination")
                .unwrap(),
            external_thread_id: Some(ExternalThreadId::try_new("private-thread").unwrap()),
            text: "private-text".into(),
            payload_sha256: Sha256Digest::hash_bytes(b"private-text"),
            dispatch_material_sha256: Sha256Digest::hash_bytes(b"material"),
            part_ordinal: 1,
            part_count: 1,
        };
        let debug = format!("{dispatch:?}");
        assert!(!debug.contains("private-destination"));
        assert!(!debug.contains("private-thread"));
        assert!(!debug.contains("private-text"));

        let accepted = ChannelDispatchResult::Accepted {
            external_message_id: Some(ExternalMessageId::try_new("private-provider-id").unwrap()),
        };
        let debug = format!("{accepted:?}");
        assert!(!debug.contains("private-provider-id"));
    }
}
