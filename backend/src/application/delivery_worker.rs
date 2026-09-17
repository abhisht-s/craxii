//! One global sequential durable outbound-delivery worker.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::domain::{
    ChannelDispatchResult, DeliveryFailure, DeliveryFailureClass, OutboundDeliveryAttemptId,
    RuntimeInstanceId, UtcTimestamp,
};
use crate::ports::channel_delivery::ChannelDeliveryAdapterRegistry;
use crate::ports::clock::Clock;
use crate::ports::delivery_store::{
    ClaimDeliveryRequest, DeliveryClaim, DeliveryStore, PersistDispatchResultRequest,
    ShutdownDeliveryRequest,
};

pub const DELIVERY_FALLBACK_SCAN: Duration = Duration::from_secs(1);
pub const DELIVERY_DISPATCH_TIMEOUT: Duration = Duration::from_secs(30);
pub const DELIVERY_BACKOFF_CAP: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct DeliveryNotifier {
    notify: Arc<tokio::sync::Notify>,
}

impl DeliveryNotifier {
    #[must_use]
    pub fn new() -> Self {
        Self {
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn wake(&self) {
        self.notify.notify_one();
    }
}

impl Default for DeliveryNotifier {
    fn default() -> Self {
        Self::new()
    }
}

pub trait DeliveryJitterSource: Send + 'static {
    fn sample_inclusive(&mut self, upper_bound_millis: u64) -> u64;
}

pub struct DeliveryWorkerHandle {
    notifier: DeliveryNotifier,
    claiming: Arc<AtomicBool>,
    claim_gate: Arc<tokio::sync::Mutex<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), DeliveryWorkerError>>>,
    store: Arc<dyn DeliveryStore>,
    clock: Arc<dyn Clock>,
    runtime_instance_id: RuntimeInstanceId,
    initial_scan: Option<tokio::sync::oneshot::Receiver<Result<(), DeliveryWorkerError>>>,
}

struct WorkerCoordination {
    notifier: DeliveryNotifier,
    claiming: Arc<AtomicBool>,
    claim_gate: Arc<tokio::sync::Mutex<()>>,
    initial_scan_sender: tokio::sync::oneshot::Sender<Result<(), DeliveryWorkerError>>,
}

impl DeliveryWorkerHandle {
    #[must_use]
    pub fn notifier(&self) -> DeliveryNotifier {
        self.notifier.clone()
    }

    pub fn stop_claiming(&self) {
        self.claiming.store(false, Ordering::Release);
        self.notifier.wake();
    }

    /// Closes delivery-claim admission and waits for an already-entered claim section to either
    /// leave durable state unchanged or reconcile its committed attempt before returning.
    pub fn stop_claiming_and_wait(&self) -> impl Future<Output = ()> + Send + 'static {
        self.stop_claiming();
        let claim_gate = Arc::clone(&self.claim_gate);
        async move {
            let quiesced = claim_gate.lock().await;
            drop(quiesced);
        }
    }

    /// Waits until the worker has completed its first durable claim scan.
    pub async fn wait_initial_scan(&mut self) -> Result<(), DeliveryWorkerError> {
        self.initial_scan
            .take()
            .ok_or(DeliveryWorkerError::TaskJoin)?
            .await
            .map_err(|_| DeliveryWorkerError::TaskJoin)?
    }

    pub async fn shutdown_before(
        mut self,
        deadline: tokio::time::Instant,
    ) -> Result<(), DeliveryWorkerError> {
        self.stop_claiming_and_wait().await;
        let mut join = self.join.take().ok_or(DeliveryWorkerError::TaskJoin)?;
        tokio::select! {
            result = &mut join => {
                result.map_err(|_| DeliveryWorkerError::TaskJoin)??;
                Ok(())
            }
            () = tokio::time::sleep_until(deadline) => {
                join.abort();
                let join_result = observe_aborted_worker(join).await;
                let classification_result = classify_shutdown_interruption(
                    self.store.as_ref(),
                    self.clock.as_ref(),
                    self.runtime_instance_id,
                ).await;
                match classification_result {
                    Err(primary) => Err(primary),
                    Ok(()) => join_result,
                }
            }
        }
    }
}

impl Drop for DeliveryWorkerHandle {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryWorkerError {
    StateStore,
    Clock,
    TaskJoin,
}

impl std::fmt::Display for DeliveryWorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StateStore => "delivery worker state-store failure",
            Self::Clock => "delivery worker clock failure",
            Self::TaskJoin => "delivery worker task join failure",
        })
    }
}

impl std::error::Error for DeliveryWorkerError {}

