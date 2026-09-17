//! Deterministic V1 outbound text planning and immutable provider profiles.

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::{
    ChannelDeliveryProfile, ChannelProviderId, ControlAcknowledgementOutcome, DeliveryFailure,
    DeliveryFailureClass, DeliverySource, Message, MessageContent, OutboundDelivery,
    OutboundDeliveryId, OutboundDeliveryState, Sha256Digest, UtcTimestamp,
};
use crate::ports::delivery_store::DeliveryRoute;

pub const CONTROL_ACK_APPLIED: &str = "Cancellation requested.";
pub const CONTROL_ACK_NO_OP: &str = "Nothing is currently running or queued.";
pub const DELIVERY_DEADLINE_SECONDS: i64 = 60 * 60;

#[derive(Clone, Default)]
pub struct ChannelDeliveryProfileRegistry {
    profiles: BTreeMap<ChannelProviderId, ChannelDeliveryProfile>,
}

impl ChannelDeliveryProfileRegistry {
    pub fn try_new(
        profiles: impl IntoIterator<Item = ChannelDeliveryProfile>,
    ) -> Result<Self, DeliveryPlannerError> {
        let mut registered = BTreeMap::new();
        for profile in profiles {
            if registered
                .insert(profile.provider_id().clone(), profile)
                .is_some()
            {
                return Err(DeliveryPlannerError::DuplicateProvider);
            }
        }
        Ok(Self {
            profiles: registered,
        })
    }

    #[must_use]
    pub fn profile(&self, provider_id: &ChannelProviderId) -> Option<&ChannelDeliveryProfile> {
        self.profiles.get(provider_id)
    }

    #[must_use]
    pub fn contains(&self, provider_id: &ChannelProviderId) -> bool {
        self.profiles.contains_key(provider_id)
    }
}

impl fmt::Debug for ChannelDeliveryProfileRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelDeliveryProfileRegistry")
            .field("provider_count", &self.profiles.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryPlannerError {
    DuplicateProvider,
    InvalidTime,
    EmptyPayload,
}

impl fmt::Display for DeliveryPlannerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateProvider => "duplicate channel delivery profile",
            Self::InvalidTime => "delivery planning timestamp is invalid",
            Self::EmptyPayload => "delivery payload is empty",
        })
    }
}

impl std::error::Error for DeliveryPlannerError {}

/// Renders only canonical text blocks, preserving their bytes and order exactly.
pub fn render_assistant_v1(content: &MessageContent) -> Result<String, DeliveryPlannerError> {
    let mut rendered = String::new();
    for (index, block) in content.blocks().iter().enumerate() {
        if index != 0 {
            rendered.push_str("\n\n");
        }
        rendered.push_str(block.as_text());
    }
    if rendered.is_empty() {
        Err(DeliveryPlannerError::EmptyPayload)
    } else {
        Ok(rendered)
    }
}

#[must_use]
pub const fn render_control_acknowledgement(
    outcome: ControlAcknowledgementOutcome,
) -> &'static str {
    match outcome {
        ControlAcknowledgementOutcome::Applied => CONTROL_ACK_APPLIED,
        ControlAcknowledgementOutcome::NoOp => CONTROL_ACK_NO_OP,
    }
}

/// Splits exact UTF-8 bytes with deterministic paragraph/newline/scalar preference.
pub fn split_text_v1(
    text: &str,
    profile: &ChannelDeliveryProfile,
) -> Result<Vec<String>, DeliveryFailureClass> {
    if text.is_empty() {
        return Err(DeliveryFailureClass::UnsupportedPayload);
    }
    let limit = profile.max_text_utf8_bytes();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    while start < text.len() {
        let remaining = &text[start..];
        if remaining.len() <= limit {
            parts.push(remaining.to_owned());
            break;
        }

        let mut boundary = start + limit;
        while boundary > start && !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        if boundary == start {
            return Err(DeliveryFailureClass::PayloadTooLarge);
        }
        let prefix = &text[start..boundary];
        let split = prefix
            .rfind("\n\n")
            .map(|index| start + index + 2)
            .or_else(|| prefix.rfind('\n').map(|index| start + index + 1))
            .unwrap_or(boundary);
        if split <= start {
            return Err(DeliveryFailureClass::PayloadTooLarge);
        }
        parts.push(text[start..split].to_owned());
        if parts.len() >= usize::from(profile.max_parts()) {
            return Err(DeliveryFailureClass::PayloadTooLarge);
        }
        start = split;
    }
    if parts.is_empty()
        || parts.len() > usize::from(profile.max_parts())
        || parts.len() > 64
        || parts.iter().any(String::is_empty)
        || parts.concat().as_bytes() != text.as_bytes()
    {
        return Err(DeliveryFailureClass::PayloadTooLarge);
    }
    Ok(parts)
}

pub fn plan_assistant_delivery(
    message: &Message,
    route: &DeliveryRoute,
    profiles: &ChannelDeliveryProfileRegistry,
) -> Result<Vec<OutboundDelivery>, DeliveryPlannerError> {
    let rendered = render_assistant_v1(message.content())?;
    plan_rendered(
        DeliverySource::AssistantMessage {
            message_id: message.message_id(),
            work_id: message
                .produced_by_work_id()
                .ok_or(DeliveryPlannerError::EmptyPayload)?,
        },
        rendered,
        route,
        profiles,
        message.committed_at(),
    )
}

