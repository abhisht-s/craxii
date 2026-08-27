//! Positive, signed-64-bit-safe ordering values.

use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use super::error::{DomainValidationError, DomainValidationKind};

macro_rules! positive_integer {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(i64);

        impl $name {
            /// Constructs a committed positive value in `1..=i64::MAX`.
            pub const fn try_new(value: i64) -> Result<Self, DomainValidationError> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(DomainValidationError::new(
                        DomainValidationKind::InvalidPositiveInteger,
                    ))
                }
            }

            /// Returns the SQLite/Swift-compatible signed integer.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }

            /// Returns the next committed value or a typed overflow.
            pub const fn checked_increment(self) -> Result<Self, DomainValidationError> {
                match self.0.checked_add(1) {
                    Some(value) => Ok(Self(value)),
                    None => Err(DomainValidationError::new(
                        DomainValidationKind::ArithmeticOverflow,
                    )),
                }
            }

            /// Adds a nonnegative amount without wrapping or truncating.
            pub fn checked_add(self, amount: u64) -> Result<Self, DomainValidationError> {
                let amount = i64::try_from(amount).map_err(|_| {
                    DomainValidationError::new(DomainValidationKind::ArithmeticOverflow)
                })?;
                self.0.checked_add(amount).map(Self).ok_or_else(|| {
                    DomainValidationError::new(DomainValidationKind::ArithmeticOverflow)
                })
            }
        }

        impl TryFrom<i64> for $name {
            type Error = DomainValidationError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl TryFrom<u64> for $name {
            type Error = DomainValidationError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                let value = i64::try_from(value).map_err(|_| {
                    DomainValidationError::new(DomainValidationKind::InvalidPositiveInteger)
                })?;
                Self::try_new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct PositiveIntegerVisitor;

                impl<'de> Visitor<'de> for PositiveIntegerVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!(
                            "a positive signed 64-bit JSON integer for ",
                            stringify!($name)
                        ))
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::try_new(value).map_err(E::custom)
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::try_from(value).map_err(E::custom)
                    }
                }

                deserializer.deserialize_any(PositiveIntegerVisitor)
            }
        }
    };
}

positive_integer!(JournalOffset, "The global durable journal replay cursor.");
positive_integer!(
    StreamSeq,
    "The contiguous sequence within one aggregate stream."
);
positive_integer!(
    ConversationWorkOrdinal,
    "FIFO work order within one conversation."
);
positive_integer!(
    AgentStepNo,
    "The ordered agent-loop step within one work item."
);
positive_integer!(
    ToolOrdinal,
    "The ordered tool call within one model response."
);
positive_integer!(
    AttemptNo,
    "The ordered retry attempt within a logical operation."
);

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_integer_contract {
        ($name:ident) => {{
            assert_eq!(
                $name::try_new(0).unwrap_err().kind(),
                DomainValidationKind::InvalidPositiveInteger
            );
            assert_eq!(
                $name::try_new(-1).unwrap_err().kind(),
                DomainValidationKind::InvalidPositiveInteger
            );

            let one = $name::try_new(1).unwrap();
            let two = one.checked_increment().unwrap();
            assert_eq!(two.get(), 2);
            assert!(one < two);
            assert_eq!(one.checked_add(0).unwrap(), one);
            assert_eq!(one.checked_add(41).unwrap().get(), 42);

            let maximum = $name::try_new(i64::MAX).unwrap();
            assert_eq!(maximum.get(), i64::MAX);
            assert_eq!(
                maximum.checked_increment().unwrap_err().kind(),
                DomainValidationKind::ArithmeticOverflow
            );
            assert_eq!(
                one.checked_add(u64::MAX).unwrap_err().kind(),
                DomainValidationKind::ArithmeticOverflow
            );

            assert_eq!(
                serde_json::to_string(&maximum).unwrap(),
                i64::MAX.to_string()
            );
            assert_eq!(
                serde_json::from_str::<$name>(&i64::MAX.to_string()).unwrap(),
                maximum
            );
            assert!(serde_json::from_str::<$name>("0").is_err());
            assert!(serde_json::from_str::<$name>("-1").is_err());
            assert!(serde_json::from_str::<$name>("9223372036854775808").is_err());
            assert!(serde_json::from_str::<$name>("\"1\"").is_err());
        }};
    }

    #[test]
    fn every_ordered_numeric_type_enforces_positive_i64_and_numeric_serde() {
        assert_integer_contract!(JournalOffset);
        assert_integer_contract!(StreamSeq);
        assert_integer_contract!(ConversationWorkOrdinal);
        assert_integer_contract!(AgentStepNo);
        assert_integer_contract!(ToolOrdinal);
        assert_integer_contract!(AttemptNo);
    }

    #[test]
    fn distinct_wrappers_keep_equal_numbers_semantically_separate() {
        let offset = JournalOffset::try_new(7).unwrap();
        let sequence = StreamSeq::try_new(7).unwrap();
        assert_eq!(offset.get(), sequence.get());
        assert_eq!(serde_json::to_string(&offset).unwrap(), "7");
        assert_eq!(serde_json::to_string(&sequence).unwrap(), "7");
    }
}
