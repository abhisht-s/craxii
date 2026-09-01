//! Durable backend process-lifetime state.

use super::{RuntimeInstanceId, RuntimeStartEvidence, UtcTimestamp};

/// Persisted RuntimeInstance lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    Running,
    Stopping,
    Stopped,
}

impl RuntimeState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }
}

/// Closed reasons for a stopped RuntimeInstance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeStopReason {
    GracefulShutdown,
    StartupFailure,
}

impl RuntimeStopReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GracefulShutdown => "graceful_shutdown",
            Self::StartupFailure => "startup_failure",
        }
    }
}

/// Transport-neutral reason persisted when graceful shutdown begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeShutdownReason {
    GracefulShutdown,
}

impl RuntimeShutdownReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        "graceful_shutdown"
    }
}

/// Validated current-state view of one durable RuntimeInstance row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeInstance {
    start: RuntimeStartEvidence,
    state: RuntimeState,
    last_heartbeat_at: UtcTimestamp,
    stopped_at: Option<UtcTimestamp>,
    stop_reason: Option<RuntimeStopReason>,
}

impl RuntimeInstance {
    #[must_use]
    pub fn start(evidence: RuntimeStartEvidence) -> Self {
        let last_heartbeat_at = evidence.started_at();
        Self {
            start: evidence,
            state: RuntimeState::Running,
            last_heartbeat_at,
            stopped_at: None,
            stop_reason: None,
        }
    }

    pub fn try_from_persisted(
        start: RuntimeStartEvidence,
        state: RuntimeState,
        last_heartbeat_at: UtcTimestamp,
        stopped_at: Option<UtcTimestamp>,
        stop_reason: Option<RuntimeStopReason>,
    ) -> Result<Self, RuntimeLifecycleError> {
        let valid = last_heartbeat_at >= start.started_at()
            && match state {
                RuntimeState::Running => stopped_at.is_none() && stop_reason.is_none(),
                RuntimeState::Stopping => {
                    stopped_at.is_none() && stop_reason == Some(RuntimeStopReason::GracefulShutdown)
                }
                RuntimeState::Stopped => stopped_at.is_some() && stop_reason.is_some(),
            }
            && stopped_at.is_none_or(|value| value >= start.started_at());
        if !valid {
            return Err(RuntimeLifecycleError);
        }
        Ok(Self {
            start,
            state,
            last_heartbeat_at,
            stopped_at,
            stop_reason,
        })
    }

    pub fn begin_stopping(&mut self) -> Result<(), RuntimeLifecycleError> {
        if self.state != RuntimeState::Running {
            return Err(RuntimeLifecycleError);
        }
        self.state = RuntimeState::Stopping;
        self.stop_reason = Some(RuntimeStopReason::GracefulShutdown);
        Ok(())
    }

    pub fn finish_graceful(&mut self, at: UtcTimestamp) -> Result<(), RuntimeLifecycleError> {
        if self.state != RuntimeState::Stopping || at < self.start.started_at() {
            return Err(RuntimeLifecycleError);
        }
        self.state = RuntimeState::Stopped;
        self.stopped_at = Some(at);
        self.stop_reason = Some(RuntimeStopReason::GracefulShutdown);
        Ok(())
    }

    pub fn finish_startup_failure(
        &mut self,
        at: UtcTimestamp,
    ) -> Result<(), RuntimeLifecycleError> {
        if !matches!(self.state, RuntimeState::Running | RuntimeState::Stopping)
            || at < self.start.started_at()
        {
            return Err(RuntimeLifecycleError);
        }
        self.state = RuntimeState::Stopped;
        self.stopped_at = Some(at);
        self.stop_reason = Some(RuntimeStopReason::StartupFailure);
        Ok(())
    }

    pub fn observe_heartbeat(&mut self, at: UtcTimestamp) -> Result<bool, RuntimeLifecycleError> {
        if self.state != RuntimeState::Running {
            return Err(RuntimeLifecycleError);
        }
        if at <= self.last_heartbeat_at {
            return Ok(false);
        }
        self.last_heartbeat_at = at;
        Ok(true)
    }

    #[must_use]
    pub const fn runtime_instance_id(&self) -> RuntimeInstanceId {
        self.start.runtime_instance_id()
    }

    #[must_use]
    pub const fn start_evidence(&self) -> &RuntimeStartEvidence {
        &self.start
    }

    #[must_use]
    pub const fn state(&self) -> RuntimeState {
        self.state
    }

    #[must_use]
    pub const fn last_heartbeat_at(&self) -> UtcTimestamp {
        self.last_heartbeat_at
    }

    #[must_use]
    pub const fn stopped_at(&self) -> Option<UtcTimestamp> {
        self.stopped_at
    }

    #[must_use]
    pub const fn stop_reason(&self) -> Option<RuntimeStopReason> {
        self.stop_reason
    }
}

/// Safe closed runtime lifecycle failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeLifecycleError;

impl std::fmt::Display for RuntimeLifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid runtime lifecycle")
    }
}

impl std::error::Error for RuntimeLifecycleError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CraxiiId, DiagnosticPid, GitRevision, LinuxBootId, PackageVersion,
        RuntimeStartEvidenceInput, SchemaVersion, WorkstationGeneration, WorkstationId,
    };

    fn at(second: u8) -> UtcTimestamp {
        format!("2026-08-28T00:00:{second:02}.000000Z")
            .parse()
            .unwrap()
    }

    fn evidence() -> RuntimeStartEvidence {
        RuntimeStartEvidence::new(RuntimeStartEvidenceInput {
            runtime_instance_id: RuntimeInstanceId::generate(),
            craxii_id: CraxiiId::generate(),
            workstation_id: WorkstationId::generate(),
            workstation_generation: WorkstationGeneration::try_new(1).unwrap(),
            linux_boot_id: Some(LinuxBootId::try_new("non_linux_not_applicable").unwrap()),
            diagnostic_pid: Some(DiagnosticPid::try_new(7).unwrap()),
            package_version: PackageVersion::try_new("0.0.1").unwrap(),
            git_revision: GitRevision::try_new("test").unwrap(),
            schema_version: SchemaVersion::try_new(4).unwrap(),
            started_at: at(1),
        })
    }

    #[test]
    fn exact_runtime_lifecycle_and_monotonic_heartbeat_are_enforced() {
        let mut runtime = RuntimeInstance::start(evidence());
        assert_eq!(runtime.state(), RuntimeState::Running);
        assert!(!runtime.observe_heartbeat(at(1)).unwrap());
        assert!(runtime.observe_heartbeat(at(2)).unwrap());
        runtime.begin_stopping().unwrap();
        assert!(runtime.observe_heartbeat(at(3)).is_err());
        runtime.finish_graceful(at(4)).unwrap();
        assert_eq!(
            runtime.stop_reason(),
            Some(RuntimeStopReason::GracefulShutdown)
        );
    }

    #[test]
    fn startup_failure_closes_running_or_stopping_only() {
        for stopping in [false, true] {
            let mut runtime = RuntimeInstance::start(evidence());
            if stopping {
                runtime.begin_stopping().unwrap();
            }
            runtime.finish_startup_failure(at(4)).unwrap();
            assert_eq!(
                runtime.stop_reason(),
                Some(RuntimeStopReason::StartupFailure)
            );
            assert!(runtime.finish_startup_failure(at(5)).is_err());
        }
    }
}
