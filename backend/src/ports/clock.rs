use std::fmt::{Debug, Display, Formatter};
use std::sync::Mutex;
use std::time::Duration;

use time::OffsetDateTime;

pub trait Clock: Send + Sync {
    fn utc_now(&self) -> Result<OffsetDateTime, ClockError>;

    fn monotonic_now(&self) -> MonotonicInstant;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicInstant {
    elapsed: Duration,
}

impl MonotonicInstant {
    pub const fn from_elapsed(elapsed: Duration) -> Self {
        Self { elapsed }
    }

    pub const fn elapsed(self) -> Duration {
        self.elapsed
    }

    pub fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        self.elapsed.checked_sub(earlier.elapsed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError {
    WallTimeOutOfRange,
    WallTimeOverflow,
    MonotonicOverflow,
    MonotonicWouldReverse,
    SynchronizationFailure,
}

impl Display for ClockError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::WallTimeOutOfRange => "wall clock value is outside the supported UTC range",
            Self::WallTimeOverflow => "wall clock advancement overflowed",
            Self::MonotonicOverflow => "monotonic clock advancement overflowed",
            Self::MonotonicWouldReverse => "monotonic clock cannot move backward",
            Self::SynchronizationFailure => "clock synchronization failed",
        })
    }
}

impl std::error::Error for ClockError {}

pub struct TestClock {
    state: Mutex<TestClockState>,
}

#[derive(Clone, Copy)]
struct TestClockState {
    wall: OffsetDateTime,
    monotonic: MonotonicInstant,
}

impl TestClock {
    pub const fn new(wall: OffsetDateTime, monotonic_elapsed: Duration) -> Self {
        Self {
            state: Mutex::new(TestClockState {
                wall,
                monotonic: MonotonicInstant::from_elapsed(monotonic_elapsed),
            }),
        }
    }

    pub fn set_wall(&self, wall: OffsetDateTime) -> Result<(), ClockError> {
        self.with_state(|state| state.wall = wall)
    }

    pub fn advance_wall(&self, duration: time::Duration) -> Result<(), ClockError> {
        self.with_state(|state| {
            state.wall = state
                .wall
                .checked_add(duration)
                .ok_or(ClockError::WallTimeOverflow)?;
            Ok(())
        })?
    }

    pub fn set_monotonic(&self, elapsed: Duration) -> Result<(), ClockError> {
        self.with_state(|state| {
            if elapsed < state.monotonic.elapsed {
                return Err(ClockError::MonotonicWouldReverse);
            }
            state.monotonic = MonotonicInstant::from_elapsed(elapsed);
            Ok(())
        })?
    }

    pub fn advance_monotonic(&self, duration: Duration) -> Result<(), ClockError> {
        self.with_state(|state| {
            let elapsed = state
                .monotonic
                .elapsed
                .checked_add(duration)
                .ok_or(ClockError::MonotonicOverflow)?;
            state.monotonic = MonotonicInstant::from_elapsed(elapsed);
            Ok(())
        })?
    }

    fn with_state<T>(
        &self,
        operation: impl FnOnce(&mut TestClockState) -> T,
    ) -> Result<T, ClockError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClockError::SynchronizationFailure)?;
        Ok(operation(&mut state))
    }
}

impl Clock for TestClock {
    fn utc_now(&self) -> Result<OffsetDateTime, ClockError> {
        self.with_state(|state| state.wall)
    }

    fn monotonic_now(&self) -> MonotonicInstant {
        match self.state.lock() {
            Ok(state) => state.monotonic,
            Err(poisoned) => poisoned.into_inner().monotonic,
        }
    }
}

impl Debug for TestClock {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TestClock { .. }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_and_monotonic_time_are_independent_and_wall_may_reverse() {
        let start = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let clock = TestClock::new(start, Duration::from_secs(10));

        clock.advance_wall(time::Duration::seconds(-5)).unwrap();
        assert_eq!(clock.utc_now().unwrap(), start - time::Duration::seconds(5));
        assert_eq!(clock.monotonic_now().elapsed(), Duration::from_secs(10));

        clock.advance_monotonic(Duration::from_secs(3)).unwrap();
        assert_eq!(clock.utc_now().unwrap(), start - time::Duration::seconds(5));
        assert_eq!(clock.monotonic_now().elapsed(), Duration::from_secs(13));
    }

    #[test]
    fn monotonic_time_never_moves_backward() {
        let clock = TestClock::new(OffsetDateTime::UNIX_EPOCH, Duration::from_secs(2));
        assert_eq!(
            clock.set_monotonic(Duration::from_secs(1)),
            Err(ClockError::MonotonicWouldReverse)
        );
        assert_eq!(clock.monotonic_now().elapsed(), Duration::from_secs(2));
    }

    #[test]
    fn checked_advancement_reports_overflow() {
        let maximum_wall = OffsetDateTime::from_unix_timestamp(253_402_300_799).unwrap();
        let clock = TestClock::new(maximum_wall, Duration::MAX);
        assert_eq!(
            clock.advance_wall(time::Duration::SECOND),
            Err(ClockError::WallTimeOverflow)
        );
        assert_eq!(
            clock.advance_monotonic(Duration::from_nanos(1)),
            Err(ClockError::MonotonicOverflow)
        );
    }
}
