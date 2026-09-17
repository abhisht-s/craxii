//! Generic admission of an already-authenticated provider-neutral inbound event.

use std::fmt;
use std::sync::Arc;

use crate::application::command_service::CommandPostCommit;
use crate::application::control_message_policy::ControlMessagePolicy;
use crate::application::delivery_planner::ChannelDeliveryProfileRegistry;
use crate::application::delivery_worker::DeliveryNotifier;
use crate::application::transport::MutationAdmission;
use crate::bootstrap::health::{Health, HealthState};
use crate::domain::{
    ChannelAccountId, ExternalConversationId, ExternalEventId, ExternalMessageId,
    ExternalSubjectId, ExternalThreadId, InboundDeliveryId, JournalEventId, JournalOffset,
    MessageContent, MessageId, OutboundDeliveryId, Sha256Digest, UtcTimestamp, WorkId,
};
use crate::ports::channel_ingress::{
    ChannelIngressStore, ChannelIngressStoreError, ChannelIngressStoreErrorKind,
    ClassifyInboundRequest, InboundCandidates, InboundPostCommitEffect,
};

const INBOUND_MATERIAL_MAGIC: &[u8] = b"craxii.inbound-material";
const INBOUND_MATERIAL_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedInboundKind {
    NonText,
    Edit,
    Reaction,
    Other,
}

impl UnsupportedInboundKind {
    const fn tag(self) -> u8 {
        match self {
            Self::NonText => 1,
            Self::Edit => 2,
            Self::Reaction => 3,
            Self::Other => 4,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum VerifiedInboundPayload {
    Text(MessageContent),
    Unsupported(UnsupportedInboundKind),
}

impl fmt::Debug for VerifiedInboundPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(content) => formatter
                .debug_struct("Text")
                .field("content_sha256", &content.content_sha256())
                .field("text_bytes", &content.total_text_bytes())
                .finish(),
            Self::Unsupported(kind) => formatter.debug_tuple("Unsupported").field(kind).finish(),
        }
    }
}

/// Authenticated normalized evidence. Only trusted in-crate adapters may construct it.
#[derive(Clone)]
pub struct VerifiedInboundEvent {
    channel_account_id: ChannelAccountId,
    external_event_id: ExternalEventId,
    external_message_id: Option<ExternalMessageId>,
    sender_subject_id: ExternalSubjectId,
    external_conversation_id: ExternalConversationId,
    external_thread_id: Option<ExternalThreadId>,
    payload: VerifiedInboundPayload,
    provider_occurred_at: Option<UtcTimestamp>,
    observed_at: UtcTimestamp,
}

