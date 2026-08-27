//! Distinct canonical UUIDv7 identities.

use std::{fmt, str::FromStr};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};
use uuid::{Uuid, Variant};

use super::error::{DomainValidationError, DomainValidationKind};

fn parse_uuid_v7(input: &str) -> Result<Uuid, DomainValidationError> {
    if input.len() != 36 {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidCanonicalUuid,
        ));
    }

    let parsed = Uuid::parse_str(input)
        .map_err(|_| DomainValidationError::new(DomainValidationKind::InvalidCanonicalUuid))?;
    if parsed.hyphenated().to_string() != input {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidCanonicalUuid,
        ));
    }
    if parsed.is_nil() || parsed.get_variant() != Variant::RFC4122 || parsed.get_version_num() != 7
    {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidUuidVersionOrVariant,
        ));
    }

    Ok(parsed)
}

macro_rules! canonical_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Eq, Hash, PartialEq)]
        pub struct $name(Uuid);

        impl $name {
            /// Parses the exact lowercase 36-character hyphenated UUIDv7 form.
            pub fn parse_canonical(input: &str) -> Result<Self, DomainValidationError> {
                parse_uuid_v7(input).map(Self)
            }
        }

        impl FromStr for $name {
            type Err = DomainValidationError;

            fn from_str(input: &str) -> Result<Self, Self::Err> {
                Self::parse_canonical(input)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), formatter)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.to_string())
                    .finish()
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct CanonicalIdVisitor;

                impl<'de> Visitor<'de> for CanonicalIdVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!(
                            "a canonical lowercase hyphenated UUIDv7 for ",
                            stringify!($name)
                        ))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::parse_canonical(value).map_err(E::custom)
                    }
                }

                deserializer.deserialize_str(CanonicalIdVisitor)
            }
        }
    };
}

macro_rules! server_generated_id {
    ($name:ident) => {
        impl $name {
            /// Generates a production UUIDv7 for this server-owned identity.
            #[must_use]
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }
        }
    };
}

canonical_id!(CraxiiId, "The durable identity of one Craxii.");
canonical_id!(ConversationId, "The durable identity of a conversation.");
canonical_id!(MessageId, "The durable identity of a committed message.");
canonical_id!(WorkId, "The durable identity of a work item.");
canonical_id!(WorkstationId, "The logical identity of a workstation.");
canonical_id!(WorkspaceId, "The logical identity of a workspace.");
canonical_id!(
    RuntimeInstanceId,
    "The identity of one backend boot instance."
);
canonical_id!(JournalEventId, "The durable identity of a journal event.");
canonical_id!(
    ModelInvocationId,
    "The durable identity of a model attempt."
);
canonical_id!(
    LogicalInvocationId,
    "The identity grouping bounded model retries."
);
canonical_id!(
    ContextManifestId,
    "The durable identity of a context manifest."
);
canonical_id!(ToolExecutionId, "The durable identity of a tool execution.");
canonical_id!(ArtifactId, "The durable identity of an artifact.");
canonical_id!(DeviceId, "The durable identity of an authenticated device.");
canonical_id!(
    ClientCommandId,
    "A client-owned command identity validated by the server."
);
canonical_id!(
    ClientMessageId,
    "A client-owned message/idempotency identity validated by the server."
);
canonical_id!(
    ExecutionId,
    "The stable identity of a workstation dispatch."
);
canonical_id!(DraftId, "The ephemeral identity of streamed draft output.");
canonical_id!(
    CorrelationId,
    "A distinct UUIDv7 grouping one logical responsibility across events."
);

