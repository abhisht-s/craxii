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
    ConversationWorkOrdinal, CraxiiId, DraftId, JournalEventId, JournalOffset,
    JournalWorkTerminalReason, MessageContent, MessageId, MessageRole, ModelInvocationId,
    ToolExecutionId, UtcTimestamp, WorkId, WorkState,
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
pub const MAX_DRAFT_TEXT_BYTES: usize = 262_144;
pub const MAX_DRAFT_EVENTS: u32 = 4_096;
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

/// Server-generated identity for one non-durable live event.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct EphemeralEventId(Uuid);

impl EphemeralEventId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for EphemeralEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(formatter)
    }
}

impl fmt::Debug for EphemeralEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("EphemeralEventId")
            .field(&self.to_string())
            .finish()
    }
}

impl Serialize for EphemeralEventId {
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

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicContentBlock {
    #[serde(rename = "type")]
    pub kind: PublicContentType,
    pub text: String,
}

impl fmt::Debug for PublicContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicContentBlock")
            .field("kind", &self.kind)
            .field("text_bytes", &self.text.len())
            .finish()
    }
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

#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MessageRequest {
    pub protocol_version: ProtocolVersion,
    pub client_message_id: ClientMessageId,
    pub content: Vec<PublicContentBlock>,
}

impl fmt::Debug for MessageRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessageRequest")
            .field("protocol_version", &self.protocol_version)
            .field("client_message_id", &self.client_message_id)
            .field("content_block_count", &self.content.len())
            .field(
                "content_bytes",
                &self
                    .content
                    .iter()
                    .map(|block| block.text.len())
                    .sum::<usize>(),
            )
            .finish()
    }
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

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct BootstrapResponse {
    pub protocol_version: ProtocolVersion,
    pub snapshot_cursor: JournalOffset,
    pub craxii: PublicCraxii,
    pub primary_conversation: PublicConversation,
    pub messages: Vec<PublicMessage>,
    pub work_items: Vec<PublicWorkItem>,
    pub unresolved_outcomes: Vec<UnresolvedOutcome>,
}

impl fmt::Debug for BootstrapResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapResponse")
            .field("snapshot_cursor", &self.snapshot_cursor)
            .field("craxii_id", &self.craxii.craxii_id)
            .field(
                "conversation_id",
                &self.primary_conversation.conversation_id,
            )
            .field("message_count", &self.messages.len())
            .field("work_item_count", &self.work_items.len())
            .field("unresolved_outcome_count", &self.unresolved_outcomes.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct PublicCraxii {
    pub craxii_id: CraxiiId,
    pub display_name: String,
    pub owner_label: String,
}

impl fmt::Debug for PublicCraxii {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicCraxii")
            .field("craxii_id", &self.craxii_id)
            .field("display_name_bytes", &self.display_name.len())
            .field("owner_label_bytes", &self.owner_label.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicConversation {
    pub conversation_id: ConversationId,
    pub kind: &'static str,
    pub lifecycle: ConversationLifecycle,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
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

impl fmt::Debug for PublicMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicMessage")
            .field("message_id", &self.message_id)
            .field("conversation_id", &self.conversation_id)
            .field("conversation_sequence", &self.conversation_sequence)
            .field("role", &self.role)
            .field("content_block_count", &self.content.len())
            .field(
                "content_bytes",
                &self
                    .content
                    .iter()
                    .map(|block| block.text.len())
                    .sum::<usize>(),
            )
            .field("client_message_id", &self.client_message_id)
            .field("work_id", &self.work_id)
            .field("committed_at", &self.committed_at)
            .finish()
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftDeltaKind {
    Text,
    Refusal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftAbandonReason {
    ToolContinuation,
    Superseded,
    Cancelled,
    Failed,
    Interrupted,
    DeliveryLimit,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DraftEventPayload {
    Started {},
    Delta { kind: DraftDeltaKind, text: String },
    Abandoned { reason: DraftAbandonReason },
}

impl fmt::Debug for DraftEventPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started {} => formatter.write_str("DraftEventPayload::Started"),
            Self::Delta { kind, text } => formatter
                .debug_struct("DraftEventPayload::Delta")
                .field("kind", kind)
                .field("text_bytes", &text.len())
                .finish(),
            Self::Abandoned { reason } => formatter
                .debug_struct("DraftEventPayload::Abandoned")
                .field("reason", reason)
                .finish(),
        }
    }
}

/// A lossy, non-replayable event. Its null cursor cannot enter the durable cursor namespace.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct EphemeralDraftEnvelope {
    pub protocol_version: ProtocolVersion,
    pub delivery_kind: DeliveryKind,
    pub event_id: EphemeralEventId,
    pub cursor: Option<ReplayCursor>,
    pub event_type: &'static str,
    pub conversation_id: ConversationId,
    pub work_id: WorkId,
    pub invocation_id: ModelInvocationId,
    pub draft_id: DraftId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_sequence: Option<u32>,
    pub payload: DraftEventPayload,
}

impl fmt::Debug for EphemeralDraftEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EphemeralDraftEnvelope")
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("conversation_id", &self.conversation_id)
            .field("work_id", &self.work_id)
            .field("invocation_id", &self.invocation_id)
            .field("draft_id", &self.draft_id)
            .field("delta_sequence", &self.delta_sequence)
            .field("payload", &self.payload)
            .finish()
    }
}

