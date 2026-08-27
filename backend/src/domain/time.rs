//! Canonical wall-clock evidence and process-local monotonic duration values.

use std::{fmt, str::FromStr, time::Duration};

use ::time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use super::error::{DomainValidationError, DomainValidationKind};

/// A UTC instant represented canonically as `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
///
/// This is wall-clock evidence/presentation only. It is never workflow, replay,
/// lifecycle, attempt, journal, or causal ordering authority.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UtcTimestamp(OffsetDateTime);

impl UtcTimestamp {
    /// Normalizes a trusted instant to UTC and truncates sub-microsecond precision.
    pub fn from_offset_datetime(value: OffsetDateTime) -> Result<Self, DomainValidationError> {
        let utc = value
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(|| DomainValidationError::new(DomainValidationKind::TimestampOutOfRange))?;
        if !(0..=9_999).contains(&utc.year()) {
            return Err(DomainValidationError::new(
                DomainValidationKind::TimestampOutOfRange,
            ));
        }

        let microsecond_nanoseconds = (utc.nanosecond() / 1_000) * 1_000;
        let canonical = utc
            .replace_nanosecond(microsecond_nanoseconds)
            .map_err(|_| DomainValidationError::new(DomainValidationKind::TimestampOutOfRange))?;
        Ok(Self(canonical))
    }

    /// Parses only the exact canonical six-fractional-digit UTC form.
    pub fn parse_canonical(input: &str) -> Result<Self, DomainValidationError> {
        if !has_canonical_timestamp_shape(input) {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidCanonicalTimestamp,
            ));
        }

        let parsed = OffsetDateTime::parse(input, &Rfc3339).map_err(|_| {
            DomainValidationError::new(DomainValidationKind::InvalidCanonicalTimestamp)
        })?;
        let timestamp = Self::from_offset_datetime(parsed).map_err(|_| {
            DomainValidationError::new(DomainValidationKind::InvalidCanonicalTimestamp)
        })?;
        if timestamp.to_string() != input {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidCanonicalTimestamp,
            ));
        }
        Ok(timestamp)
    }

    /// Returns the normalized, microsecond-truncated UTC instant.
    #[must_use]
    pub const fn to_offset_datetime(self) -> OffsetDateTime {
        self.0
    }
}

fn has_canonical_timestamp_shape(input: &str) -> bool {
    if input.len() != 27 || !input.is_ascii() {
        return false;
    }

    input.bytes().enumerate().all(|(index, byte)| match index {
        4 | 7 => byte == b'-',
        10 => byte == b'T',
        13 | 16 => byte == b':',
        19 => byte == b'.',
        26 => byte == b'Z',
        _ => byte.is_ascii_digit(),
    })
}

impl FromStr for UtcTimestamp {
    type Err = DomainValidationError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse_canonical(input)
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day(),
            self.0.hour(),
            self.0.minute(),
            self.0.second(),
            self.0.microsecond(),
        )
    }
}

impl fmt::Debug for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("UtcTimestamp")
            .field(&self.to_string())
            .finish()
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TimestampVisitor;

        impl<'de> Visitor<'de> for TimestampVisitor {
            type Value = UtcTimestamp;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a UTC timestamp in YYYY-MM-DDTHH:MM:SS.ffffffZ form")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                UtcTimestamp::parse_canonical(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(TimestampVisitor)
    }
}

/// A process-local monotonic duration with no persistence or Serde contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MonotonicDuration(Duration);

impl MonotonicDuration {
    /// Wraps a process-local standard-library duration.
    #[must_use]
    pub const fn from_duration(value: Duration) -> Self {
        Self(value)
    }

    /// Constructs a process-local duration from milliseconds.
    #[must_use]
    pub const fn from_millis(value: u64) -> Self {
        Self(Duration::from_millis(value))
    }