pub fn plan_control_acknowledgement(
    source_inbound_delivery_id: crate::domain::InboundDeliveryId,
    outcome: ControlAcknowledgementOutcome,
    route: &DeliveryRoute,
    profiles: &ChannelDeliveryProfileRegistry,
    created_at: UtcTimestamp,
) -> Result<Vec<OutboundDelivery>, DeliveryPlannerError> {
    plan_rendered(
        DeliverySource::ControlAcknowledgement {
            inbound_delivery_id: source_inbound_delivery_id,
            outcome,
        },
        render_control_acknowledgement(outcome).to_owned(),
        route,
        profiles,
        created_at,
    )
}

fn plan_rendered(
    source: DeliverySource,
    rendered: String,
    route: &DeliveryRoute,
    profiles: &ChannelDeliveryProfileRegistry,
    created_at: UtcTimestamp,
) -> Result<Vec<OutboundDelivery>, DeliveryPlannerError> {
    let deadline = UtcTimestamp::from_offset_datetime(
        created_at
            .to_offset_datetime()
            .checked_add(time::Duration::seconds(DELIVERY_DEADLINE_SECONDS))
            .ok_or(DeliveryPlannerError::InvalidTime)?,
    )
    .map_err(|_| DeliveryPlannerError::InvalidTime)?;

    let route_failure = if !route.binding_active {
        Some(DeliveryFailureClass::BindingRevoked)
    } else if !route.account_active {
        Some(DeliveryFailureClass::ChannelAccountDisabled)
    } else {
        None
    };
    let (parts, planning_failure) = match profiles.profile(&route.provider_id) {
        Some(profile) => match split_text_v1(&rendered, profile) {
            Ok(parts) => (parts, None),
            Err(failure) => (vec![rendered], Some(failure)),
        },
        None => (
            vec![rendered],
            Some(DeliveryFailureClass::ProfileUnavailable),
        ),
    };
    let failure = route_failure.or(planning_failure);
    let part_count = u16::try_from(parts.len()).map_err(|_| DeliveryPlannerError::EmptyPayload)?;
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(index, payload_text)| OutboundDelivery {
            outbound_delivery_id: OutboundDeliveryId::generate(),
            craxii_id: route.craxii_id,
            conversation_binding_id: route.conversation_binding_id,
            channel_account_id: route.channel_account_id,
            provider_id: route.provider_id.clone(),
            external_conversation_id: route.external_conversation_id.clone(),
            external_thread_id: route.external_thread_id.clone(),
            source,
            payload_sha256: Sha256Digest::hash_bytes(payload_text.as_bytes()),
            payload_text,
            part_ordinal: u16::try_from(index + 1).expect("part count is bounded"),
            part_count,
            state: if failure.is_some() {
                OutboundDeliveryState::PermanentFailure
            } else {
                OutboundDeliveryState::Queued
            },
            attempt_count: 0,
            delivery_deadline_at: deadline,
            failure: failure.map(DeliveryFailure::classified),
            created_at,
            updated_at: created_at,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContentBlock, MessageContent};

    fn profile(limit: usize, parts: u16) -> ChannelDeliveryProfile {
        ChannelDeliveryProfile::try_new(ChannelProviderId::try_new("fake").unwrap(), limit, parts)
            .unwrap()
    }

    #[test]
    fn assistant_rendering_and_control_strings_are_byte_exact() {
        let content = MessageContent::try_new(vec![
            ContentBlock::text("  first\n").unwrap(),
            ContentBlock::text("é\u{301} last  ").unwrap(),
        ])
        .unwrap();
        assert_eq!(
            render_assistant_v1(&content).unwrap(),
            "  first\n\n\né\u{301} last  "
        );
        assert_eq!(
            render_control_acknowledgement(ControlAcknowledgementOutcome::Applied),
            "Cancellation requested."
        );
        assert_eq!(
            render_control_acknowledgement(ControlAcknowledgementOutcome::NoOp),
            "Nothing is currently running or queued."
        );
    }

    #[test]
    fn splitting_prefers_paragraph_then_newline_then_scalar_and_reassembles_exactly() {
        let paragraph = "a".repeat(60) + "\n\n" + &"b".repeat(10);
        let parts = split_text_v1(&paragraph, &profile(64, 4)).unwrap();
        assert_eq!(parts[0], "a".repeat(60) + "\n\n");
        assert_eq!(parts.concat(), paragraph);

        let newline = "a".repeat(62) + "\n" + &"b".repeat(10);
        let parts = split_text_v1(&newline, &profile(64, 4)).unwrap();
        assert!(parts[0].ends_with('\n'));
        assert_eq!(parts.concat(), newline);

        let unicode = "é".repeat(40);
        let parts = split_text_v1(&unicode, &profile(65, 4)).unwrap();
        assert!(parts.iter().all(|part| part.is_char_boundary(part.len())));
        assert_eq!(parts.concat().as_bytes(), unicode.as_bytes());
        assert!(parts.iter().all(|part| part.len() <= 65));
    }

    #[test]
    fn split_rejects_more_than_profile_part_budget() {
        assert_eq!(
            split_text_v1(&"x".repeat(129), &profile(64, 2)),
            Err(DeliveryFailureClass::PayloadTooLarge)
        );
    }

    #[test]
    fn registry_rejects_duplicate_providers() {
        assert!(
            ChannelDeliveryProfileRegistry::try_new([profile(64, 1), profile(128, 2)]).is_err()
        );
    }
}
