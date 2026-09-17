use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use serde_json::Value;
use tracing::instrument::WithSubscriber as _;

use crate::application::channel_ingress::{
    ChannelIngressErrorKind, ChannelIngressService, UnsupportedInboundKind, VerifiedInboundEvent,
    VerifiedInboundPayload,
};
use crate::application::channel_topology::{ChannelTopologyErrorKind, ChannelTopologyService};
use crate::application::command_service::CommandPostCommit;
use crate::bootstrap::health::{FatalReasonCode, Health};
use crate::domain::{
    ChannelAccountId, ContentBlock, ConversationId, CraxiiId, ExternalConversationId,
    ExternalEventId, ExternalIdentityId, ExternalMessageId, ExternalSubjectId, MessageContent,
    UserId, UtcTimestamp,
};
use crate::ports::channel_identity::ChannelIdentityStore;
use crate::ports::channel_ingress::ChannelIngressStore;
use crate::ports::clock::Clock;

use super::TELEGRAM_PROVIDER_KEY;
use super::client::{
    TelegramClient, TelegramClientError, TelegramClientErrorKind, TelegramPollBatch,
};
use super::wire::{Message, RawUpdate};

const UNROUTABLE_SUBJECT: &str = "telegram:unroutable:subject";
const UNROUTABLE_CONVERSATION: &str = "telegram:unroutable:conversation";
const MAX_TELEGRAM_ID: u64 = (1_u64 << 52) - 1;
const MAX_TEXT_BYTES: usize = 65_536;
const MAX_PROTOCOL_FAILURES: u8 = 3;
const MAX_STARTUP_FAILURES: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelegramOwnerTopology {
    pub channel_account_id: ChannelAccountId,
    pub external_identity_id: ExternalIdentityId,
    pub craxii_id: CraxiiId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub owner_telegram_user_id: i64,
}

pub struct TelegramInboundProcessor<S, H, T, C> {
    topology: TelegramOwnerTopology,
    topology_service: Arc<ChannelTopologyService<T>>,
    ingress: Arc<ChannelIngressService<S, H>>,
    clock: Arc<C>,
}

