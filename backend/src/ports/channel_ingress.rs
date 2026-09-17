//! One-intent provider-neutral inbound classification boundary.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::application::channel_ingress::{
    DurableInboundOutcome, InboundAdmissionDisposition, InboundAdmissionResult,
    VerifiedInboundPayload,
};
use crate::domain::{
    ChannelAccountId, ChannelDeliveryProfile, ChannelProviderId, ExternalConversationId,
    ExternalEventId, ExternalMessageId, ExternalSubjectId, ExternalThreadId, InboundDeliveryId,
    JournalEventId, MessageId, OutboundDeliveryId, Sha256Digest, UtcTimestamp, WorkId,
};

pub type ChannelIngressFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ClassifiedInbound, ChannelIngressStoreError>> + Send + 'a>>;
pub type ChannelProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ChannelProviderId, ChannelIngressStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelIngressStoreErrorKind {
    ContradictoryReplay,
    Storage,
    Inconsistent,
    Invalid,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChannelIngressStoreError {
    kind: ChannelIngressStoreErrorKind,
}

impl ChannelIngressStoreError {
    #[must_use]
    pub const fn new(kind: ChannelIngressStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> ChannelIngressStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for ChannelIngressStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ChannelIngressStoreErrorKind::ContradictoryReplay => {
                "channel ingress contradictory replay"
            }
            ChannelIngressStoreErrorKind::Storage => "channel ingress storage failure",
            ChannelIngressStoreErrorKind::Inconsistent => "channel ingress storage inconsistency",
            ChannelIngressStoreErrorKind::Invalid => "invalid verified inbound event",
        })
    }
}

impl fmt::Debug for ChannelIngressStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ChannelIngressStoreError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboundCandidates {
    pub inbound_delivery_id: InboundDeliveryId,
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub acceptance_event_id: JournalEventId,
    pub queued_event_id: JournalEventId,
    pub cancellation_event_id: JournalEventId,
    pub outbound_delivery_id: OutboundDeliveryId,
}

/// Complete normalized intent for one atomic durable classification.
pub struct ClassifyInboundRequest {
    pub channel_account_id: ChannelAccountId,
    pub external_event_id: ExternalEventId,
    pub external_message_id: Option<ExternalMessageId>,
    pub sender_subject_id: ExternalSubjectId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub payload: VerifiedInboundPayload,
    pub provider_occurred_at: Option<UtcTimestamp>,
    pub observed_at: UtcTimestamp,
    pub material_digest: Sha256Digest,
    pub is_control: bool,
    pub delivery_profile: Option<ChannelDeliveryProfile>,
    pub candidates: InboundCandidates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InboundPostCommitEffect {
    None,
    MessageCommitted {
        work_id: WorkId,
        cursor: crate::domain::JournalOffset,
    },
    ActiveCancellationCommitted {
        work_id: WorkId,
        cursor: crate::domain::JournalOffset,
        delivery_created: bool,
    },
    DirectCancellationCommitted {
        work_id: WorkId,
        cursor: crate::domain::JournalOffset,
        delivery_created: bool,
    },
    ControlAcknowledgementCommitted,
}

pub struct ClassifiedInbound {
    pub result: InboundAdmissionResult,
    pub(crate) effect: InboundPostCommitEffect,
}

impl ClassifiedInbound {
    pub(crate) const fn newly(
        outcome: DurableInboundOutcome,
        effect: InboundPostCommitEffect,
    ) -> Self {
        Self {
            result: InboundAdmissionResult {
                disposition: InboundAdmissionDisposition::NewlyClassified,
                outcome,
            },
            effect,
        }
    }

    pub(crate) const fn duplicate(outcome: DurableInboundOutcome) -> Self {
        Self {
            result: InboundAdmissionResult {
                disposition: InboundAdmissionDisposition::Duplicate,
                outcome,
            },
            effect: InboundPostCommitEffect::None,
        }
    }
}

/// The store owns dedupe, topology resolution, and exactly one atomic classification transaction.
pub trait ChannelIngressStore: Send + Sync {
    fn load_channel_provider_id(
        &self,
        channel_account_id: ChannelAccountId,
    ) -> ChannelProviderFuture<'_>;

    fn classify_inbound(&self, request: ClassifyInboundRequest) -> ChannelIngressFuture<'_>;
}
