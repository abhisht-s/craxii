use std::fmt;
use std::sync::Arc;

use crate::domain::{
    ChannelAccountId, ChannelDeliveryProfile, ChannelDispatchResult, ChannelProviderId,
    DeliveryFailure, DeliveryFailureClass, DeliveryFailureCode, PreparedChannelDispatch,
};
use crate::ports::channel_delivery::{ChannelDeliveryAdapter, ChannelDeliveryFuture};

use super::TELEGRAM_PROVIDER_KEY;
use super::client::TelegramClient;

pub const TELEGRAM_MAX_TEXT_UTF8_BYTES: usize = 4096;
pub const TELEGRAM_MAX_PARTS: u16 = 64;
const MAX_TELEGRAM_ID: u64 = (1_u64 << 52) - 1;

pub fn telegram_delivery_profile() -> ChannelDeliveryProfile {
    ChannelDeliveryProfile::try_new(
        ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY).expect("fixed provider is valid"),
        TELEGRAM_MAX_TEXT_UTF8_BYTES,
        TELEGRAM_MAX_PARTS,
    )
    .expect("fixed Telegram delivery profile is valid")
}

pub struct TelegramDeliveryAdapter {
    provider_id: ChannelProviderId,
    channel_account_id: ChannelAccountId,
    client: Arc<TelegramClient>,
}

impl TelegramDeliveryAdapter {
    #[must_use]
    pub fn new(channel_account_id: ChannelAccountId, client: Arc<TelegramClient>) -> Self {
        Self {
            provider_id: ChannelProviderId::try_new(TELEGRAM_PROVIDER_KEY)
                .expect("fixed provider is valid"),
            channel_account_id,
            client,
        }
    }
}

impl ChannelDeliveryAdapter for TelegramDeliveryAdapter {
    fn provider_id(&self) -> &ChannelProviderId {
        &self.provider_id
    }

    fn dispatch(&self, dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_> {
        Box::pin(async move {
            if dispatch.provider_id != self.provider_id {
                return permanent("telegram_wrong_provider");
            }
            if dispatch.channel_account_id != self.channel_account_id {
                return permanent("telegram_unknown_account");
            }
            if dispatch.external_thread_id.is_some() {
                return permanent("telegram_thread_unsupported");
            }
            if dispatch.text.is_empty()
                || dispatch.text.len() > TELEGRAM_MAX_TEXT_UTF8_BYTES
                || crate::domain::Sha256Digest::hash_bytes(dispatch.text.as_bytes())
                    != dispatch.payload_sha256
            {
                return permanent("telegram_invalid_payload");
            }
            let route = dispatch.external_conversation_id.as_str();
            let Ok(chat_id) = route.parse::<i64>() else {
                return permanent("telegram_invalid_route");
            };
            if chat_id == 0
                || chat_id.unsigned_abs() > MAX_TELEGRAM_ID
                || chat_id.to_string() != route
            {
                return permanent("telegram_invalid_route");
            }
            self.client.send_message(chat_id, &dispatch.text).await
        })
    }
}

impl fmt::Debug for TelegramDeliveryAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelegramDeliveryAdapter")
            .field("provider_id", &self.provider_id)
            .field("channel_account_id", &self.channel_account_id)
            .field("client", &"[REDACTED]")
            .finish()
    }
}

fn permanent(code: &'static str) -> ChannelDispatchResult {
    ChannelDispatchResult::PermanentFailure {
        failure: DeliveryFailure::provider(
            DeliveryFailureClass::ProviderPermanent,
            Some(DeliveryFailureCode::try_new(code).expect("fixed code is valid")),
        )
        .expect("provider failure class is valid"),
    }
}
