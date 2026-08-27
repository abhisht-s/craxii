use std::fmt::{Display, Formatter};
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthState {
    LiveUnready,
    Ready,
    Draining,
    Fatal,
}

impl HealthState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LiveUnready => "live_unready",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Fatal => "fatal",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthReasonCode {
    Starting,
    StartupComplete,
    ShutdownRequested,
    FatalStartup,
    InternalFailure,
    SynchronizationFailure,
}

impl HealthReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::StartupComplete => "startup_complete",
            Self::ShutdownRequested => "shutdown_requested",
            Self::FatalStartup => "fatal_startup",
            Self::InternalFailure => "internal_failure",
            Self::SynchronizationFailure => "synchronization_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthSnapshot {
    state: HealthState,
    reason: HealthReasonCode,
}

impl HealthSnapshot {
    pub const fn state(self) -> HealthState {
        self.state
    }

    pub const fn reason(self) -> HealthReasonCode {
        self.reason
    }

    pub const fn is_live(self) -> bool {
        !matches!(self.state, HealthState::Fatal)
    }

    pub const fn is_ready(self) -> bool {
        matches!(self.state, HealthState::Ready)
    }
}

#[derive(Clone, Debug)]
pub struct Health {
    state: Arc<RwLock<HealthSnapshot>>,
}

impl Health {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(HealthSnapshot {
                state: HealthState::LiveUnready,
                reason: HealthReasonCode::Starting,
            })),
        }
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        match self.state.read() {
            Ok(snapshot) => *snapshot,
            Err(_) => HealthSnapshot {
                state: HealthState::Fatal,
                reason: HealthReasonCode::SynchronizationFailure,
            },
        }
    }

    pub fn mark_ready(&self) -> Result<(), HealthTransitionError> {
        self.transition(HealthState::Ready, HealthReasonCode::StartupComplete)
    }

    pub fn mark_draining(&self) -> Result<(), HealthTransitionError> {
        self.transition(HealthState::Draining, HealthReasonCode::ShutdownRequested)
    }

    pub fn mark_fatal(&self, reason: FatalReasonCode) -> Result<(), HealthTransitionError> {
        let reason = match reason {
            FatalReasonCode::Startup => HealthReasonCode::FatalStartup,
            FatalReasonCode::Internal => HealthReasonCode::InternalFailure,
        };
        self.transition(HealthState::Fatal, reason)
    }

    fn transition(
        &self,
        state: HealthState,
        reason: HealthReasonCode,
    ) -> Result<(), HealthTransitionError> {
        let mut current = self
            .state
            .write()
            .map_err(|_| HealthTransitionError::SynchronizationFailure)?;
        if current.state == HealthState::Fatal {
            return Err(HealthTransitionError::FatalIsTerminal);
        }
        let legal = matches!(
            (current.state, state),
            (
                HealthState::LiveUnready,
                HealthState::Ready | HealthState::Draining | HealthState::Fatal
            ) | (
                HealthState::Ready,
                HealthState::Draining | HealthState::Fatal
            ) | (HealthState::Draining, HealthState::Fatal)
        );
        if !legal {
            return Err(HealthTransitionError::InvalidTransition {
                from: current.state,
                to: state,
            });
        }
        *current = HealthSnapshot { state, reason };
        Ok(())
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FatalReasonCode {
    Startup,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthTransitionError {
    FatalIsTerminal,
    InvalidTransition { from: HealthState, to: HealthState },
    SynchronizationFailure,
}

impl Display for HealthTransitionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FatalIsTerminal => "fatal health state is terminal",
            Self::InvalidTransition { .. } => "invalid health state transition",
            Self::SynchronizationFailure => "health state synchronization failed",
        })
    }
}