impl EphemeralDraftEnvelope {
    #[must_use]
    pub fn started(
        conversation_id: ConversationId,
        work_id: WorkId,
        invocation_id: ModelInvocationId,
        draft_id: DraftId,
    ) -> Self {
        Self {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Ephemeral,
            event_id: EphemeralEventId::generate(),
            cursor: None,
            event_type: "assistant.draft_started",
            conversation_id,
            work_id,
            invocation_id,
            draft_id,
            delta_sequence: None,
            payload: DraftEventPayload::Started {},
        }
    }

    #[must_use]
    pub fn delta(
        conversation_id: ConversationId,
        work_id: WorkId,
        invocation_id: ModelInvocationId,
        draft_id: DraftId,
        delta_sequence: u32,
        kind: DraftDeltaKind,
        text: String,
    ) -> Self {
        Self {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Ephemeral,
            event_id: EphemeralEventId::generate(),
            cursor: None,
            event_type: "assistant.draft_delta",
            conversation_id,
            work_id,
            invocation_id,
            draft_id,
            delta_sequence: Some(delta_sequence),
            payload: DraftEventPayload::Delta { kind, text },
        }
    }

    #[must_use]
    pub fn abandoned(
        conversation_id: ConversationId,
        work_id: WorkId,
        invocation_id: ModelInvocationId,
        draft_id: DraftId,
        reason: DraftAbandonReason,
    ) -> Self {
        Self {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Ephemeral,
            event_id: EphemeralEventId::generate(),
            cursor: None,
            event_type: "assistant.draft_abandoned",
            conversation_id,
            work_id,
            invocation_id,
            draft_id,
            delta_sequence: None,
            payload: DraftEventPayload::Abandoned { reason },
        }
    }

    #[must_use]
    pub const fn is_delta(&self) -> bool {
        matches!(self.payload, DraftEventPayload::Delta { .. })
    }
}

#[derive(Clone, PartialEq, Serialize)]
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