impl VerifiedInboundEvent {
    #[allow(dead_code)] // CH-2 deliberately leaves trusted provider adapters uncomposed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        channel_account_id: ChannelAccountId,
        external_event_id: ExternalEventId,
        external_message_id: Option<ExternalMessageId>,
        sender_subject_id: ExternalSubjectId,
        external_conversation_id: ExternalConversationId,
        external_thread_id: Option<ExternalThreadId>,
        payload: VerifiedInboundPayload,
        provider_occurred_at: Option<UtcTimestamp>,
        observed_at: UtcTimestamp,
    ) -> Result<Self, ChannelIngressError> {
        let event = Self {
            channel_account_id,
            external_event_id,
            external_message_id,
            sender_subject_id,
            external_conversation_id,
            external_thread_id,
            payload,
            provider_occurred_at,
            observed_at,
        };
        event.validate()?;
        Ok(event)
    }

    fn validate(&self) -> Result<(), ChannelIngressError> {
        if matches!(self.payload, VerifiedInboundPayload::Text(_))
            && self.external_message_id.is_none()
        {
            Err(ChannelIngressError::new(
                ChannelIngressErrorKind::InvalidVerifiedEvent,
            ))
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub const fn channel_account_id(&self) -> ChannelAccountId {
        self.channel_account_id
    }

    #[must_use]
    pub const fn external_event_id(&self) -> &ExternalEventId {
        &self.external_event_id
    }

    #[must_use]
    pub const fn external_message_id(&self) -> Option<&ExternalMessageId> {
        self.external_message_id.as_ref()
    }

    #[must_use]
    pub const fn sender_subject_id(&self) -> &ExternalSubjectId {
        &self.sender_subject_id
    }

    #[must_use]
    pub const fn external_conversation_id(&self) -> &ExternalConversationId {
        &self.external_conversation_id
    }

    #[must_use]
    pub const fn external_thread_id(&self) -> Option<&ExternalThreadId> {
        self.external_thread_id.as_ref()
    }

    #[must_use]
    pub const fn payload(&self) -> &VerifiedInboundPayload {
        &self.payload
    }

    #[must_use]
    pub const fn provider_occurred_at(&self) -> Option<UtcTimestamp> {
        self.provider_occurred_at
    }

    #[must_use]
    pub const fn observed_at(&self) -> UtcTimestamp {
        self.observed_at
    }

    /// Computes the central V1 digest over normalized material only.
    #[must_use]
    pub fn material_digest(&self) -> Sha256Digest {
        fn framed(bytes: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            output.extend_from_slice(bytes);
        }
        fn optional(value: Option<&str>, output: &mut Vec<u8>) {
            match value {
                Some(value) => {
                    output.push(1);
                    framed(value.as_bytes(), output);
                }
                None => output.push(0),
            }
        }

        let mut material = Vec::with_capacity(256);
        material.extend_from_slice(INBOUND_MATERIAL_MAGIC);
        material.push(INBOUND_MATERIAL_VERSION);
        framed(
            self.channel_account_id.to_string().as_bytes(),
            &mut material,
        );
        framed(self.sender_subject_id.as_str().as_bytes(), &mut material);
        framed(
            self.external_conversation_id.as_str().as_bytes(),
            &mut material,
        );
        optional(
            self.external_thread_id
                .as_ref()
                .map(ExternalThreadId::as_str),
            &mut material,
        );
        optional(
            self.external_message_id
                .as_ref()
                .map(ExternalMessageId::as_str),
            &mut material,
        );
        match &self.payload {
            VerifiedInboundPayload::Text(content) => {
                material.push(1);
                framed(&content.canonical_bytes(), &mut material);
            }
            VerifiedInboundPayload::Unsupported(kind) => {
                material.push(2);
                material.push(kind.tag());
            }
        }
        match self.provider_occurred_at {
            Some(timestamp) => {
                material.push(1);
                framed(timestamp.to_string().as_bytes(), &mut material);
            }
            None => material.push(0),
        }
        Sha256Digest::hash_bytes(&material)
    }
}

impl fmt::Debug for VerifiedInboundEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedInboundEvent")
            .field("channel_account_id", &self.channel_account_id)
            .field("external_identifiers", &"[REDACTED]")
            .field("payload", &self.payload)
            .field("provider_occurred_at", &self.provider_occurred_at)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InboundAdmissionDisposition {
    NewlyClassified,
    Duplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableInboundOutcome {
    MessageAccepted {
        inbound_delivery_id: InboundDeliveryId,
        message_id: MessageId,
        work_id: WorkId,
        work_ordinal: crate::domain::ConversationWorkOrdinal,
        committed_cursor: JournalOffset,
    },
    ControlApplied {
        inbound_delivery_id: InboundDeliveryId,
        target_work_id: WorkId,
    },
    ControlNoOp {
        inbound_delivery_id: InboundDeliveryId,
    },
    Unsupported {
        inbound_delivery_id: InboundDeliveryId,
    },
    Rejected {
        inbound_delivery_id: InboundDeliveryId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboundAdmissionResult {
    pub disposition: InboundAdmissionDisposition,
    pub outcome: DurableInboundOutcome,
}

impl InboundAdmissionResult {
    /// Results exist only after terminal durable classification.
    #[must_use]
    pub const fn acknowledgement_safe(self) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelIngressErrorKind {
    Unavailable,
    InvalidVerifiedEvent,
    ContradictoryReplay,
    StorageFailure,
    StorageInconsistent,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChannelIngressError {
    kind: ChannelIngressErrorKind,
}

impl ChannelIngressError {
    const fn new(kind: ChannelIngressErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> ChannelIngressErrorKind {
        self.kind
    }

    /// Errors never authorize provider acknowledgement.
    #[must_use]
    pub const fn acknowledgement_safe(self) -> bool {
        false
    }
}

impl fmt::Display for ChannelIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ChannelIngressErrorKind::Unavailable => "channel ingress unavailable",
            ChannelIngressErrorKind::InvalidVerifiedEvent => "invalid verified inbound event",
            ChannelIngressErrorKind::ContradictoryReplay => "contradictory inbound replay",
            ChannelIngressErrorKind::StorageFailure => "channel ingress storage failure",
            ChannelIngressErrorKind::StorageInconsistent => "channel ingress storage inconsistency",
        })
    }
}

impl fmt::Debug for ChannelIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ChannelIngressError {}

pub struct ChannelIngressService<S, H> {
    store: Arc<S>,
    health: Health,
    admission: MutationAdmission,
    post_commit: H,
    delivery_profiles: Arc<ChannelDeliveryProfileRegistry>,
    delivery_notifier: DeliveryNotifier,
}

impl<S, H> ChannelIngressService<S, H>
where
    S: ChannelIngressStore,
    H: CommandPostCommit,
{
    #[must_use]
    pub fn new(
        store: Arc<S>,
        health: Health,
        admission: MutationAdmission,
        post_commit: H,
    ) -> Self {
        Self {
            store,
            health,
            admission,
            post_commit,
            delivery_profiles: Arc::new(ChannelDeliveryProfileRegistry::default()),
            delivery_notifier: DeliveryNotifier::new(),
        }
    }

    #[must_use]
    pub fn with_delivery(
        mut self,
        profiles: Arc<ChannelDeliveryProfileRegistry>,
        notifier: DeliveryNotifier,
    ) -> Self {
        self.delivery_profiles = profiles;
        self.delivery_notifier = notifier;
        self
    }

    pub async fn classify(
        &self,
        event: VerifiedInboundEvent,
    ) -> Result<InboundAdmissionResult, ChannelIngressError> {
        event.validate()?;
        let is_control = match event.payload() {
            VerifiedInboundPayload::Text(content) if content.blocks().len() == 1 => {
                ControlMessagePolicy::recognize(content.blocks()[0].as_text()).is_some()
            }
            _ => false,
        };
        let health_allowed = if is_control {
            matches!(
                self.health.snapshot().state(),
                HealthState::LiveUnready | HealthState::Ready
            )
        } else {
            self.health.snapshot().is_ingress_admission_ready()
        };
        if !health_allowed {
            return Err(ChannelIngressError::new(
                ChannelIngressErrorKind::Unavailable,
            ));
        }
        let _permit = self
            .admission
            .admit()
            .await
            .map_err(|_| ChannelIngressError::new(ChannelIngressErrorKind::Unavailable))?;
        let material_digest = event.material_digest();
        let delivery_profile = if is_control {
            let provider_id = self
                .store
                .load_channel_provider_id(event.channel_account_id)
                .await
                .map_err(map_store_error)?;
            self.delivery_profiles.profile(&provider_id).cloned()
        } else {
            None
        };
        let classified = self
            .store
            .classify_inbound(ClassifyInboundRequest {
                channel_account_id: event.channel_account_id,
                external_event_id: event.external_event_id,
                external_message_id: event.external_message_id,
                sender_subject_id: event.sender_subject_id,
                external_conversation_id: event.external_conversation_id,
                external_thread_id: event.external_thread_id,
                payload: event.payload,
                provider_occurred_at: event.provider_occurred_at,
                observed_at: event.observed_at,
                material_digest,
                is_control,
                delivery_profile,
                candidates: InboundCandidates {
                    inbound_delivery_id: InboundDeliveryId::generate(),
                    message_id: MessageId::generate(),
                    work_id: WorkId::generate(),
                    acceptance_event_id: JournalEventId::generate(),
                    queued_event_id: JournalEventId::generate(),
                    cancellation_event_id: JournalEventId::generate(),
                    outbound_delivery_id: OutboundDeliveryId::generate(),
                },
            })
            .await
            .map_err(map_store_error)?;
        match classified.effect {
            InboundPostCommitEffect::None => {}
            InboundPostCommitEffect::MessageCommitted { work_id, cursor } => {
                self.post_commit.message_committed(work_id, cursor);
            }
            InboundPostCommitEffect::ActiveCancellationCommitted {
                work_id,
                cursor,
                delivery_created,
            } => {
                self.post_commit
                    .active_cancellation_committed(work_id, cursor);
                if delivery_created {
                    self.delivery_notifier.wake();
                }
            }
            InboundPostCommitEffect::DirectCancellationCommitted {
                work_id,
                cursor,
                delivery_created,
            } => {
                self.post_commit
                    .direct_cancellation_committed(work_id, cursor);
                if delivery_created {
                    self.delivery_notifier.wake();
                }
            }
            InboundPostCommitEffect::ControlAcknowledgementCommitted => {
                self.delivery_notifier.wake()
            }
        }
        Ok(classified.result)
    }
}

fn map_store_error(error: ChannelIngressStoreError) -> ChannelIngressError {
    ChannelIngressError::new(match error.kind() {
        ChannelIngressStoreErrorKind::ContradictoryReplay => {
            ChannelIngressErrorKind::ContradictoryReplay
        }
        ChannelIngressStoreErrorKind::Storage => ChannelIngressErrorKind::StorageFailure,
        ChannelIngressStoreErrorKind::Inconsistent => ChannelIngressErrorKind::StorageInconsistent,
        ChannelIngressStoreErrorKind::Invalid => ChannelIngressErrorKind::InvalidVerifiedEvent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::command_service::NoopCommandPostCommit;
    use crate::domain::{ContentBlock, ExternalEventId};
    use crate::ports::channel_ingress::{
        ChannelIngressFuture, ChannelIngressStoreError, ChannelIngressStoreErrorKind,
    };

    struct FailingStore;

    impl ChannelIngressStore for FailingStore {
        fn load_channel_provider_id(
            &self,
            _: ChannelAccountId,
        ) -> crate::ports::channel_ingress::ChannelProviderFuture<'_> {
            Box::pin(async {
                Err(ChannelIngressStoreError::new(
                    ChannelIngressStoreErrorKind::Storage,
                ))
            })
        }

        fn classify_inbound(&self, _request: ClassifyInboundRequest) -> ChannelIngressFuture<'_> {
            Box::pin(async {
                Err(ChannelIngressStoreError::new(
                    ChannelIngressStoreErrorKind::Storage,
                ))
            })
        }
    }

    fn event(event_id: &str, text: &str) -> VerifiedInboundEvent {
        VerifiedInboundEvent::new(
            "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d".parse().unwrap(),
            ExternalEventId::try_new(event_id).unwrap(),
            Some(ExternalMessageId::try_new("message").unwrap()),
            ExternalSubjectId::try_new("subject").unwrap(),
            ExternalConversationId::try_new("conversation").unwrap(),
            None,
            VerifiedInboundPayload::Text(
                MessageContent::try_new(vec![ContentBlock::text(text).unwrap()]).unwrap(),
            ),
            None,
            "2026-09-16T01:02:03.000000Z".parse().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn digest_excludes_event_id_and_observed_time_but_preserves_exact_text() {
        let first = event("event-a", "é");
        let mut second = event("event-b", "é");
        second.observed_at = "2026-09-16T02:03:04.000000Z".parse().unwrap();
        assert_eq!(first.material_digest(), second.material_digest());

        second.payload = VerifiedInboundPayload::Text(
            MessageContent::try_new(vec![ContentBlock::text("e\u{301}").unwrap()]).unwrap(),
        );
        assert_ne!(first.material_digest(), second.material_digest());
    }

    #[test]
    fn digest_golden_and_optional_fields_are_collision_separated() {
        let base = event("event", " stop ");
        assert_eq!(
            base.material_digest().to_string(),
            "a496bbcf40d6f5ff2b4650bfef02c77d08863d5f01f4928950ef81aaf84c4ff6"
        );

        let mut thread = base.clone();
        thread.external_thread_id = Some(ExternalThreadId::try_new("thread").unwrap());
        let mut no_message = base.clone();
        no_message.external_message_id = None;
        let mut occurred = base.clone();
        occurred.provider_occurred_at = Some("2026-09-16T00:00:00.000000Z".parse().unwrap());
        let mut different_lengths = base.clone();
        different_lengths.sender_subject_id = ExternalSubjectId::try_new("subjec").unwrap();
        different_lengths.external_conversation_id =
            ExternalConversationId::try_new("tconversation").unwrap();
        assert_ne!(base.material_digest(), thread.material_digest());
        assert_ne!(base.material_digest(), no_message.material_digest());
        assert_ne!(base.material_digest(), occurred.material_digest());
        assert_ne!(base.material_digest(), different_lengths.material_digest());

        let mut unsupported = base.clone();
        unsupported.external_message_id = None;
        unsupported.payload = VerifiedInboundPayload::Unsupported(UnsupportedInboundKind::Edit);
        assert_eq!(
            unsupported.material_digest().to_string(),
            "436e3ffde9172cce36b9bbdeff607e0b03e9ac530b9c8781a602ab0e25a8948c"
        );
        let mut other_unsupported = unsupported.clone();
        other_unsupported.payload =
            VerifiedInboundPayload::Unsupported(UnsupportedInboundKind::Reaction);
        assert_ne!(
            unsupported.material_digest(),
            other_unsupported.material_digest()
        );
    }

    #[test]
    fn verified_event_requires_message_identity_for_text_and_redacts_debug() {
        let result = VerifiedInboundEvent::new(
            ChannelAccountId::generate(),
            ExternalEventId::try_new("private-event").unwrap(),
            None,
            ExternalSubjectId::try_new("private-subject").unwrap(),
            ExternalConversationId::try_new("private-destination").unwrap(),
            None,
            VerifiedInboundPayload::Text(
                MessageContent::try_new(vec![ContentBlock::text("private-content").unwrap()])
                    .unwrap(),
            ),
            None,
            "2026-09-16T01:02:03.000000Z".parse().unwrap(),
        );
        assert_eq!(
            result.unwrap_err().kind(),
            ChannelIngressErrorKind::InvalidVerifiedEvent
        );
        let debug = format!("{:?}", event("private-event", "private-content"));
        assert!(!debug.contains("private-event"));
        assert!(!debug.contains("private-content"));
    }

    #[tokio::test]
    async fn storage_failure_is_not_acknowledgement_safe() {
        let health = Health::new();
        health.mark_ready().unwrap();
        let service = ChannelIngressService::new(
            Arc::new(FailingStore),
            health,
            MutationAdmission::new(),
            NoopCommandPostCommit,
        );
        let error = service.classify(event("event", "hello")).await.unwrap_err();
        assert_eq!(error.kind(), ChannelIngressErrorKind::StorageFailure);
        assert!(!error.acknowledgement_safe());
        assert_eq!(format!("{error}"), "channel ingress storage failure");
        assert_eq!(format!("{error:?}"), "channel ingress storage failure");
    }

    #[tokio::test]
    async fn ordinary_ingress_requires_internal_admission_not_public_readiness() {
        let health = Health::new();
        let service = ChannelIngressService::new(
            Arc::new(FailingStore),
            health.clone(),
            MutationAdmission::new(),
            NoopCommandPostCommit,
        );

        let unavailable = service
            .classify(event("before", "hello"))
            .await
            .unwrap_err();
        assert_eq!(unavailable.kind(), ChannelIngressErrorKind::Unavailable);

        health.mark_ingress_admission_ready().unwrap();
        assert!(!health.snapshot().is_ready());
        let admitted = service.classify(event("after", "hello")).await.unwrap_err();
        assert_eq!(admitted.kind(), ChannelIngressErrorKind::StorageFailure);

        let control = service
            .classify(event("control", "/cancel"))
            .await
            .unwrap_err();
        assert_eq!(control.kind(), ChannelIngressErrorKind::StorageFailure);
    }
}
