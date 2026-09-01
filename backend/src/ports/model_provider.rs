//! Provider-neutral Stage 15 provider, estimator, retry, deadline, and cancellation boundary.

use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    Certainty, ModelCapabilitySnapshot, ModelRequest, ModelStreamEvent,
    ModelStreamProviderErrorKind, ModelTarget, NormalizedError, ProviderId, TokenEstimatorIdentity,
};
use crate::ports::clock::MonotonicInstant;

/// Initial request plus no more than two retries.
pub const MAX_PROVIDER_ATTEMPTS: u32 = 3;
pub const PROVIDER_BACKOFF_BASE: Duration = Duration::from_millis(250);
pub const PROVIDER_BACKOFF_LOCAL_CAP: Duration = Duration::from_secs(5);
pub const PROVIDER_RETRY_AFTER_CAP: Duration = Duration::from_secs(30);
pub const DEFAULT_PROVIDER_INVOCATION_LIMIT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_PROVIDER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub type ModelProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// Provider failure categories independent of HTTP/network implementation details.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProviderErrorKind {
    Authentication,
    Authorization,
    InvalidRequest,
    UnknownModel,
    RateLimited,
    TemporarilyUnavailable,
    TransportBeforeResponse,
    TransportAfterPossibleProcessing,
    TimeoutBeforeOutput,
    TimeoutAfterOutput,
    MalformedResponse,
    MalformedCompletedToolArguments,
    OutputTooLarge,
    UnsupportedResponseItem,
    ContextError,
    SafetyRefusal,
    Cancelled,
    ProviderOutcomeUnknown,
    InternalProviderError,
    ScriptMismatch,
    InvalidScriptProgram,
}

impl ProviderErrorKind {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::InvalidRequest => "invalid_request",
            Self::UnknownModel => "unknown_model",
            Self::RateLimited => "rate_limited",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::TransportBeforeResponse => "transport_before_response",
            Self::TransportAfterPossibleProcessing => "transport_after_possible_processing",
            Self::TimeoutBeforeOutput => "timeout_before_output",
            Self::TimeoutAfterOutput => "timeout_after_output",
            Self::MalformedResponse => "malformed_response",
            Self::MalformedCompletedToolArguments => "malformed_completed_tool_arguments",
            Self::OutputTooLarge => "output_too_large",
            Self::UnsupportedResponseItem => "unsupported_response_item",
            Self::ContextError => "context_error",
            Self::SafetyRefusal => "safety_refusal",
            Self::Cancelled => "cancelled",
            Self::ProviderOutcomeUnknown => "provider_outcome_unknown",
            Self::InternalProviderError => "internal_provider_error",
            Self::ScriptMismatch => "script_mismatch",
            Self::InvalidScriptProgram => "invalid_scripted_provider_program",
        }
    }

    const fn transient_before_output(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::TemporarilyUnavailable
                | Self::TransportBeforeResponse
                | Self::TimeoutBeforeOutput
        )
    }
}

/// Certainty about what the provider may have accepted or emitted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProviderOutcomeCertainty {
    DefinitelyNotSent,
    DefiniteProviderFailure,
    DefinitelyCompleted,
    SemanticOutputObserved,
    ProviderOutcomeUnknown,
}

impl ProviderOutcomeCertainty {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DefinitelyNotSent => "definitely_not_sent",
            Self::DefiniteProviderFailure => "definite_provider_failure",
            Self::DefinitelyCompleted => "definitely_completed",
            Self::SemanticOutputObserved => "semantic_output_observed",
            Self::ProviderOutcomeUnknown => "outcome_unknown",
        }
    }

    #[must_use]
    pub const fn normalized(self) -> Certainty {
        match self {
            Self::ProviderOutcomeUnknown => Certainty::OutcomeUnknown,
            Self::DefinitelyNotSent
            | Self::DefiniteProviderFailure
            | Self::DefinitelyCompleted
            | Self::SemanticOutputObserved => Certainty::Definite,
        }
    }
}