impl fmt::Debug for DurableEventEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableEventEnvelope")
            .field("event_id", &self.event_id)
            .field("cursor", &self.cursor)
            .field("event_type", &self.event_type)
            .field("conversation_id", &self.conversation_id)
            .field("work_id", &self.work_id)
            .field("recorded_at", &self.recorded_at)
            .field("payload_bytes", &self.payload.to_string().len())
            .finish()
    }
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
    fn stage23_content_bearing_protocol_debug_is_metadata_only() {
        let message_sentinel = "SENTINEL_USER_MESSAGE_23";
        let draft_sentinel = "SENTINEL_MODEL_DRAFT_23";
        let payload_sentinel = "SENTINEL_DURABLE_PAYLOAD_23";
        let request: MessageRequest = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "client_message_id": V7,
            "content": [{"type": "text", "text": message_sentinel}],
        }))
        .unwrap();
        let draft = EphemeralDraftEnvelope::delta(
            ConversationId::generate(),
            WorkId::generate(),
            ModelInvocationId::generate(),
            DraftId::generate(),
            1,
            DraftDeltaKind::Text,
            draft_sentinel.to_owned(),
        );
        let durable = DurableEventEnvelope {
            protocol_version: ProtocolVersion,
            delivery_kind: DeliveryKind::Durable,
            event_id: JournalEventId::generate(),
            cursor: JournalOffset::try_new(1).unwrap(),
            event_type: "message.accepted",
            conversation_id: Some(ConversationId::generate()),
            work_id: None,
            recorded_at: "2026-09-04T00:00:00.000000Z".parse().unwrap(),
            payload: serde_json::json!({"text": payload_sentinel}),
        };
        let rendered = format!("{request:?}{draft:?}{durable:?}");
        for sentinel in [message_sentinel, draft_sentinel, payload_sentinel] {
            assert!(
                !rendered.contains(sentinel),
                "leaked {sentinel}: {rendered}"
            );
        }
        assert!(rendered.contains(V7));
        assert!(rendered.contains("text_bytes"));
        assert!(rendered.contains("payload_bytes"));
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
            include_str!("../tests/fixtures/protocol-v1/ephemeral-drafts.json"),
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
                "reasoning_text",
                "tool_arguments",
                "provider_opaque",
                "provider_request_id",
                "provider_response_id",
            ] {
                assert!(!encoded.contains(forbidden), "{forbidden}");
            }
        }

        let drafts: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/protocol-v1/ephemeral-drafts.json"
        ))
        .unwrap();
        let drafts = drafts.as_array().unwrap();
        assert!(drafts.iter().all(|event| {
            event["protocol_version"] == 1
                && event["delivery_kind"] == "ephemeral"
                && event["cursor"].is_null()
                && event.get("conversation_id").is_some()
                && event.get("work_id").is_some()
                && event.get("invocation_id").is_some()
                && event.get("draft_id").is_some()
        }));
        assert_eq!(drafts[1]["delta_sequence"], 1);
        assert_eq!(drafts[1]["payload"]["kind"], "text");
        assert_eq!(drafts[4]["payload"]["kind"], "refusal");
    }

    #[test]
    fn stage20_simulated_client_converges_with_duplicates_delays_and_draft_loss() {
        use std::collections::{HashMap, HashSet};

        #[derive(Default)]
        struct Projection {
            active_drafts: HashMap<String, String>,
            finalized_work: HashSet<String>,
            durable_events: HashSet<String>,
            answers: HashMap<String, String>,
        }

        impl Projection {
            fn reconnect(&mut self) {
                self.active_drafts.clear();
            }

            fn apply(&mut self, event: &Value) {
                let work = event["work_id"].as_str().unwrap_or_default().to_owned();
                match event["delivery_kind"].as_str() {
                    Some("durable") => {
                        let event_id = event["event_id"].as_str().unwrap().to_owned();
                        if !self.durable_events.insert(event_id) {
                            return;
                        }
                        if event["event_type"] == "assistant.message_committed" {
                            self.finalized_work.insert(work.clone());
                            self.active_drafts.remove(&work);
                            self.answers.insert(
                                work,
                                event["payload"]["content"][0]["text"]
                                    .as_str()
                                    .unwrap()
                                    .to_owned(),
                            );
                        }
                    }
                    Some("ephemeral") if !self.finalized_work.contains(&work) => {
                        if event["event_type"] == "assistant.draft_started" {
                            self.active_drafts
                                .insert(work, event["draft_id"].as_str().unwrap().to_owned());
                        } else if event["event_type"] == "assistant.draft_abandoned" {
                            self.active_drafts.remove(&work);
                        }
                    }
                    _ => {}
                }
            }
        }

        let drafts: Vec<Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/protocol-v1/ephemeral-drafts.json"
        ))
        .unwrap();
        let work = drafts[0]["work_id"].as_str().unwrap();
        let committed = serde_json::json!({
            "protocol_version": 1,
            "delivery_kind": "durable",
            "event_id": "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa019",
            "cursor": 20,
            "event_type": "assistant.message_committed",
            "conversation_id": drafts[0]["conversation_id"],
            "work_id": work,
            "recorded_at": "2026-08-28T00:00:03.000000Z",
            "payload": {
                "message_id": "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa020",
                "role": "assistant",
                "content": [{"type": "text", "text": "canonical answer"}],
                "work_id": work,
                "committed_at": "2026-08-28T00:00:03.000000Z"
            }
        });

        for delivery in [
            vec![drafts[0].clone(), drafts[1].clone(), committed.clone()],
            vec![drafts[0].clone(), committed.clone(), drafts[1].clone()],
            vec![committed.clone(), committed.clone(), drafts[0].clone()],
        ] {
            let mut projection = Projection::default();
            for event in delivery {
                projection.apply(&event);
            }
            projection.reconnect();
            projection.apply(&committed);
            assert!(projection.active_drafts.is_empty());
            assert_eq!(projection.answers.get(work).unwrap(), "canonical answer");
            assert_eq!(projection.answers.len(), 1);
        }
    }
}