impl std::error::Error for HealthTransitionError {}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn default_is_live_and_unready() {
        let snapshot = Health::new().snapshot();
        assert_eq!(snapshot.state(), HealthState::LiveUnready);
        assert_eq!(snapshot.reason(), HealthReasonCode::Starting);
        assert!(snapshot.is_live());
        assert!(!snapshot.is_ready());
    }

    #[test]
    fn initial_may_become_ready_and_ready_may_become_draining() {
        let health = Health::new();
        health.mark_ready().unwrap();
        assert!(health.snapshot().is_live());
        assert!(health.snapshot().is_ready());

        health.mark_draining().unwrap();
        let snapshot = health.snapshot();
        assert_eq!(snapshot.state(), HealthState::Draining);
        assert!(snapshot.is_live());
        assert!(!snapshot.is_ready());
    }

    #[test]
    fn initial_may_become_draining_but_draining_cannot_return_to_ready() {
        let health = Health::new();
        health.mark_draining().unwrap();
        assert_eq!(
            health.mark_ready(),
            Err(HealthTransitionError::InvalidTransition {
                from: HealthState::Draining,
                to: HealthState::Ready,
            })
        );
        let snapshot = health.snapshot();
        assert_eq!(snapshot.state(), HealthState::Draining);
        assert_eq!(snapshot.reason(), HealthReasonCode::ShutdownRequested);
    }

    #[test]
    fn ready_and_draining_may_become_fatal() {
        let ready = Health::new();
        ready.mark_ready().unwrap();
        ready.mark_fatal(FatalReasonCode::Internal).unwrap();
        assert_eq!(ready.snapshot().state(), HealthState::Fatal);
        assert_eq!(ready.snapshot().reason(), HealthReasonCode::InternalFailure);

        let draining = Health::new();
        draining.mark_draining().unwrap();
        draining.mark_fatal(FatalReasonCode::Startup).unwrap();
        assert_eq!(draining.snapshot().state(), HealthState::Fatal);
        assert_eq!(draining.snapshot().reason(), HealthReasonCode::FatalStartup);
    }

    #[test]
    fn fatal_is_not_live_or_ready_and_rejects_every_transition() {
        let health = Health::new();
        health.mark_fatal(FatalReasonCode::Startup).unwrap();
        let snapshot = health.snapshot();
        assert_eq!(snapshot.state(), HealthState::Fatal);
        assert!(!snapshot.is_live());
        assert!(!snapshot.is_ready());
        assert_eq!(
            health.mark_ready(),
            Err(HealthTransitionError::FatalIsTerminal)
        );
        assert_eq!(
            health.mark_draining(),
            Err(HealthTransitionError::FatalIsTerminal)
        );
        assert_eq!(
            health.mark_fatal(FatalReasonCode::Internal),
            Err(HealthTransitionError::FatalIsTerminal)
        );
        assert_eq!(health.snapshot().reason(), HealthReasonCode::FatalStartup);
    }

    #[test]
    fn concurrent_readers_only_observe_coherent_snapshots() {
        let health = Health::new();
        let writer = health.clone();
        let handle = thread::spawn(move || {
            writer.mark_ready().unwrap();
            thread::yield_now();
            writer.mark_draining().unwrap();
            thread::yield_now();
            writer.mark_fatal(FatalReasonCode::Internal).unwrap();
        });

        for _ in 0..2_000 {
            let snapshot = health.snapshot();
            assert_eq!(snapshot.is_ready(), snapshot.state() == HealthState::Ready);
            assert_eq!(snapshot.is_live(), snapshot.state() != HealthState::Fatal);
            assert!(matches!(
                (snapshot.state(), snapshot.reason()),
                (HealthState::LiveUnready, HealthReasonCode::Starting)
                    | (HealthState::Ready, HealthReasonCode::StartupComplete)
                    | (HealthState::Draining, HealthReasonCode::ShutdownRequested)
                    | (HealthState::Fatal, HealthReasonCode::InternalFailure)
            ));
        }
        handle.join().unwrap();
    }

    #[test]
    fn reasons_are_closed_safe_codes() {
        for reason in [
            HealthReasonCode::Starting,
            HealthReasonCode::StartupComplete,
            HealthReasonCode::ShutdownRequested,
            HealthReasonCode::FatalStartup,
            HealthReasonCode::InternalFailure,
            HealthReasonCode::SynchronizationFailure,
        ] {
            assert!(
                reason
                    .as_str()
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            );
        }
    }
}