/// Redacted provider failure with only stable classification and bounded retry guidance.
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderError {
    kind: ProviderErrorKind,
    certainty: ProviderOutcomeCertainty,
    retry_after: Option<Duration>,
}

impl ProviderError {
    #[must_use]
    pub const fn new(kind: ProviderErrorKind, certainty: ProviderOutcomeCertainty) -> Self {
        Self {
            kind,
            certainty,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn with_retry_after(
        kind: ProviderErrorKind,
        certainty: ProviderOutcomeCertainty,
        retry_after: Duration,
    ) -> Self {
        Self {
            kind,
            certainty,
            retry_after: Some(retry_after.min(PROVIDER_RETRY_AFTER_CAP)),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ProviderErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn certainty(&self) -> ProviderOutcomeCertainty {
        self.certainty
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    /// Canonical terminal stream classification preserving outcome certainty.
    #[must_use]
    pub const fn stream_terminal_kind(&self) -> ModelStreamProviderErrorKind {
        match self.kind {
            ProviderErrorKind::Cancelled => ModelStreamProviderErrorKind::Cancelled,
            ProviderErrorKind::ProviderOutcomeUnknown
            | ProviderErrorKind::TransportAfterPossibleProcessing => {
                ModelStreamProviderErrorKind::OutcomeUnknown
            }
            ProviderErrorKind::TimeoutBeforeOutput => {
                ModelStreamProviderErrorKind::TimeoutBeforeOutput
            }
            ProviderErrorKind::TimeoutAfterOutput => {
                ModelStreamProviderErrorKind::TimeoutAfterOutput
            }
            ProviderErrorKind::MalformedResponse
            | ProviderErrorKind::MalformedCompletedToolArguments
            | ProviderErrorKind::OutputTooLarge
            | ProviderErrorKind::UnsupportedResponseItem => {
                ModelStreamProviderErrorKind::ProtocolFailure
            }
            ProviderErrorKind::RateLimited
            | ProviderErrorKind::TemporarilyUnavailable
            | ProviderErrorKind::TransportBeforeResponse => {
                ModelStreamProviderErrorKind::TransientUnavailable
            }
            ProviderErrorKind::Authentication
            | ProviderErrorKind::Authorization
            | ProviderErrorKind::InvalidRequest
            | ProviderErrorKind::UnknownModel
            | ProviderErrorKind::ContextError
            | ProviderErrorKind::SafetyRefusal
            | ProviderErrorKind::InternalProviderError
            | ProviderErrorKind::ScriptMismatch
            | ProviderErrorKind::InvalidScriptProgram => {
                ModelStreamProviderErrorKind::DefiniteFailure
            }
        }
    }

    #[must_use]
    pub fn normalized(&self) -> NormalizedError {
        if self.kind.transient_before_output()
            && matches!(
                self.certainty,
                ProviderOutcomeCertainty::DefinitelyNotSent
                    | ProviderOutcomeCertainty::DefiniteProviderFailure
            )
        {
            NormalizedError::provider_bounded(self.certainty.normalized(), None)
        } else if self.kind == ProviderErrorKind::Cancelled {
            NormalizedError::cancellation(self.certainty.normalized())
        } else {
            NormalizedError::provider(self.certainty.normalized(), None)
        }
    }
}

impl Display for ProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind.code())
    }
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderError")
            .field("kind", &self.kind)
            .field("certainty", &self.certainty)
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

impl std::error::Error for ProviderError {}

/// Positive provider-attempt ordinal bounded by the V0 initial-plus-two policy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProviderAttempt(u32);

impl ProviderAttempt {
    pub fn try_new(value: u32) -> Result<Self, ProviderError> {
        if value == 0 || value > MAX_PROVIDER_ATTEMPTS {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Why the pure retry classifier did or did not allow another attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryReasonCode {
    ClassifiedTransientBeforeOutput,
    SemanticOutputObserved,
    NonretryableCategory,
    ProviderOutcomeAmbiguous,
    AttemptCapReached,
    Cancelled,
    DeadlineExhausted,
}

impl RetryReasonCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClassifiedTransientBeforeOutput => "classified_transient_before_output",
            Self::SemanticOutputObserved => "semantic_output_observed",
            Self::NonretryableCategory => "nonretryable_category",
            Self::ProviderOutcomeAmbiguous => "provider_outcome_ambiguous",
            Self::AttemptCapReached => "attempt_cap_reached",
            Self::Cancelled => "cancelled",
            Self::DeadlineExhausted => "deadline_exhausted",
        }
    }
}

/// Whether a physical provider attempt reported normalized usage evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelUsageStatus {
    Reported,
    Unavailable,
    NotApplicableOrUnknown,
}

impl ModelUsageStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Unavailable => "unavailable",
            Self::NotApplicableOrUnknown => "not_applicable_or_unknown",
        }
    }
}

