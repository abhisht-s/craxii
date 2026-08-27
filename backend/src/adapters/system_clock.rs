use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;

use crate::ports::clock::{Clock, ClockError, MonotonicInstant};

#[derive(Debug)]
pub struct SystemClock {
    monotonic_origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            monotonic_origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn utc_now(&self) -> Result<OffsetDateTime, ClockError> {
        system_time_to_utc(SystemTime::now())
    }

    fn monotonic_now(&self) -> MonotonicInstant {
        MonotonicInstant::from_elapsed(self.monotonic_origin.elapsed())
    }
}

fn system_time_to_utc(value: SystemTime) -> Result<OffsetDateTime, ClockError> {
    let nanoseconds = match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration_to_nanoseconds(duration)?,
        Err(error) => duration_to_nanoseconds(error.duration())?
            .checked_neg()
            .ok_or(ClockError::WallTimeOutOfRange)?,
    };
    OffsetDateTime::from_unix_timestamp_nanos(nanoseconds)
        .map_err(|_| ClockError::WallTimeOutOfRange)
}

fn duration_to_nanoseconds(duration: Duration) -> Result<i128, ClockError> {
    i128::try_from(duration.as_nanos()).map_err(|_| ClockError::WallTimeOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_system_time_on_both_sides_of_epoch() {
        assert_eq!(
            system_time_to_utc(UNIX_EPOCH).unwrap(),
            OffsetDateTime::UNIX_EPOCH
        );
        assert_eq!(
            system_time_to_utc(UNIX_EPOCH - Duration::from_micros(1))
                .unwrap()
                .unix_timestamp_nanos(),
            -1_000
        );
    }
}
