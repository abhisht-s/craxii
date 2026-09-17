//! Provider-neutral persistence boundary for channel identity and inbound evidence.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::domain::{
    ChannelAccount, ChannelAccountId, ChannelProviderId, ConversationBinding,
    ConversationBindingId, ConversationId, CraxiiId, ExternalAccountId, ExternalConversationId,
    ExternalIdentity, ExternalIdentityId, ExternalSubjectId, ExternalThreadId, InboundDelivery,
    InboundDeliveryId, UserId, UtcTimestamp,
};

pub type ChannelIdentityFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ChannelIdentityStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelIdentityStoreErrorKind {
    Storage,
    Conflict,
    Inconsistent,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChannelIdentityStoreError {
    kind: ChannelIdentityStoreErrorKind,
}

pub struct EnsureChannelAccountAndIdentityRequest {
    pub channel_account_id: ChannelAccountId,
    pub proposed_external_identity_id: ExternalIdentityId,
    pub craxii_id: CraxiiId,
    pub provider_id: ChannelProviderId,
    pub external_account_id: ExternalAccountId,
    pub owner_user_id: UserId,
    pub owner_external_subject_id: ExternalSubjectId,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnsuredChannelAccountAndIdentity {
    pub channel_account_id: ChannelAccountId,
    pub external_identity_id: ExternalIdentityId,
}

pub struct EnsureConversationBindingRequest {
    pub proposed_conversation_binding_id: ConversationBindingId,
    pub channel_account_id: ChannelAccountId,
    pub external_identity_id: ExternalIdentityId,
    pub craxii_id: CraxiiId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub external_conversation_id: ExternalConversationId,
    pub external_thread_id: Option<ExternalThreadId>,
    pub created_at: UtcTimestamp,
}

impl ChannelIdentityStoreError {
    #[must_use]
    pub const fn new(kind: ChannelIdentityStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> ChannelIdentityStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for ChannelIdentityStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ChannelIdentityStoreErrorKind::Storage => "channel identity storage failure",
            ChannelIdentityStoreErrorKind::Conflict => "channel identity conflict",
            ChannelIdentityStoreErrorKind::Inconsistent => "channel identity inconsistency",
        })
    }
}

impl fmt::Debug for ChannelIdentityStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ChannelIdentityStoreError {}

pub trait ChannelIdentityStore: Send + Sync {
    fn ensure_channel_account_and_identity(
        &self,
        request: EnsureChannelAccountAndIdentityRequest,
    ) -> ChannelIdentityFuture<'_, EnsuredChannelAccountAndIdentity>;

    fn ensure_first_conversation_binding(
        &self,
        request: EnsureConversationBindingRequest,
    ) -> ChannelIdentityFuture<'_, ConversationBinding>;

    fn persist_channel_account(&self, account: ChannelAccount) -> ChannelIdentityFuture<'_, ()>;

    fn load_channel_account(
        &self,
        id: ChannelAccountId,
    ) -> ChannelIdentityFuture<'_, Option<ChannelAccount>>;

    fn persist_external_identity(
        &self,
        identity: ExternalIdentity,
    ) -> ChannelIdentityFuture<'_, ()>;

    fn load_external_identity(
        &self,
        id: ExternalIdentityId,
    ) -> ChannelIdentityFuture<'_, Option<ExternalIdentity>>;

    fn persist_conversation_binding(
        &self,
        binding: ConversationBinding,
    ) -> ChannelIdentityFuture<'_, ()>;

    fn load_conversation_binding(
        &self,
        id: ConversationBindingId,
    ) -> ChannelIdentityFuture<'_, Option<ConversationBinding>>;

    fn persist_inbound_delivery(&self, delivery: InboundDelivery) -> ChannelIdentityFuture<'_, ()>;

    fn load_inbound_delivery(
        &self,
        id: InboundDeliveryId,
    ) -> ChannelIdentityFuture<'_, Option<InboundDelivery>>;
}