    /// Returns the process-local duration.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        self.0
    }

    /// Adds two durations without wrapping.
    pub const fn checked_add(self, other: Self) -> Result<Self, DomainValidationError> {
        match self.0.checked_add(other.0) {
            Some(value) => Ok(Self(value)),
            None => Err(DomainValidationError::new(
                DomainValidationKind::ArithmeticOverflow,
            )),
        }
    }

    /// Subtracts durations without creating a negative value.
    pub const fn checked_sub(self, other: Self) -> Result<Self, DomainValidationError> {
        match self.0.checked_sub(other.0) {
            Some(value) => Ok(Self(value)),
            None => Err(DomainValidationError::new(
                DomainValidationKind::ArithmeticUnderflow,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use ::time::{Date, Month};

    use super::*;

    fn trusted(
        date: (i32, Month, u8),
        time: (u8, u8, u8, u32),
        offset: UtcOffset,
    ) -> OffsetDateTime {
        Date::from_calendar_date(date.0, date.1, date.2)
            .unwrap()
            .with_hms_nano(time.0, time.1, time.2, time.3)
            .unwrap()
            .assume_offset(offset)
    }

    #[test]
    fn trusted_offsets_normalize_to_utc_and_utc_is_unchanged() {
        let positive = trusted(
            (2024, Month::January, 2),
            (3, 4, 5, 123_456_000),
            UtcOffset::from_hms(5, 30, 0).unwrap(),
        );
        assert_eq!(
            UtcTimestamp::from_offset_datetime(positive)
                .unwrap()
                .to_string(),
            "2024-01-01T21:34:05.123456Z"
        );

        let negative = trusted(
            (2024, Month::January, 2),
            (3, 4, 5, 654_321_000),
            UtcOffset::from_hms(-7, 0, 0).unwrap(),
        );
        assert_eq!(
            UtcTimestamp::from_offset_datetime(negative)
                .unwrap()
                .to_string(),
            "2024-01-02T10:04:05.654321Z"
        );

        let utc = trusted((2024, Month::January, 2), (3, 4, 5, 1_000), UtcOffset::UTC);
        let canonical = UtcTimestamp::from_offset_datetime(utc).unwrap();
        assert_eq!(canonical.to_offset_datetime(), utc);
        assert_eq!(canonical.to_string(), "2024-01-02T03:04:05.000001Z");
    }

    #[test]
    fn trusted_submicroseconds_are_deliberately_truncated() {
        let trusted = trusted(
            (2024, Month::June, 30),
            (23, 59, 58, 123_456_999),
            UtcOffset::UTC,
        );
        let timestamp = UtcTimestamp::from_offset_datetime(trusted).unwrap();
        assert_eq!(timestamp.to_string(), "2024-06-30T23:59:58.123456Z");
        assert_eq!(timestamp.to_offset_datetime().nanosecond(), 123_456_000);
    }

    #[test]
    fn canonical_parse_display_debug_and_serde_are_exact() {
        let text = "2026-08-27T12:34:56.000007Z";
        let timestamp: UtcTimestamp = text.parse().unwrap();
        assert_eq!(timestamp.to_string(), text);
        assert_eq!(
            format!("{timestamp:?}"),
            format!("UtcTimestamp(\"{text}\")")
        );
        let json = serde_json::to_string(&timestamp).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(
            serde_json::from_str::<UtcTimestamp>(&json).unwrap(),
            timestamp
        );
    }

    #[test]
    fn canonical_parse_rejects_every_noncanonical_rfc3339_shape() {
        let rejected = [
            "2026-08-27T12:34:56Z",
            "2026-08-27T12:34:56.1Z",
            "2026-08-27T12:34:56.12345Z",
            "2026-08-27T12:34:56.1234567Z",
            "2026-08-27T12:34:56.123456+00:00",
            "2026-08-27T12:34:56.123456+05:30",
            "2026-08-27t12:34:56.123456Z",
            "2026-08-27T12:34:56.123456z",
            " 2026-08-27T12:34:56.123456Z",
            "2026-08-27T12:34:56.123456Z ",
            "2026-08-27T12:34:56.123456Ztrailing",
            "2026-02-30T12:34:56.123456Z",
            "2026-08-27 12:34:56.123456Z",
        ];

        for input in rejected {
            let error = input.parse::<UtcTimestamp>().expect_err("must reject");
            assert_eq!(
                error.kind(),
                DomainValidationKind::InvalidCanonicalTimestamp,
                "unexpected kind for {input}"
            );
            let json = serde_json::to_string(input).unwrap();
            assert!(serde_json::from_str::<UtcTimestamp>(&json).is_err());
        }
    }

    #[test]
    fn pre_epoch_timestamp_roundtrips_and_trusted_out_of_range_is_typed() {
        let text = "1969-12-31T23:59:59.999999Z";
        let timestamp: UtcTimestamp = text.parse().unwrap();
        assert_eq!(timestamp.to_string(), text);
        assert!(timestamp.to_offset_datetime().unix_timestamp_nanos() < 0);

        let outside_canonical_year =
            trusted((-1, Month::December, 31), (23, 59, 59, 0), UtcOffset::UTC);
        assert_eq!(
            UtcTimestamp::from_offset_datetime(outside_canonical_year)
                .unwrap_err()
                .kind(),
            DomainValidationKind::TimestampOutOfRange
        );
    }

    #[test]
    fn monotonic_duration_construction_and_checked_arithmetic_are_process_local() {
        let first = MonotonicDuration::from_millis(250);
        let second = MonotonicDuration::from_duration(Duration::from_millis(750));
        assert_eq!(
            first.checked_add(second).unwrap().as_duration(),
            Duration::from_secs(1)
        );
        assert_eq!(
            second.checked_sub(first).unwrap().as_duration(),
            Duration::from_millis(500)
        );
        assert_eq!(
            first.checked_sub(second).unwrap_err().kind(),
            DomainValidationKind::ArithmeticUnderflow
        );
        assert_eq!(
            MonotonicDuration::from_duration(Duration::MAX)
                .checked_add(MonotonicDuration::from_millis(1))
                .unwrap_err()
                .kind(),
            DomainValidationKind::ArithmeticOverflow
        );
    }
}