impl<S, H, T, C> TelegramInboundProcessor<S, H, T, C>
where
    S: ChannelIngressStore,
    H: CommandPostCommit,
    T: ChannelIdentityStore,
    C: Clock,
{
    #[must_use]
    pub fn new(
        topology: TelegramOwnerTopology,
        topology_service: Arc<ChannelTopologyService<T>>,
        ingress: Arc<ChannelIngressService<S, H>>,
        clock: Arc<C>,
    ) -> Self {
        Self {
            topology,
            topology_service,
            ingress,
            clock,
        }
    }

    async fn process(&self, update: RawUpdate) -> Result<(), TelegramPollerError> {
        let update_id = update
            .update_id
            .as_i64()
            .filter(|value| *value >= 0)
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
        let observed_at = UtcTimestamp::from_offset_datetime(
            self.clock
                .utc_now()
                .map_err(|_| poller_error(TelegramPollerErrorKind::FatalClock))?,
        )
        .map_err(|_| poller_error(TelegramPollerErrorKind::FatalClock))?;

        let (message, unsupported_kind) = if update.edited_message.is_some() {
            (
                update
                    .edited_message
                    .and_then(|value| serde_json::from_value::<Message>(value).ok()),
                Some(UnsupportedInboundKind::Edit),
            )
        } else if let Some(value) = update.message {
            match serde_json::from_value::<Message>(value) {
                Ok(message) => (Some(message), None),
                Err(_) => (None, Some(UnsupportedInboundKind::Other)),
            }
        } else {
            (None, Some(UnsupportedInboundKind::Other))
        };

        let normalized = self
            .normalize(message.as_ref(), unsupported_kind, observed_at)
            .await?;
        let event = VerifiedInboundEvent::new(
            self.topology.channel_account_id,
            ExternalEventId::try_new(update_id.to_string())
                .map_err(|_| poller_error(TelegramPollerErrorKind::FatalProtocol))?,
            normalized.external_message_id,
            normalized.sender_subject_id,
            normalized.external_conversation_id,
            None,
            normalized.payload,
            normalized.provider_occurred_at,
            observed_at,
        )
        .map_err(|_| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
        self.ingress
            .classify(event)
            .await
            .map_err(|error| match error.kind() {
                ChannelIngressErrorKind::ContradictoryReplay => {
                    poller_error(TelegramPollerErrorKind::FatalContradictoryReplay)
                }
                ChannelIngressErrorKind::StorageInconsistent => {
                    poller_error(TelegramPollerErrorKind::FatalState)
                }
                ChannelIngressErrorKind::InvalidVerifiedEvent => {
                    poller_error(TelegramPollerErrorKind::FatalProtocol)
                }
                ChannelIngressErrorKind::StorageFailure | ChannelIngressErrorKind::Unavailable => {
                    poller_error(TelegramPollerErrorKind::RetryableState)
                }
            })?;
        Ok(())
    }

    async fn normalize(
        &self,
        message: Option<&Message>,
        forced_unsupported: Option<UnsupportedInboundKind>,
        observed_at: UtcTimestamp,
    ) -> Result<NormalizedInbound, TelegramPollerError> {
        let Some(message) = message else {
            return Ok(NormalizedInbound::unsupported(
                None,
                None,
                None,
                forced_unsupported.unwrap_or(UnsupportedInboundKind::Other),
            ));
        };
        let sender_id = message
            .from
            .as_ref()
            .and_then(|user| user.id.as_ref())
            .and_then(Value::as_i64)
            .filter(|value| telegram_positive_id(*value));
        let chat_id = message
            .chat
            .as_ref()
            .and_then(|chat| chat.id.as_ref())
            .and_then(Value::as_i64)
            .filter(|value| telegram_route_id(*value));
        let message_id = message
            .message_id
            .as_ref()
            .and_then(Value::as_i64)
            .filter(|value| *value > 0);
        let timestamp = message
            .date
            .as_ref()
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .and_then(|value| time::OffsetDateTime::from_unix_timestamp(value).ok())
            .and_then(|value| UtcTimestamp::from_offset_datetime(value).ok());
        let sender_subject_id = sender_id
            .and_then(|value| ExternalSubjectId::try_new(value.to_string()).ok())
            .unwrap_or_else(|| {
                ExternalSubjectId::try_new(UNROUTABLE_SUBJECT).expect("fixed sentinel is valid")
            });
        let external_conversation_id = chat_id
            .and_then(|value| ExternalConversationId::try_new(value.to_string()).ok())
            .unwrap_or_else(|| {
                ExternalConversationId::try_new(UNROUTABLE_CONVERSATION)
                    .expect("fixed sentinel is valid")
            });
        let external_message_id =
            message_id.and_then(|value| ExternalMessageId::try_new(value.to_string()).ok());

        let unsupported = forced_unsupported.or_else(|| {
            let chat_private = message
                .chat
                .as_ref()
                .and_then(|chat| chat.kind.as_ref())
                .and_then(Value::as_str)
                == Some("private");
            let human_owner = sender_id == Some(self.topology.owner_telegram_user_id)
                && message
                    .from
                    .as_ref()
                    .and_then(|user| user.is_bot.as_ref())
                    .and_then(Value::as_bool)
                    == Some(false);
            let no_topic = message.message_thread_id.is_none()
                && message
                    .is_topic_message
                    .as_ref()
                    .is_none_or(|value| value.as_bool() == Some(false));
            let text = message.text.as_ref().and_then(Value::as_str);
            if message.has_media_or_nontext() || text.is_none() {
                Some(UnsupportedInboundKind::NonText)
            } else if !chat_private
                || !human_owner
                || message_id.is_none()
                || chat_id.is_none()
                || message.sender_chat.is_some()
                || !no_topic
                || message.has_service_or_rich_marker()
                || timestamp.is_none()
                || text.is_some_and(|text| text.is_empty() || text.len() > MAX_TEXT_BYTES)
            {
                Some(UnsupportedInboundKind::Other)
            } else {
                None
            }
        });
        if let Some(kind) = unsupported {
            return Ok(NormalizedInbound {
                external_message_id,
                sender_subject_id,
                external_conversation_id,
                provider_occurred_at: timestamp,
                payload: VerifiedInboundPayload::Unsupported(kind),
            });
        }

        let text = message
            .text
            .as_ref()
            .and_then(Value::as_str)
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
        let content = MessageContent::try_new(vec![
            ContentBlock::text(text)
                .map_err(|_| poller_error(TelegramPollerErrorKind::FatalProtocol))?,
        ])
        .map_err(|_| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
        // A route conflict is intentionally handed to generic ingress, which durably rejects it.
        // Storage or invariant failures cannot be converted into a provider acknowledgement.
        if let Err(error) = self
            .topology_service
            .ensure_first_binding(
                self.topology.channel_account_id,
                self.topology.external_identity_id,
                self.topology.craxii_id,
                self.topology.user_id,
                self.topology.conversation_id,
                external_conversation_id.clone(),
                observed_at,
            )
            .await
            && error.kind() != ChannelTopologyErrorKind::Conflict
        {
            return Err(poller_error(match error.kind() {
                ChannelTopologyErrorKind::Storage => TelegramPollerErrorKind::RetryableState,
                ChannelTopologyErrorKind::Inconsistent => TelegramPollerErrorKind::FatalState,
                ChannelTopologyErrorKind::Conflict => unreachable!(),
            }));
        }
        Ok(NormalizedInbound {
            external_message_id,
            sender_subject_id,
            external_conversation_id,
            provider_occurred_at: timestamp,
            payload: VerifiedInboundPayload::Text(content),
        })
    }
}

struct NormalizedInbound {
    external_message_id: Option<ExternalMessageId>,
    sender_subject_id: ExternalSubjectId,
    external_conversation_id: ExternalConversationId,
    provider_occurred_at: Option<UtcTimestamp>,
    payload: VerifiedInboundPayload,
}

impl NormalizedInbound {
    fn unsupported(
        external_message_id: Option<ExternalMessageId>,
        sender_subject_id: Option<ExternalSubjectId>,
        external_conversation_id: Option<ExternalConversationId>,
        kind: UnsupportedInboundKind,
    ) -> Self {
        Self {
            external_message_id,
            sender_subject_id: sender_subject_id.unwrap_or_else(|| {
                ExternalSubjectId::try_new(UNROUTABLE_SUBJECT).expect("fixed sentinel is valid")
            }),
            external_conversation_id: external_conversation_id.unwrap_or_else(|| {
                ExternalConversationId::try_new(UNROUTABLE_CONVERSATION)
                    .expect("fixed sentinel is valid")
            }),
            provider_occurred_at: None,
            payload: VerifiedInboundPayload::Unsupported(kind),
        }
    }
}

pub(crate) trait TelegramUpdateSink: Send + Sync + 'static {
    fn process_update(
        &self,
        update: RawUpdate,
    ) -> Pin<Box<dyn Future<Output = Result<(), TelegramPollerError>> + Send + '_>>;
}

impl<S, H, T, C> TelegramUpdateSink for TelegramInboundProcessor<S, H, T, C>
where
    S: ChannelIngressStore + 'static,
    H: CommandPostCommit + 'static,
    T: ChannelIdentityStore + 'static,
    C: Clock + 'static,
{
    fn process_update(
        &self,
        update: RawUpdate,
    ) -> Pin<Box<dyn Future<Output = Result<(), TelegramPollerError>> + Send + '_>> {
        Box::pin(self.process(update))
    }
}