/// Exact durable reason and bounded delay that caused a physical retry attempt to exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderRetryEvidence {
    pub reason: RetryReasonCode,
    pub delay: Duration,
    pub provider_retry_after: Option<Duration>,
}

/// Complete pure retry policy result. This never performs a retry or sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderRetryDecision {
    retryable: bool,
    reason: RetryReasonCode,
    certainty: ProviderOutcomeCertainty,
    attempt_cap: u32,
    provider_retry_after: Option<Duration>,
}

impl ProviderRetryDecision {
    #[must_use]
    pub const fn retryable(self) -> bool {
        self.retryable
    }

    #[must_use]
    pub const fn reason(self) -> RetryReasonCode {
        self.reason
    }

    #[must_use]
    pub const fn certainty(self) -> ProviderOutcomeCertainty {
        self.certainty
    }

    #[must_use]
    pub const fn attempt_cap(self) -> u32 {
        self.attempt_cap
    }

    #[must_use]
    pub const fn provider_retry_after(self) -> Option<Duration> {
        self.provider_retry_after
    }
}

/// Classifies only; Stage 17 will own attempt rows and retry orchestration.
#[must_use]
pub fn classify_provider_retry(
    error: &ProviderError,
    semantic_output_observed: bool,
    current_attempt: u32,
    cancellation_requested: bool,
    deadline_exhausted: bool,
) -> ProviderRetryDecision {
    let reason = if cancellation_requested || error.kind == ProviderErrorKind::Cancelled {
        RetryReasonCode::Cancelled
    } else if deadline_exhausted {
        RetryReasonCode::DeadlineExhausted
    } else if semantic_output_observed
        || error.certainty == ProviderOutcomeCertainty::SemanticOutputObserved
    {
        RetryReasonCode::SemanticOutputObserved
    } else if error.certainty == ProviderOutcomeCertainty::ProviderOutcomeUnknown
        || error.kind == ProviderErrorKind::ProviderOutcomeUnknown
        || error.kind == ProviderErrorKind::TransportAfterPossibleProcessing
    {
        RetryReasonCode::ProviderOutcomeAmbiguous
    } else if current_attempt >= MAX_PROVIDER_ATTEMPTS {
        RetryReasonCode::AttemptCapReached
    } else if error.kind.transient_before_output() {
        RetryReasonCode::ClassifiedTransientBeforeOutput
    } else {
        RetryReasonCode::NonretryableCategory
    };
    ProviderRetryDecision {
        retryable: reason == RetryReasonCode::ClassifiedTransientBeforeOutput,
        reason,
        certainty: error.certainty,
        attempt_cap: MAX_PROVIDER_ATTEMPTS,
        provider_retry_after: error.retry_after,
    }
}

