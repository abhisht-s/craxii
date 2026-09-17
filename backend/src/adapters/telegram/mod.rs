//! Telegram Bot API owner adapter. Provider wire and transport details remain in this module.

mod client;
mod delivery;
mod polling;
mod wire;

pub use client::{TelegramClient, TelegramClientError, TelegramClientErrorKind, TelegramPollBatch};
pub use delivery::{TelegramDeliveryAdapter, telegram_delivery_profile};
pub use polling::{
    TelegramInboundProcessor, TelegramLongPollingDriverHandle, TelegramOwnerTopology,
    TelegramPollerError, TelegramPollerErrorKind,
};
pub(crate) use polling::{
    TelegramJitterSource, TelegramStartupFailureBudget, start_long_polling, startup_probe,
    verify_startup_identity,
};

pub const TELEGRAM_PROVIDER_KEY: &str = "telegram";

#[cfg(test)]
mod tests;