pub(crate) trait TelegramJitterSource: Send + 'static {
    fn sample_inclusive(&mut self, lower_millis: u64, upper_millis: u64) -> u64;
}

struct CatchUnwindFuture<F> {
    future: Pin<Box<F>>,
}

impl<F> CatchUnwindFuture<F> {
    fn new(future: F) -> Self {
        Self {
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for CatchUnwindFuture<F> {
    type Output = Result<F::Output, ()>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match std::panic::catch_unwind(AssertUnwindSafe(|| self.future.as_mut().poll(context))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(Err(())),
        }
    }
}

#[derive(Default)]
pub(crate) struct TelegramStartupFailureBudget {
    failures: u8,
}

pub struct TelegramLongPollingDriverHandle {
    stop: tokio::sync::watch::Sender<bool>,
    join: Option<tokio::task::JoinHandle<Result<(), TelegramPollerError>>>,
    started: Option<tokio::sync::oneshot::Receiver<Result<(), TelegramPollerError>>>,
}

impl TelegramLongPollingDriverHandle {
    pub async fn wait_started(&mut self) -> Result<(), TelegramPollerError> {
        self.started
            .take()
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalTaskJoin))?
            .await
            .map_err(|_| poller_error(TelegramPollerErrorKind::FatalTaskJoin))?
    }