/// Injected full-jitter source. Implementations must return `0..=upper_bound`.
pub trait FullJitterSource {
    fn sample_inclusive(&mut self, upper_bound: u64) -> u64;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackoffDecision {
    Delay(Duration),
    Cancelled,
    DeadlineInsufficient,
}

/// Pure exponential/full-jitter calculation with bounded provider Retry-After guidance.
#[must_use]
pub fn provider_backoff(
    failed_attempt: u32,
    retry_after: Option<Duration>,
    jitter: &mut dyn FullJitterSource,
    cancellation_requested: bool,
    remaining_budget: Option<Duration>,
) -> BackoffDecision {
    if cancellation_requested {
        return BackoffDecision::Cancelled;
    }
    let exponent = failed_attempt.saturating_sub(1).min(31);
    let factor = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    let local_ceiling = PROVIDER_BACKOFF_BASE
        .checked_mul(factor)
        .unwrap_or(PROVIDER_BACKOFF_LOCAL_CAP)
        .min(PROVIDER_BACKOFF_LOCAL_CAP);
    let ceiling = retry_after
        .map(|value| value.min(PROVIDER_RETRY_AFTER_CAP))
        .unwrap_or(local_ceiling);
    let upper_millis = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
    let sampled = jitter.sample_inclusive(upper_millis).min(upper_millis);
    let delay = Duration::from_millis(sampled);
    if remaining_budget.is_some_and(|remaining| delay >= remaining) {
        BackoffDecision::DeadlineInsufficient
    } else {
        BackoffDecision::Delay(delay)
    }
}

/// Cloneable cancellation signal with an awaitable edge and no provider-specific handle.
#[derive(Clone, Debug)]
pub struct ProviderCancellationToken {
    sender: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for ProviderCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderCancellationToken {
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = tokio::sync::watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut receiver = self.sender.subscribe();
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
    }
}

/// Candidate absolute deadlines; the minimum is frozen once for one provider attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvocationDeadlineInputs {
    pub work_deadline: Option<MonotonicInstant>,
    pub shutdown_deadline: Option<MonotonicInstant>,
    pub provider_deadline: MonotonicInstant,
    pub retry_budget_deadline: Option<MonotonicInstant>,
}

impl InvocationDeadlineInputs {
    #[must_use]
    pub fn effective(self) -> MonotonicInstant {
        [
            self.work_deadline,
            self.shutdown_deadline,
            Some(self.provider_deadline),
            self.retry_budget_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
        .expect("provider deadline is always present")
    }
}

#[must_use]
pub fn default_provider_deadline(started_at: MonotonicInstant) -> Option<MonotonicInstant> {
    started_at.checked_add(DEFAULT_PROVIDER_INVOCATION_LIMIT)
}

#[must_use]
pub fn remaining_before(deadline: MonotonicInstant, now: MonotonicInstant) -> Option<Duration> {
    deadline.checked_duration_since(now)
}

/// Frozen per-attempt provider control passed unchanged to provider stream ownership.
#[derive(Clone, Debug)]
pub struct ModelInvocationControl {
    cancellation: ProviderCancellationToken,
    absolute_deadline: MonotonicInstant,
    idle_timeout: Duration,
}

impl ModelInvocationControl {
    pub fn try_new(
        cancellation: ProviderCancellationToken,
        absolute_deadline: MonotonicInstant,
        idle_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if idle_timeout.is_zero() || idle_timeout > DEFAULT_PROVIDER_IDLE_TIMEOUT {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            ));
        }
        Ok(Self {
            cancellation,
            absolute_deadline,
            idle_timeout,
        })
    }

    #[must_use]
    pub const fn cancellation(&self) -> &ProviderCancellationToken {
        &self.cancellation
    }

    #[must_use]
    pub const fn absolute_deadline(&self) -> MonotonicInstant {
        self.absolute_deadline
    }

    #[must_use]
    pub const fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }
}

/// Already-selected provider invocation command. It performs no selection or persistence.
#[derive(Clone, Debug)]
pub struct ModelProviderInvocation {
    pub request: ModelRequest,
    pub attempt: ProviderAttempt,
    pub control: ModelInvocationControl,
    pub fixture_key: Option<String>,
}

/// Owned provider-neutral event sequence.
pub trait ModelProviderStream: Send {
    fn next_event(&mut self) -> ModelProviderFuture<'_, Option<ModelStreamEvent>>;
}

/// Object-safe provider port. Stage 19 adapters translate wire types behind this boundary.
pub trait ModelProvider: Send + Sync {
    fn provider_id(&self) -> &ProviderId;

