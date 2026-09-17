use std::fmt;
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use reqwest::{StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::bootstrap::secret::SecretString;

use super::wire::{
    ApiResponse, GetUpdatesRequest, Message, RawUpdate, SendMessageRequest, User, WebhookInfo,
};

const PRODUCTION_ORIGIN: &str = "https://api.telegram.org/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const GET_ME_TIMEOUT: Duration = Duration::from_secs(10);
const WEBHOOK_INFO_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_POLL_CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const REGULAR_POLL_CLIENT_TIMEOUT: Duration = Duration::from_secs(35);
const SEND_MESSAGE_TIMEOUT: Duration = Duration::from_secs(25);
const SMALL_BODY_CAP: usize = 64 * 1024;
const POLL_BODY_CAP: usize = 4 * 1024 * 1024;
const MAX_TELEGRAM_ID: u64 = (1_u64 << 52) - 1;

pub struct TelegramPollBatch {
    pub(super) updates: Vec<RawUpdate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelegramClientErrorKind {
    RetryableTransport,
    RetryableRateLimited,
    RetryableServerRejected,
    FatalAuthentication,
    FatalConflict,
    FatalConfiguration,
    FatalIdentityMismatch,
    FatalWebhookConfigured,
    FatalPrivateTopics,
    MalformedResponse,
    OversizedResponse,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct TelegramClientError {
    kind: TelegramClientErrorKind,
    retry_after: Option<Duration>,
}

impl TelegramClientError {
    const fn new(kind: TelegramClientErrorKind) -> Self {
        Self {
            kind,
            retry_after: None,
        }
    }

    const fn rate_limited(retry_after: Option<Duration>) -> Self {
        Self {
            kind: TelegramClientErrorKind::RetryableRateLimited,
            retry_after,
        }
    }

    #[must_use]
    pub const fn kind(self) -> TelegramClientErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn retry_after(self) -> Option<Duration> {
        self.retry_after
    }

    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self.kind,
            TelegramClientErrorKind::RetryableTransport
                | TelegramClientErrorKind::RetryableRateLimited
                | TelegramClientErrorKind::RetryableServerRejected
                | TelegramClientErrorKind::MalformedResponse
                | TelegramClientErrorKind::OversizedResponse
        )
    }

    #[must_use]
    pub const fn protocol_failure(self) -> bool {
        matches!(
            self.kind,
            TelegramClientErrorKind::MalformedResponse | TelegramClientErrorKind::OversizedResponse
        )
    }
}

impl fmt::Display for TelegramClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            TelegramClientErrorKind::RetryableTransport => "telegram transport unavailable",
            TelegramClientErrorKind::RetryableRateLimited => "telegram rate limited",
            TelegramClientErrorKind::RetryableServerRejected => {
                "telegram server temporarily rejected request"
            }
            TelegramClientErrorKind::FatalAuthentication => "telegram authentication rejected",
            TelegramClientErrorKind::FatalConflict => "telegram polling conflict",
            TelegramClientErrorKind::FatalConfiguration => "telegram configuration rejected",
            TelegramClientErrorKind::FatalIdentityMismatch => "telegram bot identity mismatch",
            TelegramClientErrorKind::FatalWebhookConfigured => "telegram webhook is configured",
            TelegramClientErrorKind::FatalPrivateTopics => {
                "telegram private topic mode is unsupported"
            }
            TelegramClientErrorKind::MalformedResponse => "telegram response is malformed",
            TelegramClientErrorKind::OversizedResponse => "telegram response exceeds size limit",
        })
    }
}

impl fmt::Debug for TelegramClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelegramClientError")
            .field("kind", &self.kind)
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

impl std::error::Error for TelegramClientError {}

enum ApiOrigin {
    Production,
    #[cfg(test)]
    Loopback(Url),
}

impl ApiOrigin {
    fn base(&self) -> Result<Url, TelegramClientError> {
        match self {
            Self::Production => Url::parse(PRODUCTION_ORIGIN)
                .map_err(|_| TelegramClientError::new(TelegramClientErrorKind::FatalConfiguration)),
            #[cfg(test)]
            Self::Loopback(url) => Ok(url.clone()),
        }
    }
}

impl fmt::Debug for ApiOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Production => "ApiOrigin::Production",
            #[cfg(test)]
            Self::Loopback(_) => "ApiOrigin::Loopback([REDACTED])",
        })
    }
}

pub struct TelegramClient {
    token: SecretString,
    origin: ApiOrigin,
    client: reqwest::Client,
}

impl TelegramClient {
    pub(crate) fn try_new(token: SecretString) -> Result<Self, TelegramClientError> {
        Self::with_origin(token, ApiOrigin::Production)
    }

