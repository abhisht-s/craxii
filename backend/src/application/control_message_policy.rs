//! Provider-neutral exact control-message recognition.

/// The only inbound text controls recognized before model admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlMessage {
    Cancel,
}

/// Pure exact-match policy for channel control messages.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlMessagePolicy;

impl ControlMessagePolicy {
    /// Recognizes the entire trimmed string using ASCII-only case folding.
    #[must_use]
    pub fn recognize(text: &str) -> Option<ControlMessage> {
        let candidate = text.trim();
        ["cancel", "stop", "/cancel", "/stop"]
            .iter()
            .any(|control| candidate.eq_ignore_ascii_case(control))
            .then_some(ControlMessage::Cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_exact_trimmed_ascii_case_insensitive_controls() {
        for value in [
            "stop", "cancel", "/stop", "/cancel", "STOP", " stop ", "/Cancel",
        ] {
            assert_eq!(
                ControlMessagePolicy::recognize(value),
                Some(ControlMessage::Cancel),
                "{value:?}"
            );
        }
        for value in [
            "stop please",
            "please stop",
            "don't stop",
            "can you stop?",
            "",
            "   ",
        ] {
            assert_eq!(ControlMessagePolicy::recognize(value), None, "{value:?}");
        }
    }
}
