use std::fmt::{Debug, Display, Formatter};

use serde::{Serialize, Serializer};

pub const REDACTION_MARKER: &str = "[REDACTED]";

pub struct SecretString(#[allow(dead_code)] String);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[allow(dead_code)]
    pub(crate) fn expose_secret(&self) -> &str {
        &self.0
    }
}

pub struct SecretBytes(#[allow(dead_code)] Vec<u8>);

impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    #[allow(dead_code)]
    pub(crate) fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

macro_rules! impl_redacted {
    ($type:ty) => {
        impl Display for $type {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(REDACTION_MARKER)
            }
        }

        impl Debug for $type {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(REDACTION_MARKER)
            }
        }

        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(REDACTION_MARKER)
            }
        }
    };
}

impl_redacted!(SecretString);
impl_redacted!(SecretBytes);

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "sentinel-secret-XYZ-123456789";

    #[test]
    fn string_formatting_and_json_are_fixed_redactions() {
        let secret = SecretString::new(SENTINEL.to_owned());
        assert_eq!(secret.expose_secret(), SENTINEL);
        assert_redacted(&secret);
    }

    #[test]
    fn byte_formatting_and_json_are_fixed_redactions() {
        let secret = SecretBytes::new(SENTINEL.as_bytes().to_vec());
        assert_eq!(secret.expose_secret(), SENTINEL.as_bytes());
        assert_redacted(&secret);
    }

    fn assert_redacted(secret: &(impl Display + Debug + Serialize)) {
        let display = format!("{secret}");
        let debug = format!("{secret:?}");
        let json = serde_json::to_string(secret).unwrap();
        for output in [&display, &debug, &json] {
            assert!(!output.contains(SENTINEL));
            assert!(!output.contains("sentinel"));
            assert!(!output.contains("XYZ"));
            assert!(!output.contains("123456789"));
            assert!(!output.contains(&SENTINEL.len().to_string()));
        }
        assert_eq!(display, REDACTION_MARKER);
        assert_eq!(debug, REDACTION_MARKER);
        assert_eq!(json, format!("\"{REDACTION_MARKER}\""));
    }
}