    #[cfg(test)]
    pub(super) fn for_test(token: SecretString, origin: &str) -> Result<Self, TelegramClientError> {
        let origin = Url::parse(origin)
            .map_err(|_| TelegramClientError::new(TelegramClientErrorKind::FatalConfiguration))?;
        if origin.scheme() != "http"
            || origin.host_str().is_none()
            || !origin
                .host_str()
                .is_some_and(|host| matches!(host, "127.0.0.1" | "::1" | "localhost"))
        {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalConfiguration,
            ));
        }
        Self::with_origin(token, ApiOrigin::Loopback(origin))
    }

    fn with_origin(token: SecretString, origin: ApiOrigin) -> Result<Self, TelegramClientError> {
        validate_token(token.expose_secret())?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .retry(reqwest::retry::never())
            .connect_timeout(CONNECT_TIMEOUT)
            .connection_verbose(false)
            .http1_only()
            .user_agent("craxii/0.0.1")
            .build()
            .map_err(|_| TelegramClientError::new(TelegramClientErrorKind::FatalConfiguration))?;
        Ok(Self {
            token,
            origin,
            client,
        })
    }

    pub(crate) async fn verify_get_me(
        &self,
        expected_bot_user_id: i64,
    ) -> Result<(), TelegramClientError> {
        let (status, body) = self
            .post_json("getMe", &json!({}), GET_ME_TIMEOUT, SMALL_BODY_CAP)
            .await?;
        let envelope: ApiResponse<User> = decode_envelope(status, &body)?;
        let user = success_result(envelope)?;
        let id = value_i64(user.id.as_ref()).ok_or_else(malformed)?;
        let is_bot = user
            .is_bot
            .as_ref()
            .and_then(Value::as_bool)
            .ok_or_else(malformed)?;
        if !is_bot || id != expected_bot_user_id {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalIdentityMismatch,
            ));
        }
        match user.has_topics_enabled.as_ref().map(Value::as_bool) {
            Some(Some(true)) => Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalPrivateTopics,
            )),
            Some(Some(false)) | None => Ok(()),
            Some(None) => Err(malformed()),
        }
    }

    pub(crate) async fn verify_webhook_absent(&self) -> Result<(), TelegramClientError> {
        let (status, body) = self
            .post_json(
                "getWebhookInfo",
                &json!({}),
                WEBHOOK_INFO_TIMEOUT,
                SMALL_BODY_CAP,
            )
            .await?;
        let envelope: ApiResponse<WebhookInfo> = decode_envelope(status, &body)?;
        let info = success_result(envelope)?;
        let url = info
            .url
            .as_ref()
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        if url.is_empty() {
            Ok(())
        } else {
            Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalWebhookConfigured,
            ))
        }
    }

    pub(crate) async fn get_updates(
        &self,
        offset: Option<i64>,
        startup: bool,
    ) -> Result<TelegramPollBatch, TelegramClientError> {
        if offset.is_some_and(|value| value < 0) {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalConfiguration,
            ));
        }
        let timeout = if startup { 1 } else { 25 };
        let client_timeout = if startup {
            STARTUP_POLL_CLIENT_TIMEOUT
        } else {
            REGULAR_POLL_CLIENT_TIMEOUT
        };
        let request = GetUpdatesRequest {
            offset,
            limit: 32,
            timeout,
            allowed_updates: ["message"],
        };
        let (status, body) = self
            .post_json("getUpdates", &request, client_timeout, POLL_BODY_CAP)
            .await?;
        let envelope: ApiResponse<Vec<Value>> = decode_envelope(status, &body)?;
        let values = success_result(envelope)?;
        if values.len() > 32 {
            return Err(malformed());
        }
        let mut updates = Vec::with_capacity(values.len());
        for value in values {
            let update: RawUpdate = serde_json::from_value(value).map_err(|_| malformed())?;
            if !value_i64(Some(&update.update_id))
                .is_some_and(|value| value >= 0 && value.checked_add(1).is_some())
            {
                return Err(malformed());
            }
            updates.push(update);
        }
        Ok(TelegramPollBatch { updates })
    }

    pub(super) async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
    ) -> crate::domain::ChannelDispatchResult {
        use crate::domain::{ChannelDispatchResult, DeliveryFailureClass, ExternalMessageId};

        let request = SendMessageRequest { chat_id, text };
        let request = match serde_json::to_vec(&request) {
            Ok(request) => request,
            Err(_) => return permanent("telegram_client_configuration"),
        };
        let endpoint = match self.endpoint("sendMessage") {
            Ok(endpoint) => endpoint,
            Err(_) => return permanent("telegram_client_configuration"),
        };
        let response = match self
            .client
            .post(endpoint)
            .timeout(SEND_MESSAGE_TIMEOUT)
            .header(CONTENT_TYPE, "application/json")
            .body(request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_connect() => {
                return ChannelDispatchResult::RetryableFailure {
                    failure: provider_failure(
                        DeliveryFailureClass::ProviderRetryable,
                        "telegram_connect",
                    ),
                    retry_after: None,
                };
            }
            Err(_) => return unknown("telegram_transport_unknown"),
        };
        let status = response.status();
        let body = match read_bounded(response, SMALL_BODY_CAP).await {
            Ok(body) => body,
            Err(_) => return unknown("telegram_response_unusable"),
        };
        let envelope = match serde_json::from_slice::<ApiResponse<Message>>(&body) {
            Ok(envelope) => envelope,
            Err(_) => return unknown("telegram_response_malformed"),
        };
        if envelope.ok {
            if !status.is_success()
                || envelope.error_code.is_some()
                || envelope.description.is_some()
                || envelope.parameters.is_some()
            {
                return unknown("telegram_success_malformed");
            }
            let Some(message) = envelope.result else {
                return unknown("telegram_success_malformed");
            };
            let Some(message_id) = message.message_id.as_ref().and_then(Value::as_i64) else {
                return unknown("telegram_success_malformed");
            };
            let Some(returned_chat_id) = message
                .chat
                .as_ref()
                .and_then(|chat| chat.id.as_ref())
                .and_then(Value::as_i64)
            else {
                return unknown("telegram_success_malformed");
            };
            let valid_private_chat = message
                .chat
                .as_ref()
                .and_then(|chat| chat.kind.as_ref())
                .and_then(Value::as_str)
                == Some("private");
            let valid_date = message
                .date
                .as_ref()
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
                .and_then(|value| time::OffsetDateTime::from_unix_timestamp(value).ok())
                .is_some();
            if message_id <= 0 || returned_chat_id != chat_id || !valid_private_chat || !valid_date
            {
                return unknown("telegram_success_mismatch");
            }
            let external_message_id = match ExternalMessageId::try_new(message_id.to_string()) {
                Ok(value) => value,
                Err(_) => return unknown("telegram_success_malformed"),
            };
            return ChannelDispatchResult::Accepted {
                external_message_id: Some(external_message_id),
            };
        }

        if envelope.result.is_some() || envelope.error_code.is_none() {
            return unknown("telegram_rejection_malformed");
        }
        let code = envelope.error_code.expect("checked");
        let retry_after = envelope
            .parameters
            .as_ref()
            .and_then(|parameters| positive_retry_after(parameters.retry_after));
        if code == 429 {
            return ChannelDispatchResult::RetryableFailure {
                failure: provider_failure(
                    DeliveryFailureClass::ProviderRetryable,
                    "telegram_rate_limited",
                ),
                retry_after,
            };
        }
        if (500..=599).contains(&code) {
            return ChannelDispatchResult::RetryableFailure {
                failure: provider_failure(
                    DeliveryFailureClass::ProviderRetryable,
                    "telegram_server_rejected",
                ),
                retry_after: None,
            };
        }
        if (400..=499).contains(&code) {
            return ChannelDispatchResult::PermanentFailure {
                failure: provider_failure(
                    DeliveryFailureClass::ProviderPermanent,
                    "telegram_request_rejected",
                ),
            };
        }
        unknown("telegram_rejection_unclassified")
    }

    async fn post_json<T: Serialize + ?Sized>(
        &self,
        method: &'static str,
        body: &T,
        timeout: Duration,
        cap: usize,
    ) -> Result<(StatusCode, Vec<u8>), TelegramClientError> {
        let endpoint = self.endpoint(method)?;
        let body = serde_json::to_vec(body)
            .map_err(|_| TelegramClientError::new(TelegramClientErrorKind::FatalConfiguration))?;
        let response = self
            .client
            .post(endpoint)
            .timeout(timeout)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| TelegramClientError::new(TelegramClientErrorKind::RetryableTransport))?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalAuthentication,
            ));
        }
        if status == StatusCode::CONFLICT {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalConflict,
            ));
        }
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            return Err(TelegramClientError::new(
                TelegramClientErrorKind::FatalConfiguration,
            ));
        }
        let body = read_bounded(response, cap)
            .await
            .map_err(|error| match error {
                BodyReadError::Read => {
                    TelegramClientError::new(TelegramClientErrorKind::RetryableTransport)
                }
                BodyReadError::Oversized => {
                    TelegramClientError::new(TelegramClientErrorKind::OversizedResponse)
                }
            })?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = serde_json::from_slice::<ApiResponse<Value>>(&body)
                .ok()
                .and_then(|envelope| envelope.parameters)
                .and_then(|parameters| positive_retry_after(parameters.retry_after));
            return Err(TelegramClientError::rate_limited(retry_after));
        }
        Ok((status, body))
    }

    fn endpoint(&self, method: &'static str) -> Result<Url, TelegramClientError> {
        let mut url = self.origin.base()?;
        let path = format!("bot{}/{}", self.token.expose_secret(), method);
        url.set_path(&path);
        Ok(url)
    }
}

