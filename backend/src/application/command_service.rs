//! Transport-free Stage 9 command orchestration.

use std::fmt;

use crate::bootstrap::compatibility::PROTOCOL_VERSION;
use crate::domain::{
    AuthenticatedDevice, CancellationCommandReceipt, ClientCommandId, ClientMessageId,
    CommandHashEncodingVersion, CommandOutcome, CommandRequestHash, ConversationId, IdempotencyKey,
    JournalEventId, MessageCommandReceipt, MessageContent, MessageId, UtcTimestamp, WorkId,
};
use crate::ports::state_store::{
    AcceptUserMessageRequest, CommandStateStore, MessageCommandCandidates,
    RequestCancellationRequest, StateStoreErrorKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandServiceErrorKind {
    IdempotencyConflict,
    CommandValidationFailed,
    TargetNotFound,
    StorageInconsistent,
    StorageFailure,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CommandServiceError {
    kind: CommandServiceErrorKind,
}

impl CommandServiceError {
    const fn new(kind: CommandServiceErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> CommandServiceErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self.kind {
            CommandServiceErrorKind::IdempotencyConflict => "idempotency_conflict",
            CommandServiceErrorKind::CommandValidationFailed => "command_validation_failed",
            CommandServiceErrorKind::TargetNotFound => "target_not_found",
            CommandServiceErrorKind::StorageInconsistent => "storage_inconsistent",
            CommandServiceErrorKind::StorageFailure => "storage_failure",
        }
    }
}

impl fmt::Display for CommandServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl fmt::Debug for CommandServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CommandServiceError {}

pub struct AcceptMessageCommand {
    pub idempotency_key: IdempotencyKey,
    pub client_message_id: ClientMessageId,
    pub conversation_id: ConversationId,
    pub content: MessageContent,
    pub accepted_at: UtcTimestamp,
}

pub struct CancelWorkCommand {
    pub idempotency_key: IdempotencyKey,
    pub client_command_id: ClientCommandId,
    pub work_id: WorkId,
    pub requested_at: UtcTimestamp,
}

pub trait CommandPostCommit: Send + Sync {
    fn message_committed(&self, work_id: WorkId);
    fn active_cancellation_committed(&self, work_id: WorkId);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopCommandPostCommit;

impl CommandPostCommit for NoopCommandPostCommit {
    fn message_committed(&self, _: WorkId) {}
    fn active_cancellation_committed(&self, _: WorkId) {}
}

pub struct CommandService<'a, S, H = NoopCommandPostCommit> {
    store: &'a S,
    post_commit: H,
}

impl<'a, S> CommandService<'a, S, NoopCommandPostCommit>
where
    S: CommandStateStore,
{
    #[must_use]
    pub const fn new(store: &'a S) -> Self {
        Self {
            store,
            post_commit: NoopCommandPostCommit,
        }
    }
}

impl<'a, S, H> CommandService<'a, S, H>
where
    S: CommandStateStore,
    H: CommandPostCommit,
{
    #[must_use]
    pub const fn with_post_commit(store: &'a S, post_commit: H) -> Self {
        Self { store, post_commit }
    }

    pub async fn accept_message(
        &self,
        authenticated: AuthenticatedDevice,
        command: AcceptMessageCommand,
    ) -> Result<CommandOutcome<MessageCommandReceipt>, CommandServiceError> {
        command
            .idempotency_key
            .require_message_id(command.client_message_id)
            .map_err(|_| {
                CommandServiceError::new(CommandServiceErrorKind::CommandValidationFailed)
            })?;
        let request_hash = CommandRequestHash::for_message(
            PROTOCOL_VERSION,
            command.conversation_id,
            command.client_message_id,
            &command.content,
        );
        let outcome = self
            .store
            .accept_user_message_and_create_work(AcceptUserMessageRequest {
                client_message_id: command.client_message_id,
                device_id: authenticated.device_id(),
                idempotency_key: command.idempotency_key,
                request_hash,
                hash_version: CommandHashEncodingVersion::V1,
                conversation_id: command.conversation_id,
                content: command.content,
                accepted_at: command.accepted_at,
                candidates: MessageCommandCandidates {
                    message_id: MessageId::generate(),
                    work_id: WorkId::generate(),
                    acceptance_event_id: JournalEventId::generate(),
                    queued_event_id: JournalEventId::generate(),
                },
            })
            .await
            .map_err(map_store_error)?;
        if let CommandOutcome::Committed(receipt) = &outcome {
            #[cfg(feature = "test-failpoints")]
            crate::test_failpoints::reach(
                crate::test_failpoints::PhysicalHook::AfterMessageTransactionCommit,
            );
            self.post_commit.message_committed(receipt.work_id);
        }
        Ok(outcome)
    }

    pub async fn cancel_work(
        &self,
        authenticated: AuthenticatedDevice,
        command: CancelWorkCommand,
    ) -> Result<CommandOutcome<CancellationCommandReceipt>, CommandServiceError> {
        command
            .idempotency_key
            .require_command_id(command.client_command_id)
            .map_err(|_| {
                CommandServiceError::new(CommandServiceErrorKind::CommandValidationFailed)
            })?;
        let request_hash = CommandRequestHash::for_cancellation(
            PROTOCOL_VERSION,
            command.client_command_id,
            command.work_id,
        );
        let outcome = self
            .store
            .request_cancellation(RequestCancellationRequest {
                client_command_id: command.client_command_id,
                device_id: authenticated.device_id(),
                idempotency_key: command.idempotency_key,
                request_hash,
                hash_version: CommandHashEncodingVersion::V1,
                work_id: command.work_id,
                requested_at: command.requested_at,
                event_id: JournalEventId::generate(),
            })
            .await
            .map_err(map_store_error)?;
        if let CommandOutcome::Committed(receipt) = &outcome
            && receipt.resulting_work_state == crate::domain::WorkState::CancelRequested
        {
            #[cfg(feature = "test-failpoints")]
            crate::test_failpoints::reach(
                crate::test_failpoints::PhysicalHook::AfterCancelRequestedCommit,
            );
            self.post_commit
                .active_cancellation_committed(receipt.work_id);
        }
        Ok(outcome)
    }
}

fn map_store_error(error: crate::ports::state_store::StateStoreError) -> CommandServiceError {
    let kind = match error.kind() {
        StateStoreErrorKind::IdempotencyConflict => CommandServiceErrorKind::IdempotencyConflict,
        StateStoreErrorKind::TargetNotFound => CommandServiceErrorKind::TargetNotFound,
        StateStoreErrorKind::InternalInvariant | StateStoreErrorKind::StateConflict => {
            CommandServiceErrorKind::StorageInconsistent
        }
        StateStoreErrorKind::Storage => CommandServiceErrorKind::StorageFailure,
    };
    CommandServiceError::new(kind)
}