    fn capabilities(&self, target: &ModelTarget) -> Result<ModelCapabilitySnapshot, ProviderError>;

    fn invoke_stream(
        &self,
        invocation: ModelProviderInvocation,
    ) -> ModelProviderFuture<'_, Box<dyn ModelProviderStream>>;
}

/// Stage 16-neutral units accepted by conservative estimators after caller rendering choices.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TokenEstimateUnit {
    TextBytes(u64),
    StructuredBytes(u64),
    ToolDefinitionBytes(u64),
    ProviderOpaqueBytes(u64),
}

/// Conservative estimate plus the exact estimator identity used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConservativeTokenEstimate {
    estimator: TokenEstimatorIdentity,
    tokens: u64,
}

impl ConservativeTokenEstimate {
    pub fn try_new(estimator: TokenEstimatorIdentity, tokens: u64) -> Result<Self, ProviderError> {
        if tokens > i64::MAX as u64 {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                ProviderOutcomeCertainty::DefinitelyNotSent,
            ));
        }
        Ok(Self { estimator, tokens })
    }

    #[must_use]
    pub const fn estimator(&self) -> &TokenEstimatorIdentity {
        &self.estimator
    }

    #[must_use]
    pub const fn tokens(&self) -> u64 {
        self.tokens
    }
}

/// Object-safe provider-neutral token estimator seam; it never assembles context.
pub trait TokenEstimator: Send + Sync {
    fn identity(&self) -> &TokenEstimatorIdentity;

    fn estimate(
        &self,
        target: &ModelTarget,
        units: &[TokenEstimateUnit],
    ) -> Result<ConservativeTokenEstimate, ProviderError>;
}