fn provider_failure(
    class: crate::domain::DeliveryFailureClass,
    code: &'static str,
) -> crate::domain::DeliveryFailure {
    crate::domain::DeliveryFailure::provider(
        class,
        Some(crate::domain::DeliveryFailureCode::try_new(code).expect("fixed code is valid")),
    )
    .expect("provider failure class is valid")
}

fn permanent(code: &'static str) -> crate::domain::ChannelDispatchResult {
    crate::domain::ChannelDispatchResult::PermanentFailure {
        failure: provider_failure(crate::domain::DeliveryFailureClass::ProviderPermanent, code),
    }
}

fn unknown(code: &'static str) -> crate::domain::ChannelDispatchResult {
    crate::domain::ChannelDispatchResult::OutcomeUnknown {
        failure: provider_failure(
            crate::domain::DeliveryFailureClass::ProviderOutcomeUnknown,
            code,
        ),
    }
}

impl fmt::Debug for TelegramClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelegramClient")
            .field("provider", &"telegram")
            .field("origin", &self.origin)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

fn validate_token(token: &str) -> Result<(), TelegramClientError> {
    let Some((prefix, secret)) = token.split_once(':') else {
        return Err(TelegramClientError::new(
            TelegramClientErrorKind::FatalConfiguration,
        ));
    };
    let valid_prefix = prefix
        .parse::<u64>()
        .ok()
        .is_some_and(|value| value > 0 && value <= MAX_TELEGRAM_ID && value.to_string() == prefix);
    let valid_secret = (20..=128).contains(&secret.len())
        && secret.is_ascii()
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if token.len() > 256
        || token.trim() != token
        || token.chars().any(char::is_whitespace)
        || !valid_prefix
        || !valid_secret
    {
        return Err(TelegramClientError::new(
            TelegramClientErrorKind::FatalConfiguration,
        ));
    }
    Ok(())
}

