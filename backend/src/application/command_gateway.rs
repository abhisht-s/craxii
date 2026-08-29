//! Transport-neutral command admission around the Stage 9 CommandService.

use std::fmt;
use std::sync::Arc;

use crate::application::command_service::{
    AcceptMessageCommand, CancelWorkCommand, CommandPostCommit, CommandService, CommandServiceError,
};
use crate::application::transport::{AdmissionClosed, MutationAdmission};
use crate::bootstrap::health::{Health, HealthState};
use crate::domain::{
    AuthenticatedDevice, CancellationCommandReceipt, ClientCommandId, ClientMessageId,
    CommandOutcome, ConversationId, IdempotencyKey, MessageCommandReceipt, MessageContent,
    UtcTimestamp, WorkId,
};
use crate::ports::clock::Clock;
use crate::ports::state_store::CommandStateStore;

pub struct CommandGateway<S, C, H> {
    store: Arc<S>,
    clock: Arc<C>,
    health: Health,
    admission: MutationAdmission,
    post_commit: H,
}

impl<S, C, H> CommandGateway<S, C, H>
where
    S: CommandStateStore,
    C: Clock,
    H: CommandPostCommit + Clone,
{
    #[must_use]
    pub const fn new(
        store: Arc<S>,
        clock: Arc<C>,
        health: Health,
        admission: MutationAdmission,
        post_commit: H,
    ) -> Self {
        Self {
            store,
            clock,
            health,
            admission,
            post_commit,
        }
    }

    pub async fn accept_message(
        &self,
        authenticated: AuthenticatedDevice,
        conversation_id: ConversationId,
        idempotency_key: IdempotencyKey,
        client_message_id: ClientMessageId,
        content: MessageContent,
    ) -> Result<CommandOutcome<MessageCommandReceipt>, CommandGatewayError> {
        if self.health.snapshot().state() != HealthState::Ready {
            return Err(CommandGatewayError::unavailable());
        }
        let _permit = self.admission.admit().await.map_err(map_admission)?;
        let accepted_at = now(self.clock.as_ref())?;
        CommandService::with_post_commit(self.store.as_ref(), self.post_commit.clone())
            .accept_message(
                authenticated,
                AcceptMessageCommand {
                    idempotency_key,
                    client_message_id,
                    conversation_id,
                    content,
                    accepted_at,
                },
            )
            .await
            .map_err(CommandGatewayError::command)
    }

    pub async fn cancel_work(
        &self,
        authenticated: AuthenticatedDevice,
        work_id: WorkId,
        idempotency_key: IdempotencyKey,
        client_command_id: ClientCommandId,
    ) -> Result<CommandOutcome<CancellationCommandReceipt>, CommandGatewayError> {
        if !matches!(
            self.health.snapshot().state(),
            HealthState::LiveUnready | HealthState::Ready
        ) {
            return Err(CommandGatewayError::unavailable());
        }
        let _permit = self.admission.admit().await.map_err(map_admission)?;
        let requested_at = now(self.clock.as_ref())?;
        CommandService::with_post_commit(self.store.as_ref(), self.post_commit.clone())
            .cancel_work(
                authenticated,
                CancelWorkCommand {
                    idempotency_key,
                    client_command_id,
                    work_id,
                    requested_at,
                },
            )
            .await
            .map_err(CommandGatewayError::command)
    }
}

fn now(clock: &dyn Clock) -> Result<UtcTimestamp, CommandGatewayError> {
    UtcTimestamp::from_offset_datetime(clock.utc_now().map_err(|_| CommandGatewayError::clock())?)
        .map_err(|_| CommandGatewayError::clock())
}

fn map_admission(_: AdmissionClosed) -> CommandGatewayError {
    CommandGatewayError::unavailable()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandGatewayErrorKind {
    Unavailable,
    Clock,
    Command,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CommandGatewayError {
    kind: CommandGatewayErrorKind,
    command: Option<crate::application::command_service::CommandServiceErrorKind>,
}

impl CommandGatewayError {
    const fn unavailable() -> Self {
        Self {
            kind: CommandGatewayErrorKind::Unavailable,
            command: None,
        }
    }

    const fn clock() -> Self {
        Self {
            kind: CommandGatewayErrorKind::Clock,
            command: None,
        }
    }

    const fn command(error: CommandServiceError) -> Self {
        Self {
            kind: CommandGatewayErrorKind::Command,
            command: Some(error.kind()),
        }
    }

    #[must_use]
    pub const fn kind(self) -> CommandGatewayErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn command_kind(
        self,
    ) -> Option<crate::application::command_service::CommandServiceErrorKind> {
        self.command
    }
}

impl fmt::Display for CommandGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            CommandGatewayErrorKind::Unavailable => "command admission unavailable",
            CommandGatewayErrorKind::Clock => "command clock failure",
            CommandGatewayErrorKind::Command => "command service failure",
        })
    }
}

impl fmt::Debug for CommandGatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for CommandGatewayError {}
