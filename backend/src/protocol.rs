//! Transport-independent Craxii public protocol version 1.
//!
//! These DTOs deliberately contain no Axum, Hyper, SQLx, provider, operating-system, or
//! filesystem types. Request decoding is closed; response/event encoding may gain additive
//! optional fields only through an architecture decision.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::{
    ClientCommandId, ClientMessageId, ContentBlock, ConversationId, ConversationLifecycle,
    ConversationWorkOrdinal, CraxiiId, JournalEventId, JournalOffset, JournalWorkTerminalReason,
    MessageContent, MessageId, MessageRole, ToolExecutionId, UtcTimestamp, WorkId, WorkState,
};

pub const PROTOCOL_VERSION: u64 = 1;
pub const MESSAGE_BODY_LIMIT: usize = 512 * 1024;
pub const CANCELLATION_BODY_LIMIT: usize = 8 * 1024;
pub const HTTP_CONCURRENCY_LIMIT: usize = 64;
pub const MUTATION_CONCURRENCY_LIMIT: usize = 16;
pub const WEBSOCKET_CONNECTION_LIMIT: usize = 32;
pub const REPLAY_PAGE_ROWS: u32 = 128;
pub const CURSOR_BROADCAST_CAPACITY: usize = 256;
pub const WEBSOCKET_OUTBOUND_FRAMES: usize = 16;
pub const MAX_DURABLE_PAYLOAD_BYTES: usize = 262_144;
pub const MAX_WEBSOCKET_FRAME_BYTES: usize = 270_336;
pub const MAX_BOOTSTRAP_MESSAGES: usize = 2_048;
pub const MAX_BOOTSTRAP_TERMINAL_WORK: usize = 512;
pub const MAX_BOOTSTRAP_TOOL_SUMMARIES: usize = 2_048;
pub const MAX_BOOTSTRAP_SOURCE_MESSAGE_JSON_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_BOOTSTRAP_JSON_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolVersion;

impl Serialize for ProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(PROTOCOL_VERSION)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        if value == PROTOCOL_VERSION {
            Ok(Self)
        } else {
            Err(de::Error::custom("unsupported protocol version"))
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RequestId(Uuid);

impl RequestId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(formatter)
    }
}

impl fmt::Debug for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RequestId")
            .field(&self.to_string())
            .finish()
    }
}

impl Serialize for RequestId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Wire cursor. Zero is the replay-from-start sentinel; positive values map to JournalOffset.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReplayCursor(u64);

impl ReplayCursor {
    pub const START: Self = Self(0);

    pub const fn try_new(value: u64) -> Result<Self, CursorParseError> {
        if value <= i64::MAX as u64 {
            Ok(Self(value))
        } else {
            Err(CursorParseError)
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn as_journal_offset(self) -> Option<JournalOffset> {
        if self.0 == 0 {
            None
        } else {
            JournalOffset::try_from(self.0).ok()
        }
    }

    #[must_use]
    pub fn from_journal_offset(value: JournalOffset) -> Self {
        Self(value.get() as u64)
    }
}

impl FromStr for ReplayCursor {
    type Err = CursorParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(CursorParseError);
        }
        let parsed = value.parse::<u64>().map_err(|_| CursorParseError)?;
        let cursor = Self::try_new(parsed)?;
        if cursor.to_string() != value {
            return Err(CursorParseError);
        }
        Ok(cursor)
    }
}

impl fmt::Display for ReplayCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorParseError;

impl fmt::Display for CursorParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid replay cursor")
    }
}