fn decode_envelope<T: DeserializeOwned>(
    status: StatusCode,
    body: &[u8],
) -> Result<ApiResponse<T>, TelegramClientError> {
    let envelope: ApiResponse<T> = serde_json::from_slice(body).map_err(|_| malformed())?;
    if status.is_server_error() {
        return if !envelope.ok
            && envelope
                .error_code
                .is_some_and(|code| (500..=599).contains(&code))
            && envelope.result.is_none()
        {
            Err(TelegramClientError::new(
                TelegramClientErrorKind::RetryableServerRejected,
            ))
        } else {
            Err(malformed())
        };
    }
    if !status.is_success() {
        return Err(malformed());
    }
    Ok(envelope)
}

fn success_result<T>(envelope: ApiResponse<T>) -> Result<T, TelegramClientError> {
    if envelope.ok {
        if envelope.error_code.is_some()
            || envelope.description.is_some()
            || envelope.parameters.is_some()
        {
            return Err(malformed());
        }
        return envelope.result.ok_or_else(malformed);
    }
    if envelope.result.is_some() || envelope.error_code.is_none() {
        return Err(malformed());
    }
    match envelope.error_code.expect("checked") {
        401 | 403 => Err(TelegramClientError::new(
            TelegramClientErrorKind::FatalAuthentication,
        )),
        409 => Err(TelegramClientError::new(
            TelegramClientErrorKind::FatalConflict,
        )),
        429 => Err(TelegramClientError::rate_limited(
            envelope
                .parameters
                .and_then(|parameters| positive_retry_after(parameters.retry_after)),
        )),
        500..=599 => Err(TelegramClientError::new(
            TelegramClientErrorKind::RetryableServerRejected,
        )),
        400..=499 => Err(TelegramClientError::new(
            TelegramClientErrorKind::FatalConfiguration,
        )),
        _ => Err(malformed()),
    }
}

fn malformed() -> TelegramClientError {
    TelegramClientError::new(TelegramClientErrorKind::MalformedResponse)
}

fn value_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64)
}

fn positive_retry_after(value: Option<i64>) -> Option<Duration> {
    value
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
        .map(Duration::from_secs)
}

enum BodyReadError {
    Read,
    Oversized,
}

async fn read_bounded(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, BodyReadError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| BodyReadError::Read)? {
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(BodyReadError::Oversized);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
