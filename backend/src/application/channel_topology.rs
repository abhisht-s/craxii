//! Provider-neutral idempotent bootstrap of an exact channel account, owner identity, and route.

use std::fmt;
use std::sync::Arc;

use crate::domain::{
    ChannelAccountId, ChannelProviderId, ConversationBinding, ConversationBindingId,
    ConversationId, CraxiiId, ExternalAccountId, ExternalConversationId, ExternalIdentityId,
    ExternalSubjectId, UserId, UtcTimestamp,
};
use crate::ports::channel_identity::{
    ChannelIdentityStore, ChannelIdentityStoreError, ChannelIdentityStoreErrorKind,
    EnsureChannelAccountAndIdentityRequest, EnsureConversationBindingRequest,
    EnsuredChannelAccountAndIdentity,
};

pub struct ChannelTopologyService<S> {
    store: Arc<S>,
}

impl<S> ChannelTopologyService<S>
where
    S: ChannelIdentityStore,
{
    #[must_use]
    pub const fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn ensure_account_and_owner_identity(
        &self,
        channel_account_id: ChannelAccountId,
        craxii_id: CraxiiId,
        provider_id: ChannelProviderId,
        external_account_id: ExternalAccountId,
        owner_user_id: UserId,
        owner_external_subject_id: ExternalSubjectId,
        created_at: UtcTimestamp,
    ) -> Result<EnsuredChannelAccountAndIdentity, ChannelTopologyError> {
        self.store
            .ensure_channel_account_and_identity(EnsureChannelAccountAndIdentityRequest {
                channel_account_id,
                proposed_external_identity_id: ExternalIdentityId::generate(),
                craxii_id,
                provider_id,
                external_account_id,
                owner_user_id,
                owner_external_subject_id,
                created_at,
            })
            .await
            .map_err(map_store_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn ensure_first_binding(
        &self,
        channel_account_id: ChannelAccountId,
        external_identity_id: ExternalIdentityId,
        craxii_id: CraxiiId,
        user_id: UserId,
        conversation_id: ConversationId,
        external_conversation_id: ExternalConversationId,
        created_at: UtcTimestamp,
    ) -> Result<ConversationBinding, ChannelTopologyError> {
        self.store
            .ensure_first_conversation_binding(EnsureConversationBindingRequest {
                proposed_conversation_binding_id: ConversationBindingId::generate(),
                channel_account_id,
                external_identity_id,
                craxii_id,
                user_id,
                conversation_id,
                external_conversation_id,
                external_thread_id: None,
                created_at,
            })
            .await
            .map_err(map_store_error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelTopologyErrorKind {
    Conflict,
    Storage,
    Inconsistent,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ChannelTopologyError {
    kind: ChannelTopologyErrorKind,
}

impl ChannelTopologyError {
    #[must_use]
    pub const fn kind(self) -> ChannelTopologyErrorKind {
        self.kind
    }
}

impl fmt::Display for ChannelTopologyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            ChannelTopologyErrorKind::Conflict => "channel topology conflict",
            ChannelTopologyErrorKind::Storage => "channel topology storage failure",
            ChannelTopologyErrorKind::Inconsistent => "channel topology inconsistency",
        })
    }
}

impl fmt::Debug for ChannelTopologyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ChannelTopologyError {}

fn map_store_error(error: ChannelIdentityStoreError) -> ChannelTopologyError {
    ChannelTopologyError {
        kind: match error.kind() {
            ChannelIdentityStoreErrorKind::Conflict => ChannelTopologyErrorKind::Conflict,
            ChannelIdentityStoreErrorKind::Storage => ChannelTopologyErrorKind::Storage,
            ChannelIdentityStoreErrorKind::Inconsistent => ChannelTopologyErrorKind::Inconsistent,
        },
    }
}
