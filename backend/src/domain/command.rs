//! Stage 9 canonical command identity, hashing, and stable logical receipts.

use std::fmt;

use super::{
    ClientCommandId, ClientMessageId, ConversationId, ConversationWorkOrdinal, MessageContent,
    MessageId, Sha256Digest, WorkId, WorkState,
};

const COMMAND_MAGIC: &[u8] = b"craxii.command";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandKind {
    Message,
    Cancel,
}

impl CommandKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Cancel => "cancel",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CommandValidationError> {
        match value {
            "message" => Ok(Self::Message),
            "cancel" => Ok(Self::Cancel),
            _ => Err(CommandValidationError::new(
                CommandValidationKind::InvalidCommandKind,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CommandHashEncodingVersion;

impl CommandHashEncodingVersion {
    pub const V1: Self = Self;

    #[must_use]
    pub const fn get(self) -> u8 {
        1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandValidationKind {
    InvalidIdempotencyKey,
    InvalidCommandKind,
    KeyIdentityMismatch,
    InvalidReceipt,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CommandValidationError {
    kind: CommandValidationKind,
}

impl CommandValidationError {
    const fn new(kind: CommandValidationKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> CommandValidationKind {
        self.kind
    }
}

impl fmt::Display for CommandValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid command material")
    }
}

impl fmt::Debug for CommandValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandValidationError")
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::error::Error for CommandValidationError {}

/// Canonical UUIDv7 text scoped with a device across every command kind.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn parse_canonical(value: &str) -> Result<Self, CommandValidationError> {
        ClientCommandId::parse_canonical(value)
            .map_err(|_| CommandValidationError::new(CommandValidationKind::InvalidIdempotencyKey))
            .map(|_| Self(value.to_owned()))
    }

    #[must_use]
    pub fn for_message(value: ClientMessageId) -> Self {
        Self(value.to_string())
    }

    #[must_use]
    pub fn for_cancellation(value: ClientCommandId) -> Self {
        Self(value.to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn require_message_id(
        &self,
        client_message_id: ClientMessageId,
    ) -> Result<(), CommandValidationError> {
        if self.0 == client_message_id.to_string() {
            Ok(())
        } else {
            Err(CommandValidationError::new(
                CommandValidationKind::KeyIdentityMismatch,
            ))
        }
    }

    pub fn require_command_id(
        &self,
        client_command_id: ClientCommandId,
    ) -> Result<(), CommandValidationError> {
        if self.0 == client_command_id.to_string() {
            Ok(())
        } else {
            Err(CommandValidationError::new(
                CommandValidationKind::KeyIdentityMismatch,
            ))
        }
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("IdempotencyKey")
            .field(&self.0)
            .finish()
    }
}

/// SHA-256 of one exact versioned semantic command encoding.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CommandRequestHash(Sha256Digest);

impl CommandRequestHash {
    pub fn parse_canonical(value: &str) -> Result<Self, CommandValidationError> {
        Sha256Digest::parse_canonical(value)
            .map(Self)
            .map_err(|_| CommandValidationError::new(CommandValidationKind::InvalidReceipt))
    }

    #[must_use]
    pub(crate) fn canonical_text(self) -> String {
        self.0.to_string()
    }

    #[must_use]
    pub const fn digest(self) -> Sha256Digest {
        self.0
    }

    #[must_use]
    pub fn for_message(
        protocol_version: u64,
        conversation_id: ConversationId,
        client_message_id: ClientMessageId,
        content: &MessageContent,
    ) -> Self {
        let version = protocol_version.to_be_bytes();
        Self(hash_fields(&[
            &version,
            CommandKind::Message.as_str().as_bytes(),
            conversation_id.to_string().as_bytes(),
            client_message_id.to_string().as_bytes(),
            &content.canonical_bytes(),
        ]))
    }

    #[must_use]
    pub fn for_cancellation(
        protocol_version: u64,
        client_command_id: ClientCommandId,
        work_id: WorkId,
    ) -> Self {
        let version = protocol_version.to_be_bytes();
        Self(hash_fields(&[
            &version,
            CommandKind::Cancel.as_str().as_bytes(),
            client_command_id.to_string().as_bytes(),
            work_id.to_string().as_bytes(),
        ]))
    }
}

impl fmt::Debug for CommandRequestHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CommandRequestHash")
            .field(&self.0.to_string())
            .finish()
    }
}

fn hash_fields(fields: &[&[u8]]) -> Sha256Digest {
    let payload_bytes = fields.iter().fold(0_usize, |total, field| {
        total
            .checked_add(8)
            .and_then(|value| value.checked_add(field.len()))
            .expect("validated command fields fit memory")
    });
    let mut bytes = Vec::with_capacity(COMMAND_MAGIC.len() + 1 + payload_bytes);
    bytes.extend_from_slice(COMMAND_MAGIC);
    bytes.push(CommandHashEncodingVersion::V1.get());
    for field in fields {
        let length = u64::try_from(field.len()).expect("command field length fits u64");
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(field);
    }
    Sha256Digest::hash_bytes(&bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationCleanupDisposition {
    NotPending,
    Pending,
}

impl CancellationCleanupDisposition {
    #[must_use]
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageCommandReceipt {
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub work_id: WorkId,
    pub work_ordinal: ConversationWorkOrdinal,
    pub committed_cursor: super::JournalOffset,
}

impl MessageCommandReceipt {
    pub const VERSION: u8 = 1;
    pub const HTTP_STATUS: u16 = 202;

    #[must_use]
    pub const fn work_state(&self) -> WorkState {
        WorkState::Queued
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancellationCommandReceipt {
    pub work_id: WorkId,
    pub resulting_work_state: WorkState,
    pub cleanup: CancellationCleanupDisposition,
    pub committed_cursor: super::JournalOffset,
}

impl CancellationCommandReceipt {
    pub const VERSION: u8 = 1;

    pub fn try_new(
        work_id: WorkId,
        resulting_work_state: WorkState,
        cleanup: CancellationCleanupDisposition,
        committed_cursor: super::JournalOffset,
    ) -> Result<Self, CommandValidationError> {
        let valid = matches!(
            (resulting_work_state, cleanup),
            (
                WorkState::CancelRequested,
                CancellationCleanupDisposition::Pending
            ) | (
                WorkState::Completed
                    | WorkState::Failed
                    | WorkState::Cancelled
                    | WorkState::Interrupted,
                CancellationCleanupDisposition::NotPending
            )
        );
        if !valid {
            return Err(CommandValidationError::new(
                CommandValidationKind::InvalidReceipt,
            ));
        }
        Ok(Self {
            work_id,
            resulting_work_state,
            cleanup,
            committed_cursor,
        })
    }

    #[must_use]
    pub const fn http_status(&self) -> u16 {
        if self.cleanup.is_pending() { 202 } else { 200 }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandOutcome<T> {
    Committed(T),
    Replayed(T),
}

impl<T> CommandOutcome<T> {
    #[must_use]
    pub const fn is_replay(&self) -> bool {
        matches!(self, Self::Replayed(_))
    }

    #[must_use]
    pub fn receipt(&self) -> &T {
        match self {
            Self::Committed(value) | Self::Replayed(value) => value,
        }
    }

    #[must_use]
    pub fn into_receipt(self) -> T {
        match self {
            Self::Committed(value) | Self::Replayed(value) => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContentBlock, JournalOffset};

    const CONVERSATION: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";
    const MESSAGE: &str = "01890f6c-7b3b-7cc0-98f1-2e6f7a8b9c0d";
    const COMMAND: &str = "01890f6c-7b3c-7cc0-98f1-2e6f7a8b9c0d";
    const WORK: &str = "01890f6c-7b3d-7cc0-98f1-2e6f7a8b9c0d";

    fn content() -> MessageContent {
        MessageContent::try_new(vec![ContentBlock::text("héllo 世界").unwrap()]).unwrap()
    }

    #[test]
    fn idempotency_key_is_canonical_uuidv7_and_identity_specific() {
        let message: ClientMessageId = MESSAGE.parse().unwrap();
        let command: ClientCommandId = COMMAND.parse().unwrap();
        assert!(
            IdempotencyKey::for_message(message)
                .require_message_id(message)
                .is_ok()
        );
        assert!(
            IdempotencyKey::for_cancellation(command)
                .require_command_id(command)
                .is_ok()
        );
        assert!(
            IdempotencyKey::for_message(message)
                .require_command_id(command)
                .is_err()
        );
        for value in [
            MESSAGE.to_uppercase(),
            MESSAGE.replace('-', ""),
            "not-a-uuid".into(),
        ] {
            assert!(IdempotencyKey::parse_canonical(&value).is_err());
        }
    }

    #[test]
    fn canonical_hashes_have_permanent_known_vectors_and_semantic_sensitivity() {
        let conversation: ConversationId = CONVERSATION.parse().unwrap();
        let message: ClientMessageId = MESSAGE.parse().unwrap();
        let command: ClientCommandId = COMMAND.parse().unwrap();
        let work: WorkId = WORK.parse().unwrap();
        let message_hash = CommandRequestHash::for_message(1, conversation, message, &content());
        let cancel_hash = CommandRequestHash::for_cancellation(1, command, work);
        assert_eq!(
            message_hash.canonical_text(),
            "eac36ca315e433584cd4b6f149022a44457aa5a4be6344297d369324864286d7"
        );
        assert_eq!(
            cancel_hash.canonical_text(),
            "f1771feb093948c81b3bd7e3f683c17857efebbc70b7084a1fa4249adac47bc2"
        );
        assert_ne!(
            message_hash,
            CommandRequestHash::for_message(2, conversation, message, &content())
        );
        assert_ne!(
            cancel_hash,
            CommandRequestHash::for_cancellation(2, command, work)
        );
        assert_ne!(
            cancel_hash,
            CommandRequestHash::for_cancellation(1, command, WorkId::generate())
        );
    }

    #[test]
    fn cancellation_receipt_state_status_and_cleanup_matrix_is_closed() {
        let work: WorkId = WORK.parse().unwrap();
        let cursor = JournalOffset::try_new(1).unwrap();
        for state in [
            WorkState::Completed,
            WorkState::Failed,
            WorkState::Cancelled,
            WorkState::Interrupted,
        ] {
            let receipt = CancellationCommandReceipt::try_new(
                work,
                state,
                CancellationCleanupDisposition::NotPending,
                cursor,
            )
            .unwrap();
            assert_eq!(receipt.http_status(), 200);
        }
        let active = CancellationCommandReceipt::try_new(
            work,
            WorkState::CancelRequested,
            CancellationCleanupDisposition::Pending,
            cursor,
        )
        .unwrap();
        assert_eq!(active.http_status(), 202);
        assert!(
            CancellationCommandReceipt::try_new(
                work,
                WorkState::CancelRequested,
                CancellationCleanupDisposition::NotPending,
                cursor,
            )
            .is_err()
        );
    }
}