pub fn start_delivery_worker<J>(
    store: Arc<dyn DeliveryStore>,
    adapters: Arc<ChannelDeliveryAdapterRegistry>,
    clock: Arc<dyn Clock>,
    runtime_instance_id: RuntimeInstanceId,
    notifier: DeliveryNotifier,
    jitter: J,
    fatal: tokio::sync::watch::Sender<bool>,
) -> DeliveryWorkerHandle
where
    J: DeliveryJitterSource,
{
    let claiming = Arc::new(AtomicBool::new(true));
    let claim_gate = Arc::new(tokio::sync::Mutex::new(()));
    let worker_store = Arc::clone(&store);
    let worker_clock = Arc::clone(&clock);
    let worker_notifier = notifier.clone();
    let worker_claiming = Arc::clone(&claiming);
    let worker_claim_gate = Arc::clone(&claim_gate);
    let (initial_scan_sender, initial_scan) = tokio::sync::oneshot::channel();
    let join = tokio::spawn(async move {
        let result = run_worker(
            worker_store,
            adapters,
            worker_clock,
            runtime_instance_id,
            jitter,
            WorkerCoordination {
                notifier: worker_notifier,
                claiming: worker_claiming,
                claim_gate: worker_claim_gate,
                initial_scan_sender,
            },
        )
        .await;
        if result.is_err() {
            let _ = fatal.send(true);
        }
        result
    });
    DeliveryWorkerHandle {
        notifier,
        claiming,
        claim_gate,
        join: Some(join),
        store,
        clock,
        runtime_instance_id,
        initial_scan: Some(initial_scan),
    }
}

async fn run_worker<J>(
    store: Arc<dyn DeliveryStore>,
    adapters: Arc<ChannelDeliveryAdapterRegistry>,
    clock: Arc<dyn Clock>,
    runtime_instance_id: RuntimeInstanceId,
    mut jitter: J,
    coordination: WorkerCoordination,
) -> Result<(), DeliveryWorkerError>
where
    J: DeliveryJitterSource,
{
    let mut initial_scan_sender = Some(coordination.initial_scan_sender);
    loop {
        let claim_section = coordination.claim_gate.lock().await;
        if !coordination.claiming.load(Ordering::Acquire) {
            return Ok(());
        }
        let claimed_at = now(clock.as_ref())?;
        let claim_result = store
            .claim_next_delivery(ClaimDeliveryRequest {
                runtime_instance_id,
                outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
                now: claimed_at,
            })
            .await;
        let claimed = match claim_result {
            Ok(claimed) => {
                if let Some(sender) = initial_scan_sender.take() {
                    let _ = sender.send(Ok(()));
                }
                claimed
            }
            Err(_) => {
                if let Some(sender) = initial_scan_sender.take() {
                    let _ = sender.send(Err(DeliveryWorkerError::StateStore));
                }
                return Err(DeliveryWorkerError::StateStore);
            }
        };
        if !coordination.claiming.load(Ordering::Acquire) {
            if matches!(claimed, DeliveryClaim::Dispatch(_)) {
                store
                    .interrupt_owned_delivery(ShutdownDeliveryRequest {
                        runtime_instance_id,
                        interrupted_at: claimed_at,
                    })
                    .await
                    .map_err(|_| DeliveryWorkerError::StateStore)?;
            }
            return Ok(());
        }
        drop(claim_section);
        match claimed {
            DeliveryClaim::StateAdvanced => continue,
            DeliveryClaim::NoneDue => {
                tokio::select! {
                    () = coordination.notifier.notify.notified() => {}
                    () = tokio::time::sleep(DELIVERY_FALLBACK_SCAN) => {}
                }
            }
            DeliveryClaim::Dispatch(dispatch) => {
                let result = if let Some(adapter) = adapters.adapter(&dispatch.provider_id) {
                    dispatch_with_timeout(
                        Arc::clone(adapter),
                        dispatch.as_ref().clone(),
                        DELIVERY_DISPATCH_TIMEOUT,
                    )
                    .await
                } else {
                    ChannelDispatchResult::PermanentFailure {
                        failure: DeliveryFailure::classified(
                            DeliveryFailureClass::AdapterUnavailable,
                        ),
                    }
                };
                let local_retry_delay =
                    matches!(result, ChannelDispatchResult::RetryableFailure { .. })
                        .then(|| backoff(dispatch.attempt_number, &mut jitter));
                store
                    .persist_dispatch_result(PersistDispatchResultRequest {
                        dispatch,
                        runtime_instance_id,
                        result,
                        local_retry_delay,
                        completed_at: now(clock.as_ref())?,
                    })
                    .await
                    .map_err(|_| DeliveryWorkerError::StateStore)?;
            }
        }
    }
}

async fn dispatch_with_timeout(
    adapter: Arc<dyn crate::ports::channel_delivery::ChannelDeliveryAdapter>,
    dispatch: crate::domain::PreparedChannelDispatch,
    timeout: Duration,
) -> ChannelDispatchResult {
    let mut dispatch = adapter.dispatch(dispatch);
    let guarded_dispatch = std::future::poll_fn(move |context| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch.as_mut().poll(context)
        })) {
            Ok(std::task::Poll::Ready(result)) => std::task::Poll::Ready(Ok(result)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(_) => std::task::Poll::Ready(Err(())),
        }
    });
    match tokio::time::timeout(timeout, guarded_dispatch).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => ChannelDispatchResult::OutcomeUnknown {
            failure: DeliveryFailure::classified(DeliveryFailureClass::ProviderOutcomeUnknown),
        },
    }
}

