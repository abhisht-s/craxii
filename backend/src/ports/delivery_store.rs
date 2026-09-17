//! Narrow durable outbound-delivery persistence boundary.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::domain::{
    ChannelAccountId, ChannelDispatchResult, ChannelProviderId, ConversationBindingId, CraxiiId,
    DeliveryFailureClass, DeliverySource, ExternalConversationId, ExternalThreadId,
    OutboundDeliveryAttemptId, OutboundDeliveryId, OutboundDeliveryState, PreparedChannelDispatch,
    RuntimeInstanceId, Sha256Digest, UtcTimestamp, WorkId,
};

pub type DeliveryStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DeliveryStoreError>> + Send + 'a>>;

#[derive(Clone, Eq, PartialEq)]
pub struct DeliveryRoute {
    pub craxii_id: CraxiiId,
    pub conversation_binding_id: ConversationBindingId,
    pub channel_account_id: ChannelAccountId,
    pub provider_id: ChannelProviderId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub binding_active: bool,
    pub account_active: bool,
}

impl fmt::Debug for DeliveryRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryRoute")
            .field("craxii_id", &self.craxii_id)
            .field("conversation_binding_id", &self.conversation_binding_id)
            .field("channel_account_id", &self.channel_account_id)
            .field("provider_id", &self.provider_id)
            .field("binding_active", &self.binding_active)
            .field("account_active", &self.account_active)
            .field("external_destination", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadDeliveryRouteRequest {
    pub work_id: WorkId,
    pub conversation_binding_id: ConversationBindingId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimDeliveryRequest {
    pub runtime_instance_id: RuntimeInstanceId,
    pub outbound_delivery_attempt_id: OutboundDeliveryAttemptId,
    pub now: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryClaim {
    Dispatch(Box<PreparedChannelDispatch>),
    StateAdvanced,
    NoneDue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistDispatchResultRequest {
    pub dispatch: Box<PreparedChannelDispatch>,
    pub runtime_instance_id: RuntimeInstanceId,
    pub result: ChannelDispatchResult,
    pub local_retry_delay: Option<Duration>,
    pub completed_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistDispatchResultDisposition {
    Applied,
    Idempotent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverDeliveriesRequest {
    pub recovered_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeliveryRecoveryReceipt {
    pub dispatches_marked_unknown: u64,
    pub later_parts_blocked: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownDeliveryRequest {
    pub runtime_instance_id: RuntimeInstanceId,
    pub interrupted_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListDeliverySummariesRequest {
    pub states: Vec<OutboundDeliveryState>,
    pub after: Option<OutboundDeliveryId>,
    pub limit: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliverySummary {
    pub outbound_delivery_id: OutboundDeliveryId,
    pub source: DeliverySource,
    pub channel_account_id: ChannelAccountId,
    pub provider_id: ChannelProviderId,
    pub part_ordinal: u16,
    pub part_count: u16,
    pub state: OutboundDeliveryState,
    pub attempt_count: u16,
    pub created_at: UtcTimestamp,
    pub updated_at: UtcTimestamp,
    pub next_attempt_at: Option<UtcTimestamp>,
    pub delivery_deadline_at: UtcTimestamp,
    pub terminal_at: Option<UtcTimestamp>,
    pub payload_sha256: Sha256Digest,
    pub failure_class: Option<DeliveryFailureClass>,
    pub failure_code: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryStoreErrorKind {
    Storage,
    Inconsistent,
    Invalid,
    StateConflict,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DeliveryStoreError {
    kind: DeliveryStoreErrorKind,
}

impl DeliveryStoreError {
    #[must_use]
    pub const fn new(kind: DeliveryStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> DeliveryStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for DeliveryStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            DeliveryStoreErrorKind::Storage => "delivery storage failure",
            DeliveryStoreErrorKind::Inconsistent => "delivery storage inconsistency",
            DeliveryStoreErrorKind::Invalid => "invalid delivery request",
            DeliveryStoreErrorKind::StateConflict => "delivery state conflict",
        })
    }
}

impl fmt::Debug for DeliveryStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for DeliveryStoreError {}

pub trait DeliveryStore: Send + Sync {
    fn load_delivery_route(
        &self,
        request: LoadDeliveryRouteRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRoute>;

    fn claim_next_delivery(
        &self,
        request: ClaimDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryClaim>;

    fn persist_dispatch_result(
        &self,
        request: PersistDispatchResultRequest,
    ) -> DeliveryStoreFuture<'_, PersistDispatchResultDisposition>;

    fn recover_stale_deliveries(
        &self,
        request: RecoverDeliveriesRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt>;

    fn interrupt_owned_delivery(
        &self,
        request: ShutdownDeliveryRequest,
    ) -> DeliveryStoreFuture<'_, DeliveryRecoveryReceipt>;

    fn list_delivery_summaries(
        &self,
        request: ListDeliverySummariesRequest,
    ) -> DeliveryStoreFuture<'_, Vec<DeliverySummary>>;

    fn verify_delivery_consistency(&self) -> DeliveryStoreFuture<'_, u64>;
}