impl std::error::Error for CursorParseError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicContentType {
    Text,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicContentBlock {
    #[serde(rename = "type")]
    pub kind: PublicContentType,
    pub text: String,
}

impl PublicContentBlock {
    #[must_use]
    pub fn from_domain(block: &ContentBlock) -> Self {
        Self {
            kind: PublicContentType::Text,
            text: block.as_text().to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MessageRequest {
    pub protocol_version: ProtocolVersion,
    pub client_message_id: ClientMessageId,
    pub content: Vec<PublicContentBlock>,
}

impl MessageRequest {
    pub fn into_content(self) -> Result<MessageContent, ProtocolValidationError> {
        let blocks = self
            .content
            .into_iter()
            .map(|block| ContentBlock::text(block.text).map_err(|_| ProtocolValidationError))
            .collect::<Result<Vec<_>, _>>()?;
        MessageContent::try_new(blocks).map_err(|_| ProtocolValidationError)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MessageResponse {
    pub protocol_version: ProtocolVersion,
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub work_state: WorkState,
    pub conversation_work_ordinal: ConversationWorkOrdinal,
    pub committed_cursor: JournalOffset,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CancellationRequest {
    pub protocol_version: ProtocolVersion,
    pub client_command_id: ClientCommandId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CancellationResponse {
    pub protocol_version: ProtocolVersion,
    pub work_id: WorkId,
    pub work_state: WorkState,
    pub committed_cursor: JournalOffset,
    pub duplicate: bool,
    pub cleanup_pending: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Live,
    Ready,
    LiveUnready,
    Draining,
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    pub protocol_version: ProtocolVersion,
    pub status: HealthStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub protocol_version: ProtocolVersion,
    pub error: PublicError,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicError {
    pub code: &'static str,
    pub message: &'static str,
    pub retryable: bool,
    pub request_id: RequestId,
}

impl ErrorEnvelope {
    #[must_use]
    pub const fn new(
        code: &'static str,
        message: &'static str,
        retryable: bool,
        request_id: RequestId,
    ) -> Self {
        Self {
            protocol_version: ProtocolVersion,
            error: PublicError {
                code,
                message,
                retryable,
                request_id,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BootstrapResponse {
    pub protocol_version: ProtocolVersion,
    pub snapshot_cursor: JournalOffset,
    pub craxii: PublicCraxii,
    pub primary_conversation: PublicConversation,
    pub messages: Vec<PublicMessage>,
    pub work_items: Vec<PublicWorkItem>,
    pub unresolved_outcomes: Vec<UnresolvedOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicCraxii {
    pub craxii_id: CraxiiId,
    pub display_name: String,
    pub owner_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicConversation {
    pub conversation_id: ConversationId,
    pub kind: &'static str,
    pub lifecycle: ConversationLifecycle,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicMessage {
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub conversation_sequence: i64,
    pub role: MessageRole,
    pub content: Vec<PublicContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_message_id: Option<ClientMessageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<WorkId>,
    pub committed_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicWorkItem {
    pub work_id: WorkId,
    pub conversation_id: ConversationId,
    pub conversation_work_ordinal: ConversationWorkOrdinal,
    pub state: WorkState,
    pub trigger_message_id: MessageId,
    pub created_at: UtcTimestamp,
    pub queued_at: UtcTimestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<UtcTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<UtcTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<UtcTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<JournalWorkTerminalReason>,
    pub cleanup_pending: bool,
    pub tool_summaries: Vec<PublicToolSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicToolSummary {
    pub tool_execution_id: ToolExecutionId,
    pub tool_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_class: Option<String>,
    pub requested_at: UtcTimestamp,
    pub started_at: Option<UtcTimestamp>,
    pub finished_at: Option<UtcTimestamp>,
    pub outcome_unknown: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedOutcomeKind {
    ProviderOutcomeUnknown,
    ToolOutcomeUnknown,
    CleanupUnconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UnresolvedOutcome {
    pub kind: UnresolvedOutcomeKind,
    pub work_id: WorkId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_execution_id: Option<ToolExecutionId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryKind {
    Durable,
    Ephemeral,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DurableEventEnvelope {
    pub protocol_version: ProtocolVersion,
    pub delivery_kind: DeliveryKind,
    pub event_id: JournalEventId,
    pub cursor: JournalOffset,
    pub event_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_id: Option<WorkId>,
    pub recorded_at: UtcTimestamp,
    pub payload: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SyncCompleteEnvelope {
    pub protocol_version: ProtocolVersion,
    pub delivery_kind: DeliveryKind,
    pub event_type: &'static str,
    pub through_cursor: ReplayCursor,
}

impl SyncCompleteEnvelope {
    #[must_use]
    pub const fn new(through_cursor: ReplayCursor) -> Self {
        Self {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Ephemeral,
            event_type: "sync.complete",
            through_cursor,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolValidationError;

impl fmt::Display for ProtocolValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid protocol request")
    }
}

impl std::error::Error for ProtocolValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    #[test]
    fn cursor_grammar_is_canonical_and_signed_64_bit_safe() {
        for valid in ["0", "1", "9223372036854775807"] {
            let cursor: ReplayCursor = valid.parse().unwrap();
            assert_eq!(cursor.to_string(), valid);
        }
        for invalid in [
            "",
            "-1",
            "+1",
            "00",
            "01",
            " 1",
            "1 ",
            "1.0",
            "1x",
            "9223372036854775808",
            "18446744073709551616",
        ] {
            assert!(invalid.parse::<ReplayCursor>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn requests_are_strict_versioned_and_uuid_v7_only() {
        let valid = format!(
            "{{\"protocol_version\":1,\"client_message_id\":\"{V7}\",\"content\":[{{\"type\":\"text\",\"text\":\"hello\"}}]}}"
        );
        assert!(serde_json::from_str::<MessageRequest>(&valid).is_ok());
        for invalid in [
            valid.replace("\"hello\"}", "\"hello\",\"extra\":true}"),
            valid.replace("\"content\"", "\"extra\":0,\"content\""),
            valid.replace("\"protocol_version\":1", "\"protocol_version\":2"),
            valid.replace(V7, &V7.to_uppercase()),
            valid.replace("\"text\"", "\"binary\""),
        ] {
            assert!(serde_json::from_str::<MessageRequest>(&invalid).is_err());
        }

        let cancellation = format!("{{\"protocol_version\":1,\"client_command_id\":\"{V7}\"}}");
        assert!(serde_json::from_str::<CancellationRequest>(&cancellation).is_ok());
        for invalid in [
            cancellation.replace("}", ",\"unknown\":true}"),
            cancellation.replace("\"protocol_version\":1", "\"protocol_version\":2"),
            cancellation.replace(V7, &V7.to_uppercase()),
        ] {
            assert!(serde_json::from_str::<CancellationRequest>(&invalid).is_err());
        }
    }

    #[test]
    fn request_id_is_server_uuid_v7() {
        let request_id = RequestId::generate().to_string();
        let parsed = Uuid::parse_str(&request_id).unwrap();
        assert_eq!(parsed.get_version_num(), 7);
        assert_eq!(parsed.hyphenated().to_string(), request_id);
    }

    #[test]
    fn language_neutral_protocol_v1_goldens_are_exact_and_safe() {
        let message = include_str!("../tests/fixtures/protocol-v1/message-request.json");
        let cancellation = include_str!("../tests/fixtures/protocol-v1/cancellation-request.json");
        assert!(serde_json::from_str::<MessageRequest>(message).is_ok());
        assert!(serde_json::from_str::<CancellationRequest>(cancellation).is_ok());

        let sync: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/protocol-v1/sync-complete.json"
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(SyncCompleteEnvelope::new(ReplayCursor::try_new(8).unwrap()))
                .unwrap(),
            sync
        );

        for fixture in [
            include_str!("../tests/fixtures/protocol-v1/message-response.json"),
            include_str!("../tests/fixtures/protocol-v1/cancellation-response.json"),
            include_str!("../tests/fixtures/protocol-v1/health.json"),
            include_str!("../tests/fixtures/protocol-v1/error-envelope.json"),
            include_str!("../tests/fixtures/protocol-v1/bootstrap-snapshot.json"),
            include_str!("../tests/fixtures/protocol-v1/durable-events.json"),
        ] {
            let value: Value = serde_json::from_str(fixture).unwrap();
            let encoded = serde_json::to_string(&value).unwrap();
            for forbidden in [
                "stream_id",
                "correlation_id",
                "causation_id",
                "state_version",
                "runtime_instance_id",
                "provider_call_id",
                "request_hash",
                "artifact_path",
            ] {
                assert!(!encoded.contains(forbidden), "{forbidden}");
            }
        }
    }
}