#[cfg(test)]
pub(crate) mod provider_contract;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Retryability;

    struct MaximumJitter;

    impl FullJitterSource for MaximumJitter {
        fn sample_inclusive(&mut self, upper_bound: u64) -> u64 {
            upper_bound
        }
    }

    struct HalfJitter;

    impl FullJitterSource for HalfJitter {
        fn sample_inclusive(&mut self, upper_bound: u64) -> u64 {
            upper_bound / 2
        }
    }

    fn error(kind: ProviderErrorKind) -> ProviderError {
        let certainty = match kind {
            ProviderErrorKind::TransportBeforeResponse => {
                ProviderOutcomeCertainty::DefinitelyNotSent
            }
            ProviderErrorKind::TransportAfterPossibleProcessing
            | ProviderErrorKind::ProviderOutcomeUnknown => {
                ProviderOutcomeCertainty::ProviderOutcomeUnknown
            }
            ProviderErrorKind::TimeoutAfterOutput => {
                ProviderOutcomeCertainty::SemanticOutputObserved
            }
            _ => ProviderOutcomeCertainty::DefiniteProviderFailure,
        };
        ProviderError::new(kind, certainty)
    }

    #[test]
    fn retry_classifier_covers_every_provider_category_before_output() {
        let retryable = [
            ProviderErrorKind::RateLimited,
            ProviderErrorKind::TemporarilyUnavailable,
            ProviderErrorKind::TransportBeforeResponse,
            ProviderErrorKind::TimeoutBeforeOutput,
        ];
        let never = [
            ProviderErrorKind::Authentication,
            ProviderErrorKind::Authorization,
            ProviderErrorKind::InvalidRequest,
            ProviderErrorKind::UnknownModel,
            ProviderErrorKind::TransportAfterPossibleProcessing,
            ProviderErrorKind::TimeoutAfterOutput,
            ProviderErrorKind::MalformedResponse,
            ProviderErrorKind::MalformedCompletedToolArguments,
            ProviderErrorKind::OutputTooLarge,
            ProviderErrorKind::UnsupportedResponseItem,
            ProviderErrorKind::ContextError,
            ProviderErrorKind::SafetyRefusal,
            ProviderErrorKind::Cancelled,
            ProviderErrorKind::ProviderOutcomeUnknown,
            ProviderErrorKind::InternalProviderError,
            ProviderErrorKind::ScriptMismatch,
            ProviderErrorKind::InvalidScriptProgram,
        ];
        for kind in retryable {
            let decision = classify_provider_retry(&error(kind), false, 1, false, false);
            assert!(decision.retryable(), "{kind:?}");
            assert_eq!(
                decision.reason(),
                RetryReasonCode::ClassifiedTransientBeforeOutput
            );
            assert_eq!(decision.attempt_cap(), 3);
        }
        for kind in never {
            assert!(
                !classify_provider_retry(&error(kind), false, 1, false, false).retryable(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn provider_error_terminal_mapping_preserves_certainty_and_protocol_failure() {
        let cases = [
            (
                ProviderErrorKind::Authentication,
                ModelStreamProviderErrorKind::DefiniteFailure,
            ),
            (
                ProviderErrorKind::InvalidRequest,
                ModelStreamProviderErrorKind::DefiniteFailure,
            ),
            (
                ProviderErrorKind::Cancelled,
                ModelStreamProviderErrorKind::Cancelled,
            ),
            (
                ProviderErrorKind::ProviderOutcomeUnknown,
                ModelStreamProviderErrorKind::OutcomeUnknown,
            ),
            (
                ProviderErrorKind::TransportAfterPossibleProcessing,
                ModelStreamProviderErrorKind::OutcomeUnknown,
            ),
            (
                ProviderErrorKind::TimeoutBeforeOutput,
                ModelStreamProviderErrorKind::TimeoutBeforeOutput,
            ),
            (
                ProviderErrorKind::TimeoutAfterOutput,
                ModelStreamProviderErrorKind::TimeoutAfterOutput,
            ),
            (
                ProviderErrorKind::MalformedResponse,
                ModelStreamProviderErrorKind::ProtocolFailure,
            ),
            (
                ProviderErrorKind::UnsupportedResponseItem,
                ModelStreamProviderErrorKind::ProtocolFailure,
            ),
        ];
        for (provider, terminal) in cases {
            assert_eq!(
                error(provider).stream_terminal_kind(),
                terminal,
                "{provider:?}"
            );
        }
    }

    #[test]
    fn provider_attempt_ordinal_is_positive_and_capped_at_three() {
        assert_eq!(ProviderAttempt::try_new(1).unwrap().get(), 1);
        assert_eq!(ProviderAttempt::try_new(3).unwrap().get(), 3);
        assert!(ProviderAttempt::try_new(0).is_err());
        assert!(ProviderAttempt::try_new(4).is_err());
    }

    #[test]
    fn semantic_output_cancellation_ambiguity_deadline_and_attempt_cap_veto_retry() {
        let transient = error(ProviderErrorKind::RateLimited);
        assert_eq!(
            classify_provider_retry(&transient, true, 1, false, false).reason(),
            RetryReasonCode::SemanticOutputObserved
        );
        assert_eq!(
            classify_provider_retry(&transient, false, 1, true, false).reason(),
            RetryReasonCode::Cancelled
        );
        assert_eq!(
            classify_provider_retry(&transient, false, 1, false, true).reason(),
            RetryReasonCode::DeadlineExhausted
        );
        assert_eq!(
            classify_provider_retry(&transient, false, 3, false, false).reason(),
            RetryReasonCode::AttemptCapReached
        );
        let ambiguous = error(ProviderErrorKind::ProviderOutcomeUnknown);
        assert_eq!(
            classify_provider_retry(&ambiguous, false, 1, false, false).reason(),
            RetryReasonCode::ProviderOutcomeAmbiguous
        );
    }

    #[test]
    fn rate_limit_retry_after_is_bounded_and_normalized_without_raw_provider_text() {
        let error = ProviderError::with_retry_after(
            ProviderErrorKind::RateLimited,
            ProviderOutcomeCertainty::DefiniteProviderFailure,
            Duration::from_secs(90),
        );
        let decision = classify_provider_retry(&error, false, 1, false, false);
        assert_eq!(
            decision.provider_retry_after(),
            Some(Duration::from_secs(30))
        );
        assert_eq!(error.normalized().retryability(), Retryability::Bounded);
        assert_eq!(
            format!("{error:?}"),
            "ProviderError { kind: RateLimited, certainty: DefiniteProviderFailure, retry_after: Some(30s) }"
        );
    }

    #[test]
    fn backoff_attempts_caps_retry_after_and_full_jitter_without_sleeping() {
        let mut maximum = MaximumJitter;
        assert_eq!(
            provider_backoff(1, None, &mut maximum, false, None),
            BackoffDecision::Delay(Duration::from_millis(250))
        );
        assert_eq!(
            provider_backoff(2, None, &mut maximum, false, None),
            BackoffDecision::Delay(Duration::from_millis(500))
        );
        assert_eq!(
            provider_backoff(10, None, &mut maximum, false, None),
            BackoffDecision::Delay(Duration::from_secs(5))
        );
        assert_eq!(
            provider_backoff(1, Some(Duration::from_secs(3)), &mut maximum, false, None),
            BackoffDecision::Delay(Duration::from_secs(3))
        );
        assert_eq!(
            provider_backoff(1, Some(Duration::from_secs(90)), &mut maximum, false, None),
            BackoffDecision::Delay(Duration::from_secs(30))
        );
        let mut half = HalfJitter;
        assert_eq!(
            provider_backoff(2, None, &mut half, false, None),
            BackoffDecision::Delay(Duration::from_millis(250))
        );
    }

    #[test]
    fn cancellation_and_absolute_deadline_veto_backoff_without_sleeping() {
        let mut maximum = MaximumJitter;
        assert_eq!(
            provider_backoff(1, None, &mut maximum, true, None),
            BackoffDecision::Cancelled
        );
        assert_eq!(
            provider_backoff(
                1,
                None,
                &mut maximum,
                false,
                Some(Duration::from_millis(250))
            ),
            BackoffDecision::DeadlineInsufficient
        );
    }

    #[test]
    fn effective_deadline_is_the_absolute_minimum_and_never_reconstructed() {
        let base = MonotonicInstant::from_elapsed(Duration::from_secs(100));
        let provider = default_provider_deadline(base).unwrap();
        assert_eq!(provider.elapsed(), Duration::from_secs(400));
        let effective = InvocationDeadlineInputs {
            work_deadline: Some(MonotonicInstant::from_elapsed(Duration::from_secs(350))),
            shutdown_deadline: Some(MonotonicInstant::from_elapsed(Duration::from_secs(200))),
            provider_deadline: provider,
            retry_budget_deadline: Some(MonotonicInstant::from_elapsed(Duration::from_secs(250))),
        }
        .effective();
        assert_eq!(effective.elapsed(), Duration::from_secs(200));
        assert_eq!(
            remaining_before(
                effective,
                MonotonicInstant::from_elapsed(Duration::from_secs(150))
            ),
            Some(Duration::from_secs(50))
        );
        assert_eq!(
            remaining_before(
                effective,
                MonotonicInstant::from_elapsed(Duration::from_secs(201))
            ),
            None
        );
    }

    #[tokio::test]
    async fn cancellation_token_observes_preexisting_and_released_cancellation_without_sleep() {
        let token = ProviderCancellationToken::new();
        let waiter = token.clone();
        let join = tokio::spawn(async move { waiter.cancelled().await });
        token.cancel();
        join.await.unwrap();
        assert!(token.is_cancelled());
        token.cancelled().await;
    }

    #[test]
    fn invocation_control_enforces_idle_ceiling_and_preserves_absolute_deadline() {
        let deadline = MonotonicInstant::from_elapsed(Duration::from_secs(500));
        let control = ModelInvocationControl::try_new(
            ProviderCancellationToken::new(),
            deadline,
            DEFAULT_PROVIDER_IDLE_TIMEOUT,
        )
        .unwrap();
        assert_eq!(control.absolute_deadline(), deadline);
        assert_eq!(control.idle_timeout(), Duration::from_secs(60));
        assert!(
            ModelInvocationControl::try_new(
                ProviderCancellationToken::new(),
                deadline,
                Duration::from_secs(61),
            )
            .is_err()
        );
    }
}