async fn classify_shutdown_interruption(
    store: &dyn DeliveryStore,
    clock: &dyn Clock,
    runtime_instance_id: RuntimeInstanceId,
) -> Result<(), DeliveryWorkerError> {
    let interrupted_at = now(clock)?;
    store
        .interrupt_owned_delivery(ShutdownDeliveryRequest {
            runtime_instance_id,
            interrupted_at,
        })
        .await
        .map_err(|_| DeliveryWorkerError::StateStore)?;
    Ok(())
}

async fn observe_aborted_worker(
    join: tokio::task::JoinHandle<Result<(), DeliveryWorkerError>>,
) -> Result<(), DeliveryWorkerError> {
    match join.await {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Ok(()),
        Err(_) => Err(DeliveryWorkerError::TaskJoin),
    }
}

fn backoff(attempt_number: u16, jitter: &mut dyn DeliveryJitterSource) -> Duration {
    let exponent = u32::from(attempt_number.saturating_sub(1)).min(31);
    let ceiling_seconds = 1_u64
        .checked_shl(exponent)
        .unwrap_or(u64::MAX)
        .min(DELIVERY_BACKOFF_CAP.as_secs());
    let ceiling_millis = ceiling_seconds.saturating_mul(1_000);
    Duration::from_millis(jitter.sample_inclusive(ceiling_millis).max(1_000))
}

fn now(clock: &dyn Clock) -> Result<UtcTimestamp, DeliveryWorkerError> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| DeliveryWorkerError::Clock)?)
        .map_err(|_| DeliveryWorkerError::Clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ChannelAccountId, ChannelProviderId, ExternalConversationId, OutboundDeliveryId,
        PreparedChannelDispatch, Sha256Digest,
    };
    use crate::ports::channel_delivery::{ChannelDeliveryAdapter, ChannelDeliveryFuture};

    struct Fixed(u64);
    impl DeliveryJitterSource for Fixed {
        fn sample_inclusive(&mut self, upper_bound_millis: u64) -> u64 {
            self.0.min(upper_bound_millis)
        }
    }

    #[test]
    fn backoff_is_full_jitter_clamped_and_capped() {
        assert_eq!(backoff(1, &mut Fixed(0)), Duration::from_secs(1));
        assert_eq!(backoff(2, &mut Fixed(u64::MAX)), Duration::from_secs(2));
        assert_eq!(backoff(16, &mut Fixed(u64::MAX)), Duration::from_secs(300));
    }

    struct ExceptionalAdapter {
        provider: ChannelProviderId,
        panic: bool,
    }

    impl ChannelDeliveryAdapter for ExceptionalAdapter {
        fn provider_id(&self) -> &ChannelProviderId {
            &self.provider
        }

        fn dispatch(&self, _dispatch: PreparedChannelDispatch) -> ChannelDeliveryFuture<'_> {
            Box::pin(async move {
                if self.panic {
                    panic!("synthetic adapter panic");
                }
                std::future::pending().await
            })
        }
    }

    fn dispatch() -> PreparedChannelDispatch {
        PreparedChannelDispatch {
            outbound_delivery_id: OutboundDeliveryId::generate(),
            outbound_delivery_attempt_id: OutboundDeliveryAttemptId::generate(),
            attempt_number: 1,
            channel_account_id: ChannelAccountId::generate(),
            provider_id: ChannelProviderId::try_new("fake").unwrap(),
            external_conversation_id: ExternalConversationId::try_new("destination").unwrap(),
            external_thread_id: None,
            text: "payload".into(),
            payload_sha256: Sha256Digest::hash_bytes(b"payload"),
            dispatch_material_sha256: Sha256Digest::hash_bytes(b"material"),
            part_ordinal: 1,
            part_count: 1,
        }
    }

    #[tokio::test]
    async fn timeout_and_adapter_panic_are_both_normalized_to_unknown() {
        for adapter in [
            ExceptionalAdapter {
                provider: ChannelProviderId::try_new("fake").unwrap(),
                panic: false,
            },
            ExceptionalAdapter {
                provider: ChannelProviderId::try_new("fake").unwrap(),
                panic: true,
            },
        ] {
            let adapter: Arc<dyn ChannelDeliveryAdapter> = Arc::new(adapter);
            let result = dispatch_with_timeout(adapter, dispatch(), Duration::from_millis(1)).await;
            assert!(matches!(
                result,
                ChannelDispatchResult::OutcomeUnknown { ref failure }
                    if failure.class() == DeliveryFailureClass::ProviderOutcomeUnknown
            ));
        }
    }
}