server_generated_id!(CraxiiId);
server_generated_id!(ConversationId);
server_generated_id!(MessageId);
server_generated_id!(WorkId);
server_generated_id!(WorkstationId);
server_generated_id!(WorkspaceId);
server_generated_id!(RuntimeInstanceId);
server_generated_id!(JournalEventId);
server_generated_id!(ModelInvocationId);
server_generated_id!(LogicalInvocationId);
server_generated_id!(ContextManifestId);
server_generated_id!(ToolExecutionId);
server_generated_id!(ArtifactId);
server_generated_id!(DeviceId);
server_generated_id!(ExecutionId);
server_generated_id!(DraftId);
server_generated_id!(CorrelationId);

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, hash::Hash};

    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    fn assert_hash_equality<T: Copy + fmt::Debug + Eq + Hash>(left: T, right: T) {
        use std::hash::{DefaultHasher, Hasher};

        let mut left_hasher = DefaultHasher::new();
        left.hash(&mut left_hasher);
        let mut right_hasher = DefaultHasher::new();
        right.hash(&mut right_hasher);
        assert_eq!(left, right);
        assert_eq!(left_hasher.finish(), right_hasher.finish());
    }

    macro_rules! assert_id_contract {
        ($name:ident) => {{
            let parsed: $name = V7.parse().expect("fixed UUIDv7 must parse");
            assert_eq!(parsed.to_string(), V7);
            assert_eq!(
                format!("{parsed:?}"),
                format!("{}(\"{}\")", stringify!($name), V7)
            );

            let json = serde_json::to_string(&parsed).expect("ID must serialize");
            assert_eq!(json, format!("\"{V7}\""));
            let decoded: $name = serde_json::from_str(&json).expect("ID must deserialize");
            assert_hash_equality(parsed, decoded);
        }};
    }

    #[test]
    fn every_frozen_id_has_the_same_strict_roundtrip_contract() {
        assert_id_contract!(CraxiiId);
        assert_id_contract!(ConversationId);
        assert_id_contract!(MessageId);
        assert_id_contract!(WorkId);
        assert_id_contract!(WorkstationId);
        assert_id_contract!(WorkspaceId);
        assert_id_contract!(RuntimeInstanceId);
        assert_id_contract!(JournalEventId);
        assert_id_contract!(ModelInvocationId);
        assert_id_contract!(LogicalInvocationId);
        assert_id_contract!(ContextManifestId);
        assert_id_contract!(ToolExecutionId);
        assert_id_contract!(ArtifactId);
        assert_id_contract!(DeviceId);
        assert_id_contract!(ClientCommandId);
        assert_id_contract!(ClientMessageId);
        assert_id_contract!(ExecutionId);
        assert_id_contract!(DraftId);
        assert_id_contract!(CorrelationId);
    }

    #[test]
    fn noncanonical_uuid_spellings_are_rejected() {
        let rejected = [
            V7.to_uppercase(),
            V7.replace('-', ""),
            format!("{{{V7}}}"),
            format!("urn:uuid:{V7}"),
            format!(" {V7}"),
            format!("{V7} "),
            "not-a-uuid".to_owned(),
        ];

        for input in rejected {
            let error = input.parse::<MessageId>().expect_err("must reject");
            assert_eq!(error.kind(), DomainValidationKind::InvalidCanonicalUuid);
            let json = serde_json::to_string(&input).unwrap();
            assert!(serde_json::from_str::<MessageId>(&json).is_err());
        }
    }

    #[test]
    fn nil_wrong_versions_and_non_rfc_variants_are_rejected() {
        let rejected = [
            "00000000-0000-0000-0000-000000000000",
            "f47ac10b-58cc-11cf-a447-001122334455",
            "550e8400-e29b-41d4-a716-446655440000",
            "1ef21d2f-1207-6660-8c4f-419efbd44d48",
            "01890f6c-7b3a-7cc0-18f1-2e6f7a8b9c0d",
        ];

        for input in rejected {
            let error = input.parse::<WorkId>().expect_err("must reject");
            assert_eq!(
                error.kind(),
                DomainValidationKind::InvalidUuidVersionOrVariant
            );
        }
    }

    fn assert_generated(value: impl fmt::Display) {
        let text = value.to_string();
        let uuid = Uuid::parse_str(&text).expect("generated value must parse");
        assert_eq!(text.len(), 36);
        assert_eq!(uuid.hyphenated().to_string(), text);
        assert!(!uuid.is_nil());
        assert_eq!(uuid.get_version_num(), 7);
        assert_eq!(uuid.get_variant(), Variant::RFC4122);
    }

    #[test]
    fn every_server_owned_generator_emits_canonical_rfc_uuidv7() {
        assert_generated(CraxiiId::generate());
        assert_generated(ConversationId::generate());
        assert_generated(MessageId::generate());
        assert_generated(WorkId::generate());
        assert_generated(WorkstationId::generate());
        assert_generated(WorkspaceId::generate());
        assert_generated(RuntimeInstanceId::generate());
        assert_generated(JournalEventId::generate());
        assert_generated(ModelInvocationId::generate());
        assert_generated(LogicalInvocationId::generate());
        assert_generated(ContextManifestId::generate());
        assert_generated(ToolExecutionId::generate());
        assert_generated(ArtifactId::generate());
        assert_generated(DeviceId::generate());
        assert_generated(ExecutionId::generate());
        assert_generated(DraftId::generate());
        assert_generated(CorrelationId::generate());
    }

    #[test]
    fn bounded_generated_work_id_uniqueness_sanity() {
        let generated: HashSet<_> = (0..1_024).map(|_| WorkId::generate().to_string()).collect();
        assert_eq!(generated.len(), 1_024);
    }
}