    pub async fn shutdown_before(
        mut self,
        deadline: tokio::time::Instant,
    ) -> Result<(), TelegramPollerError> {
        let _ = self.stop.send(true);
        let mut join = self
            .join
            .take()
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalTaskJoin))?;
        tokio::select! {
            result = &mut join => result
                .map_err(|_| poller_error(TelegramPollerErrorKind::FatalTaskJoin))?,
            () = tokio::time::sleep_until(deadline) => {
                join.abort();
                match join.await {
                    Err(error) if error.is_cancelled() => Ok(()),
                    _ => Err(poller_error(TelegramPollerErrorKind::FatalTaskJoin)),
                }
            }
        }
    }
}

impl Drop for TelegramLongPollingDriverHandle {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

pub(crate) fn start_long_polling<P, J>(
    client: Arc<TelegramClient>,
    initial_batch: TelegramPollBatch,
    processor: Arc<P>,
    health: Health,
    fatal: tokio::sync::watch::Sender<bool>,
    jitter: J,
) -> TelegramLongPollingDriverHandle
where
    P: TelegramUpdateSink,
    J: TelegramJitterSource,
{
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let (started_sender, started) = tokio::sync::oneshot::channel();
    let monitor_health = health.clone();
    let monitor_fatal = fatal.clone();
    let join = tokio::spawn(
        async move {
            let result = match CatchUnwindFuture::new(run_poller(
                client,
                initial_batch,
                processor,
                health,
                stopped,
                jitter,
                started_sender,
            ))
            .await
            {
                Ok(result) => result,
                Err(_) => Err(poller_error(TelegramPollerErrorKind::FatalTaskJoin)),
            };
            if result.is_err() {
                let _ = monitor_health.mark_fatal(FatalReasonCode::Internal);
                let _ = monitor_fatal.send(true);
            }
            result
        }
        .with_current_subscriber(),
    );
    TelegramLongPollingDriverHandle {
        stop,
        join: Some(join),
        started: Some(started),
    }
}

pub(crate) async fn verify_startup_identity<J: TelegramJitterSource>(
    client: &TelegramClient,
    expected_bot_user_id: i64,
    jitter: &mut J,
    budget: &mut TelegramStartupFailureBudget,
) -> Result<(), TelegramClientError> {
    retry_startup(jitter, budget, || {
        client.verify_get_me(expected_bot_user_id)
    })
    .await?;
    retry_startup(jitter, budget, || client.verify_webhook_absent()).await
}

pub(crate) async fn startup_probe<J: TelegramJitterSource>(
    client: &TelegramClient,
    jitter: &mut J,
    budget: &mut TelegramStartupFailureBudget,
) -> Result<TelegramPollBatch, TelegramClientError> {
    retry_startup(jitter, budget, || client.get_updates(None, true)).await
}

async fn retry_startup<T, J, F, Fut>(
    jitter: &mut J,
    budget: &mut TelegramStartupFailureBudget,
    mut operation: F,
) -> Result<T, TelegramClientError>
where
    J: TelegramJitterSource,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, TelegramClientError>>,
{
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() => {
                budget.failures = budget.failures.saturating_add(1);
                if budget.failures >= MAX_STARTUP_FAILURES {
                    return Err(error);
                }
                let delay = retry_delay(budget.failures, jitter, error.retry_after());
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn run_poller<P, J>(
    client: Arc<TelegramClient>,
    initial_batch: TelegramPollBatch,
    processor: Arc<P>,
    health: Health,
    mut stopped: tokio::sync::watch::Receiver<bool>,
    mut jitter: J,
    started: tokio::sync::oneshot::Sender<Result<(), TelegramPollerError>>,
) -> Result<(), TelegramPollerError>
where
    P: TelegramUpdateSink,
    J: TelegramJitterSource,
{
    let mut next_offset = None;
    if let Err(error) = process_batch(initial_batch, processor.as_ref(), &mut next_offset).await {
        let _ = started.send(Err(error));
        return Err(error);
    }
    if health.mark_ready().is_err() {
        let error = poller_error(TelegramPollerErrorKind::FatalTaskJoin);
        let _ = started.send(Err(error));
        return Err(error);
    }
    let _ = started.send(Ok(()));
    let mut retry_failures = 0_u8;
    let mut protocol_failures = 0_u8;
    loop {
        if *stopped.borrow() {
            return Ok(());
        }
        let requested_offset = next_offset;
        let poll = client.get_updates(requested_offset, false);
        let result = tokio::select! {
            biased;
            changed = stopped.changed() => {
                if changed.is_err() || *stopped.borrow() {
                    return Ok(());
                }
                continue;
            }
            result = poll => result,
        };
        match result {
            Ok(batch) => {
                let prior_retry_failures = retry_failures;
                retry_failures = 0;
                protocol_failures = 0;
                if batch.updates.is_empty() {
                    if requested_offset.is_some() {
                        next_offset = None;
                    }
                    observe_polling_recovered(prior_retry_failures);
                    continue;
                }
                if let Err(error) = process_batch(batch, processor.as_ref(), &mut next_offset).await
                {
                    if error.kind == TelegramPollerErrorKind::RetryableState {
                        retry_failures = retry_failures.saturating_add(1);
                        let delay = retry_delay(retry_failures, &mut jitter, None);
                        observe_polling_degraded("state_unavailable", retry_failures, delay);
                        if sleep_or_stop(delay, &mut stopped).await {
                            return Ok(());
                        }
                        continue;
                    }
                    return Err(error);
                }
                observe_polling_recovered(prior_retry_failures);
            }
            Err(error) if error.retryable() => {
                if error.protocol_failure() {
                    protocol_failures = protocol_failures.saturating_add(1);
                    if protocol_failures >= MAX_PROTOCOL_FAILURES {
                        return Err(poller_error(TelegramPollerErrorKind::FatalProtocol));
                    }
                }
                retry_failures = retry_failures.saturating_add(1);
                let delay = retry_delay(retry_failures, &mut jitter, error.retry_after());
                let Some(failure_class) = retryable_client_failure_class(error.kind()) else {
                    return Err(poller_error(TelegramPollerErrorKind::FatalProvider));
                };
                observe_polling_degraded(failure_class, retry_failures, delay);
                if sleep_or_stop(delay, &mut stopped).await {
                    return Ok(());
                }
            }
            Err(_) => return Err(poller_error(TelegramPollerErrorKind::FatalProvider)),
        }
    }
}

fn observe_polling_degraded(
    failure_class: &'static str,
    consecutive_retry_count: u8,
    retry_delay: Duration,
) {
    tracing::warn!(
        event_name = "telegram_polling_degraded",
        provider = TELEGRAM_PROVIDER_KEY,
        ingress_mode = "polling",
        polling_state = "degraded_retrying",
        result_class = "retry_scheduled",
        failure_class,
        consecutive_retry_count = u64::from(consecutive_retry_count),
        retry_delay_ms = u64::try_from(retry_delay.as_millis()).unwrap_or(u64::MAX),
    );
}

fn observe_polling_recovered(prior_retry_count: u8) {
    if prior_retry_count == 0 {
        return;
    }
    tracing::info!(
        event_name = "telegram_polling_recovered",
        provider = TELEGRAM_PROVIDER_KEY,
        ingress_mode = "polling",
        polling_state = "healthy",
        result_class = "recovered",
        prior_retry_count = u64::from(prior_retry_count),
    );
}

const fn retryable_client_failure_class(kind: TelegramClientErrorKind) -> Option<&'static str> {
    match kind {
        TelegramClientErrorKind::RetryableTransport => Some("transport_unavailable"),
        TelegramClientErrorKind::RetryableRateLimited => Some("rate_limited"),
        TelegramClientErrorKind::RetryableServerRejected => Some("server_rejected"),
        TelegramClientErrorKind::MalformedResponse => Some("malformed_response"),
        TelegramClientErrorKind::OversizedResponse => Some("oversized_response"),
        TelegramClientErrorKind::FatalAuthentication
        | TelegramClientErrorKind::FatalConflict
        | TelegramClientErrorKind::FatalConfiguration
        | TelegramClientErrorKind::FatalIdentityMismatch
        | TelegramClientErrorKind::FatalWebhookConfigured
        | TelegramClientErrorKind::FatalPrivateTopics => None,
    }
}

async fn process_batch<P: TelegramUpdateSink>(
    mut batch: TelegramPollBatch,
    processor: &P,
    next_offset: &mut Option<i64>,
) -> Result<(), TelegramPollerError> {
    for update in &batch.updates {
        let id = update
            .update_id
            .as_i64()
            .filter(|value| *value >= 0)
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
        id.checked_add(1)
            .ok_or_else(|| poller_error(TelegramPollerErrorKind::FatalProtocol))?;
    }
    batch
        .updates
        .sort_by_key(|update| update.update_id.as_i64().expect("validated update id"));
    for update in batch.updates {
        let update_id = update.update_id.as_i64().expect("validated update id");
        processor.process_update(update).await?;
        *next_offset = Some(
            update_id
                .checked_add(1)
                .expect("validated update offset increment"),
        );
    }
    Ok(())
}

async fn sleep_or_stop(delay: Duration, stopped: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    tokio::select! {
        biased;
        changed = stopped.changed() => {
            changed.is_err() || *stopped.borrow()
        }
        () = tokio::time::sleep(delay) => false,
    }
}

fn retry_delay(
    failures: u8,
    jitter: &mut dyn TelegramJitterSource,
    retry_after: Option<Duration>,
) -> Duration {
    let ceilings = [1_u64, 2, 4, 8, 16, 30];
    let index = usize::from(failures.saturating_sub(1)).min(ceilings.len() - 1);
    let upper = ceilings[index] * 1_000;
    let lower = (upper / 2).max(500);
    let local = Duration::from_millis(jitter.sample_inclusive(lower, upper).clamp(lower, upper));
    retry_after.map_or(local, |provider| local.max(provider))
}

fn telegram_positive_id(value: i64) -> bool {
    value > 0 && value.unsigned_abs() <= MAX_TELEGRAM_ID
}

fn telegram_route_id(value: i64) -> bool {
    value != 0 && value.unsigned_abs() <= MAX_TELEGRAM_ID
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelegramPollerErrorKind {
    RetryableState,
    FatalProvider,
    FatalProtocol,
    FatalState,
    FatalClock,
    FatalContradictoryReplay,
    FatalTaskJoin,
    Shutdown,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct TelegramPollerError {
    kind: TelegramPollerErrorKind,
}

impl TelegramPollerError {
    #[must_use]
    pub const fn kind(self) -> TelegramPollerErrorKind {
        self.kind
    }
}

impl fmt::Display for TelegramPollerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            TelegramPollerErrorKind::RetryableState => "telegram ingress temporarily unavailable",
            TelegramPollerErrorKind::FatalProvider => "telegram provider rejected polling",
            TelegramPollerErrorKind::FatalProtocol => "telegram polling protocol failure",
            TelegramPollerErrorKind::FatalState => "telegram ingress state inconsistency",
            TelegramPollerErrorKind::FatalClock => "telegram polling clock failure",
            TelegramPollerErrorKind::FatalContradictoryReplay => {
                "telegram contradictory update replay"
            }
            TelegramPollerErrorKind::FatalTaskJoin => "telegram polling task failure",
            TelegramPollerErrorKind::Shutdown => "telegram polling shutdown",
        })
    }
}

impl fmt::Debug for TelegramPollerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for TelegramPollerError {}

const fn poller_error(kind: TelegramPollerErrorKind) -> TelegramPollerError {
    TelegramPollerError { kind }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    struct RecordingSink {
        seen: Mutex<Vec<i64>>,
        fail_on: Option<i64>,
    }

    impl TelegramUpdateSink for RecordingSink {
        fn process_update(
            &self,
            update: RawUpdate,
        ) -> Pin<Box<dyn Future<Output = Result<(), TelegramPollerError>> + Send + '_>> {
            Box::pin(async move {
                let id = update.update_id.as_i64().unwrap();
                self.seen.lock().unwrap().push(id);
                if self.fail_on == Some(id) {
                    Err(poller_error(TelegramPollerErrorKind::RetryableState))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn batch(ids: &[i64]) -> TelegramPollBatch {
        TelegramPollBatch {
            updates: ids
                .iter()
                .map(|id| serde_json::from_value(json!({"update_id":id})).unwrap())
                .collect(),
        }
    }

    #[tokio::test]
    async fn batch_is_sorted_gaps_are_valid_and_failure_never_skips_a_later_update() {
        let sink = RecordingSink {
            seen: Mutex::new(Vec::new()),
            fail_on: None,
        };
        let mut next_offset = None;
        process_batch(batch(&[50, 10, 30]), &sink, &mut next_offset)
            .await
            .unwrap();
        assert_eq!(*sink.seen.lock().unwrap(), vec![10, 30, 50]);
        assert_eq!(next_offset, Some(51));

        let sink = RecordingSink {
            seen: Mutex::new(Vec::new()),
            fail_on: Some(30),
        };
        let mut next_offset = None;
        assert_eq!(
            process_batch(batch(&[50, 10, 30]), &sink, &mut next_offset)
                .await
                .unwrap_err()
                .kind(),
            TelegramPollerErrorKind::RetryableState
        );
        assert_eq!(*sink.seen.lock().unwrap(), vec![10, 30]);
        assert_eq!(next_offset, Some(11));
    }

    struct BoundaryJitter {
        choose_upper: bool,
        bounds: Vec<(u64, u64)>,
    }

    impl TelegramJitterSource for BoundaryJitter {
        fn sample_inclusive(&mut self, lower_millis: u64, upper_millis: u64) -> u64 {
            self.bounds.push((lower_millis, upper_millis));
            if self.choose_upper {
                upper_millis
            } else {
                lower_millis
            }
        }
    }

    #[test]
    fn equal_jitter_uses_exact_ceilings_minimum_and_provider_maximum() {
        let mut low = BoundaryJitter {
            choose_upper: false,
            bounds: Vec::new(),
        };
        let delays: Vec<_> = (1..=7)
            .map(|failure| retry_delay(failure, &mut low, None))
            .collect();
        assert_eq!(
            low.bounds,
            vec![
                (500, 1_000),
                (1_000, 2_000),
                (2_000, 4_000),
                (4_000, 8_000),
                (8_000, 16_000),
                (15_000, 30_000),
                (15_000, 30_000),
            ]
        );
        assert_eq!(delays[0], Duration::from_millis(500));
        assert_eq!(delays[6], Duration::from_secs(15));

        let mut high = BoundaryJitter {
            choose_upper: true,
            bounds: Vec::new(),
        };
        assert_eq!(
            retry_delay(1, &mut high, Some(Duration::from_secs(9))),
            Duration::from_secs(9)
        );
        assert_eq!(retry_delay(6, &mut high, None), Duration::from_secs(30));
    }
}
