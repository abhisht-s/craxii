//! SQLite persistence for provider-neutral channel identity foundations.

use sqlx::Row;

use crate::domain::{
    ChannelAccount, ChannelAccountId, ChannelAccountLifecycle, ChannelProviderId,
    ConversationBinding, ConversationBindingId, ConversationBindingLifecycle, ExternalAccountId,
    ExternalConversationId, ExternalEventId, ExternalIdentity, ExternalIdentityId,
    ExternalIdentityLifecycle, ExternalMessageId, ExternalSubjectId, ExternalThreadId,
    InboundClassification, InboundDelivery, InboundDeliveryId, InboundReceiptState,
};
use crate::ports::channel_identity::{
    ChannelIdentityFuture, ChannelIdentityStore, ChannelIdentityStoreError,
    ChannelIdentityStoreErrorKind, DisableChannelAccountOutcome,
    EnsureChannelAccountAndIdentityRequest, EnsureConversationBindingRequest,
    EnsuredChannelAccountAndIdentity,
};

use super::transaction::WriteTransaction;
use super::{SqliteAdapterError, SqliteRuntime};

#[derive(Clone, Debug)]
pub struct SqliteChannelIdentityStore {
    runtime: SqliteRuntime,
}

impl SqliteChannelIdentityStore {
    #[must_use]
    pub const fn new(runtime: SqliteRuntime) -> Self {
        Self { runtime }
    }
}

fn inconsistent() -> ChannelIdentityStoreError {
    ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Inconsistent)
}

fn map_sqlx(error: sqlx::Error) -> ChannelIdentityStoreError {
    if error.as_database_error().is_some() {
        ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Conflict)
    } else {
        ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
    }
}

fn map_transaction(error: SqliteAdapterError) -> ChannelIdentityStoreError {
    match error.kind() {
        super::SqliteFailureKind::BusyOrLocked | super::SqliteFailureKind::Storage => {
            ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
        }
        _ => inconsistent(),
    }
}

fn conflict() -> ChannelIdentityStoreError {
    ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Conflict)
}

fn decode_channel_account(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ChannelAccount, ChannelIdentityStoreError> {
    Ok(ChannelAccount {
        channel_account_id: parse_id(&row.try_get::<String, _>("channel_account_id")?)?,
        craxii_id: parse_id(&row.try_get::<String, _>("craxii_id")?)?,
        provider_id: ChannelProviderId::try_new(row.try_get::<String, _>("provider_key")?)
            .map_err(|_| inconsistent())?,
        external_account_id: ExternalAccountId::try_new(
            row.try_get::<String, _>("external_account_id")?,
        )
        .map_err(|_| inconsistent())?,
        lifecycle: match row.try_get::<String, _>("lifecycle_state")?.as_str() {
            "active" => ChannelAccountLifecycle::Active,
            "disabled" => ChannelAccountLifecycle::Disabled,
            _ => return Err(inconsistent()),
        },
        created_at: parse_id(&row.try_get::<String, _>("created_at")?)?,
        disabled_at: row
            .try_get::<Option<String>, _>("disabled_at")?
            .map(|value| parse_id(&value))
            .transpose()?,
    })
}

async fn disable_channel_account_inner(
    runtime: &SqliteRuntime,
    id: ChannelAccountId,
    disabled_at: crate::domain::UtcTimestamp,
) -> Result<DisableChannelAccountOutcome, ChannelIdentityStoreError> {
    let mut transaction = WriteTransaction::begin(runtime, "disable_channel_account")
        .await
        .map_err(map_transaction)?;
    let row = sqlx::query("SELECT * FROM channel_accounts WHERE channel_account_id = ?")
        .bind(id.to_string())
        .fetch_optional(transaction.connection())
        .await
        .map_err(map_sqlx)?;
    let Some(row) = row else {
        transaction.commit().await.map_err(map_transaction)?;
        return Ok(DisableChannelAccountOutcome::NotFound);
    };
    let mut account = decode_channel_account(&row)?;
    match account.lifecycle {
        ChannelAccountLifecycle::Disabled => {
            if account
                .disabled_at
                .is_none_or(|persisted| persisted < account.created_at)
            {
                return Err(inconsistent());
            }
            transaction.commit().await.map_err(map_transaction)?;
            Ok(DisableChannelAccountOutcome::AlreadyDisabled(account))
        }
        ChannelAccountLifecycle::Active => {
            if account.disabled_at.is_some() || disabled_at < account.created_at {
                return Err(conflict());
            }
            let changed = sqlx::query(
                "UPDATE channel_accounts SET lifecycle_state = 'disabled', disabled_at = ? \
                 WHERE channel_account_id = ? AND lifecycle_state = 'active' AND disabled_at IS NULL",
            )
            .bind(disabled_at.to_string())
            .bind(id.to_string())
            .execute(transaction.connection())
            .await
            .map_err(map_sqlx)?;
            if changed.rows_affected() != 1 {
                return Err(conflict());
            }
            account.lifecycle = ChannelAccountLifecycle::Disabled;
            account.disabled_at = Some(disabled_at);
            transaction.commit().await.map_err(map_transaction)?;
            Ok(DisableChannelAccountOutcome::Disabled(account))
        }
    }
}

async fn ensure_account_and_identity_inner(
    runtime: &SqliteRuntime,
    request: EnsureChannelAccountAndIdentityRequest,
) -> Result<EnsuredChannelAccountAndIdentity, ChannelIdentityStoreError> {
    let mut transaction = WriteTransaction::begin(runtime, "ensure_channel_account_and_identity")
        .await
        .map_err(map_transaction)?;
    let accounts = sqlx::query(
        "SELECT channel_account_id, craxii_id, provider_key, external_account_id, lifecycle_state \
         FROM channel_accounts WHERE channel_account_id = ? OR \
         (craxii_id = ? AND provider_key = ? AND external_account_id = ?) \
         ORDER BY channel_account_id ASC",
    )
    .bind(request.channel_account_id.to_string())
    .bind(request.craxii_id.to_string())
    .bind(request.provider_id.as_str())
    .bind(request.external_account_id.as_str())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    match accounts.as_slice() {
        [] => {
            let principal = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM craxii_principals WHERE craxii_id = ?",
            )
            .bind(request.craxii_id.to_string())
            .fetch_one(transaction.connection())
            .await
            .map_err(map_sqlx)?;
            if principal != 1 {
                return Err(inconsistent());
            }
            sqlx::query(
                "INSERT INTO channel_accounts \
                 (channel_account_id, craxii_id, provider_key, external_account_id, \
                  lifecycle_state, created_at, disabled_at) \
                 VALUES (?, ?, ?, ?, 'active', ?, NULL)",
            )
            .bind(request.channel_account_id.to_string())
            .bind(request.craxii_id.to_string())
            .bind(request.provider_id.as_str())
            .bind(request.external_account_id.as_str())
            .bind(request.created_at.to_string())
            .execute(transaction.connection())
            .await
            .map_err(map_sqlx)?;
        }
        [account]
            if account.try_get::<String, _>("channel_account_id")?
                == request.channel_account_id.to_string()
                && account.try_get::<String, _>("craxii_id")? == request.craxii_id.to_string()
                && account.try_get::<String, _>("provider_key")?
                    == request.provider_id.as_str()
                && account.try_get::<String, _>("external_account_id")?
                    == request.external_account_id.as_str()
                && account.try_get::<String, _>("lifecycle_state")? == "active" => {}
        [_] | [_, ..] => return Err(conflict()),
    }

    let identities = sqlx::query(
        "SELECT external_identity_id, channel_account_id, craxii_id, user_id, \
                external_subject_id, lifecycle_state \
         FROM external_identities WHERE external_identity_id = ? OR \
         (channel_account_id = ? AND external_subject_id = ?) \
         ORDER BY external_identity_id ASC",
    )
    .bind(request.proposed_external_identity_id.to_string())
    .bind(request.channel_account_id.to_string())
    .bind(request.owner_external_subject_id.as_str())
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let external_identity_id = match identities.as_slice() {
        [] => {
            let user =
                sqlx::query("SELECT craxii_id, lifecycle_state FROM users WHERE user_id = ?")
                    .bind(request.owner_user_id.to_string())
                    .fetch_optional(transaction.connection())
                    .await
                    .map_err(map_sqlx)?
                    .ok_or_else(inconsistent)?;
            if user.try_get::<String, _>("craxii_id")? != request.craxii_id.to_string()
                || user.try_get::<String, _>("lifecycle_state")? != "active"
            {
                return Err(conflict());
            }
            sqlx::query(
                "INSERT INTO external_identities \
                 (external_identity_id, channel_account_id, craxii_id, user_id, \
                  external_subject_id, lifecycle_state, created_at, revoked_at) \
                 VALUES (?, ?, ?, ?, ?, 'active', ?, NULL)",
            )
            .bind(request.proposed_external_identity_id.to_string())
            .bind(request.channel_account_id.to_string())
            .bind(request.craxii_id.to_string())
            .bind(request.owner_user_id.to_string())
            .bind(request.owner_external_subject_id.as_str())
            .bind(request.created_at.to_string())
            .execute(transaction.connection())
            .await
            .map_err(map_sqlx)?;
            request.proposed_external_identity_id
        }
        [identity]
            if identity.try_get::<String, _>("channel_account_id")?
                == request.channel_account_id.to_string()
                && identity.try_get::<String, _>("craxii_id")? == request.craxii_id.to_string()
                && identity.try_get::<String, _>("user_id")?
                    == request.owner_user_id.to_string()
                && identity.try_get::<String, _>("external_subject_id")?
                    == request.owner_external_subject_id.as_str()
                && identity.try_get::<String, _>("lifecycle_state")? == "active" =>
        {
            identity
                .try_get::<String, _>("external_identity_id")?
                .parse()
                .map_err(|_| inconsistent())?
        }
        [_] | [_, ..] => return Err(conflict()),
    };
    transaction.commit().await.map_err(map_transaction)?;
    Ok(EnsuredChannelAccountAndIdentity {
        channel_account_id: request.channel_account_id,
        external_identity_id,
    })
}

async fn ensure_binding_inner(
    runtime: &SqliteRuntime,
    request: EnsureConversationBindingRequest,
) -> Result<ConversationBinding, ChannelIdentityStoreError> {
    let mut transaction = WriteTransaction::begin(runtime, "ensure_first_conversation_binding")
        .await
        .map_err(map_transaction)?;
    let topology = sqlx::query(
        "SELECT a.craxii_id AS account_craxii_id, a.lifecycle_state AS account_lifecycle, \
                i.channel_account_id AS identity_account_id, i.craxii_id AS identity_craxii_id, \
                i.user_id AS identity_user_id, i.lifecycle_state AS identity_lifecycle, \
                u.craxii_id AS user_craxii_id, u.lifecycle_state AS user_lifecycle, \
                c.craxii_id AS conversation_craxii_id, c.owner_user_id, \
                c.kind, c.lifecycle_state AS conversation_lifecycle \
         FROM channel_accounts a \
         LEFT JOIN external_identities i ON i.external_identity_id = ? \
         LEFT JOIN users u ON u.user_id = ? \
         LEFT JOIN conversations c ON c.conversation_id = ? \
         WHERE a.channel_account_id = ?",
    )
    .bind(request.external_identity_id.to_string())
    .bind(request.user_id.to_string())
    .bind(request.conversation_id.to_string())
    .bind(request.channel_account_id.to_string())
    .fetch_optional(transaction.connection())
    .await
    .map_err(map_sqlx)?
    .ok_or_else(inconsistent)?;
    let exact_topology = topology.try_get::<String, _>("account_craxii_id")?
        == request.craxii_id.to_string()
        && topology.try_get::<String, _>("account_lifecycle")? == "active"
        && topology
            .try_get::<Option<String>, _>("identity_account_id")?
            .as_deref()
            == Some(request.channel_account_id.to_string().as_str())
        && topology
            .try_get::<Option<String>, _>("identity_craxii_id")?
            .as_deref()
            == Some(request.craxii_id.to_string().as_str())
        && topology
            .try_get::<Option<String>, _>("identity_user_id")?
            .as_deref()
            == Some(request.user_id.to_string().as_str())
        && topology
            .try_get::<Option<String>, _>("identity_lifecycle")?
            .as_deref()
            == Some("active")
        && topology
            .try_get::<Option<String>, _>("user_craxii_id")?
            .as_deref()
            == Some(request.craxii_id.to_string().as_str())
        && topology
            .try_get::<Option<String>, _>("user_lifecycle")?
            .as_deref()
            == Some("active")
        && topology
            .try_get::<Option<String>, _>("conversation_craxii_id")?
            .as_deref()
            == Some(request.craxii_id.to_string().as_str())
        && topology
            .try_get::<Option<String>, _>("owner_user_id")?
            .as_deref()
            == Some(request.user_id.to_string().as_str())
        && topology.try_get::<Option<String>, _>("kind")?.as_deref() == Some("primary")
        && topology
            .try_get::<Option<String>, _>("conversation_lifecycle")?
            .as_deref()
            == Some("active");
    if !exact_topology {
        return Err(conflict());
    }

    let bindings = sqlx::query(
        "SELECT * FROM conversation_bindings WHERE conversation_binding_id = ? OR \
         (channel_account_id = ? AND external_identity_id = ?) OR \
         (channel_account_id = ? AND external_conversation_id = ? \
          AND COALESCE(external_thread_id, '') = COALESCE(?, '')) \
         ORDER BY conversation_binding_id ASC",
    )
    .bind(request.proposed_conversation_binding_id.to_string())
    .bind(request.channel_account_id.to_string())
    .bind(request.external_identity_id.to_string())
    .bind(request.channel_account_id.to_string())
    .bind(request.external_conversation_id.as_str())
    .bind(request.external_thread_id.as_ref().map(|id| id.as_str()))
    .fetch_all(transaction.connection())
    .await
    .map_err(map_sqlx)?;
    let binding = match bindings.as_slice() {
        [] => {
            sqlx::query(
                "INSERT INTO conversation_bindings \
                 (conversation_binding_id, channel_account_id, external_identity_id, craxii_id, \
                  user_id, conversation_id, external_conversation_id, external_thread_id, \
                  lifecycle_state, created_at, revoked_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, NULL)",
            )
            .bind(request.proposed_conversation_binding_id.to_string())
            .bind(request.channel_account_id.to_string())
            .bind(request.external_identity_id.to_string())
            .bind(request.craxii_id.to_string())
            .bind(request.user_id.to_string())
            .bind(request.conversation_id.to_string())
            .bind(request.external_conversation_id.as_str())
            .bind(request.external_thread_id.as_ref().map(|id| id.as_str()))
            .bind(request.created_at.to_string())
            .execute(transaction.connection())
            .await
            .map_err(map_sqlx)?;
            ConversationBinding {
                conversation_binding_id: request.proposed_conversation_binding_id,
                channel_account_id: request.channel_account_id,
                external_identity_id: request.external_identity_id,
                craxii_id: request.craxii_id,
                user_id: request.user_id,
                conversation_id: request.conversation_id,
                external_conversation_id: request.external_conversation_id,
                external_thread_id: request.external_thread_id,
                lifecycle: ConversationBindingLifecycle::Active,
                created_at: request.created_at,
                revoked_at: None,
            }
        }
        [row]
            if row.try_get::<String, _>("channel_account_id")?
                == request.channel_account_id.to_string()
                && row.try_get::<String, _>("external_identity_id")?
                    == request.external_identity_id.to_string()
                && row.try_get::<String, _>("craxii_id")? == request.craxii_id.to_string()
                && row.try_get::<String, _>("user_id")? == request.user_id.to_string()
                && row.try_get::<String, _>("conversation_id")?
                    == request.conversation_id.to_string()
                && row.try_get::<String, _>("external_conversation_id")?
                    == request.external_conversation_id.as_str()
                && row
                    .try_get::<Option<String>, _>("external_thread_id")?
                    .as_deref()
                    == request
                        .external_thread_id
                        .as_ref()
                        .map(ExternalThreadId::as_str)
                && row.try_get::<String, _>("lifecycle_state")? == "active" =>
        {
            ConversationBinding {
                conversation_binding_id: row
                    .try_get::<String, _>("conversation_binding_id")?
                    .parse()
                    .map_err(|_| inconsistent())?,
                channel_account_id: request.channel_account_id,
                external_identity_id: request.external_identity_id,
                craxii_id: request.craxii_id,
                user_id: request.user_id,
                conversation_id: request.conversation_id,
                external_conversation_id: request.external_conversation_id,
                external_thread_id: request.external_thread_id,
                lifecycle: ConversationBindingLifecycle::Active,
                created_at: row
                    .try_get::<String, _>("created_at")?
                    .parse()
                    .map_err(|_| inconsistent())?,
                revoked_at: None,
            }
        }
        [_] | [_, ..] => return Err(conflict()),
    };
    transaction.commit().await.map_err(map_transaction)?;
    Ok(binding)
}

impl From<sqlx::Error> for ChannelIdentityStoreError {
    fn from(error: sqlx::Error) -> Self {
        map_sqlx(error)
    }
}

fn parse_id<T>(value: &str) -> Result<T, ChannelIdentityStoreError>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| inconsistent())
}

fn lifecycle_shape(valid: bool) -> Result<(), ChannelIdentityStoreError> {
    valid.then_some(()).ok_or_else(inconsistent)
}

fn validate_inbound_shape(value: &InboundDelivery) -> Result<(), ChannelIdentityStoreError> {
    let no_resolution = value.external_identity_id.is_none()
        && value.conversation_binding_id.is_none()
        && value.user_id.is_none()
        && value.conversation_id.is_none()
        && value.message_id.is_none()
        && value.work_id.is_none()
        && value.control_target_work_id.is_none();
    let valid = match (value.receipt_state, value.classification) {
        (InboundReceiptState::Received, None) => no_resolution && value.classified_at.is_none(),
        (InboundReceiptState::Classified, Some(InboundClassification::Message)) => {
            value.external_message_id.is_some()
                && value.external_identity_id.is_some()
                && value.conversation_binding_id.is_some()
                && value.user_id.is_some()
                && value.conversation_id.is_some()
                && value.message_id.is_some()
                && value.work_id.is_some()
                && value.control_target_work_id.is_none()
                && value.classified_at.is_some()
        }
        (InboundReceiptState::Classified, Some(InboundClassification::Control)) => {
            value.external_identity_id.is_some()
                && value.conversation_binding_id.is_some()
                && value.user_id.is_some()
                && value.conversation_id.is_some()
                && value.message_id.is_none()
                && value.work_id.is_none()
                && value.classified_at.is_some()
        }
        (
            InboundReceiptState::Classified,
            Some(InboundClassification::Rejected | InboundClassification::Unsupported),
        ) => no_resolution && value.classified_at.is_some(),
        _ => false,
    };
    lifecycle_shape(valid)
}

impl ChannelIdentityStore for SqliteChannelIdentityStore {
    fn ensure_channel_account_and_identity(
        &self,
        request: EnsureChannelAccountAndIdentityRequest,
    ) -> ChannelIdentityFuture<'_, EnsuredChannelAccountAndIdentity> {
        Box::pin(async move { ensure_account_and_identity_inner(&self.runtime, request).await })
    }

    fn ensure_first_conversation_binding(
        &self,
        request: EnsureConversationBindingRequest,
    ) -> ChannelIdentityFuture<'_, ConversationBinding> {
        Box::pin(async move { ensure_binding_inner(&self.runtime, request).await })
    }

    fn persist_channel_account(&self, account: ChannelAccount) -> ChannelIdentityFuture<'_, ()> {
        Box::pin(async move {
            lifecycle_shape(match account.lifecycle {
                ChannelAccountLifecycle::Active => account.disabled_at.is_none(),
                ChannelAccountLifecycle::Disabled => account
                    .disabled_at
                    .is_some_and(|disabled| disabled >= account.created_at),
            })?;
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            sqlx::query(
                "INSERT INTO channel_accounts \
                 (channel_account_id, craxii_id, provider_key, external_account_id, \
                  lifecycle_state, created_at, disabled_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(account.channel_account_id.to_string())
            .bind(account.craxii_id.to_string())
            .bind(account.provider_id.as_str())
            .bind(account.external_account_id.as_str())
            .bind(match account.lifecycle {
                ChannelAccountLifecycle::Active => "active",
                ChannelAccountLifecycle::Disabled => "disabled",
            })
            .bind(account.created_at.to_string())
            .bind(account.disabled_at.map(|at| at.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            Ok(())
        })
    }

    fn load_channel_account(
        &self,
        id: ChannelAccountId,
    ) -> ChannelIdentityFuture<'_, Option<ChannelAccount>> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            let row = sqlx::query("SELECT * FROM channel_accounts WHERE channel_account_id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sqlx)?;
            row.map(|row| decode_channel_account(&row)).transpose()
        })
    }

    fn disable_channel_account(
        &self,
        id: ChannelAccountId,
        disabled_at: crate::domain::UtcTimestamp,
    ) -> ChannelIdentityFuture<'_, DisableChannelAccountOutcome> {
        Box::pin(async move { disable_channel_account_inner(&self.runtime, id, disabled_at).await })
    }

    fn persist_external_identity(
        &self,
        identity: ExternalIdentity,
    ) -> ChannelIdentityFuture<'_, ()> {
        Box::pin(async move {
            lifecycle_shape(match identity.lifecycle {
                ExternalIdentityLifecycle::Active => identity.revoked_at.is_none(),
                ExternalIdentityLifecycle::Revoked => identity
                    .revoked_at
                    .is_some_and(|revoked| revoked >= identity.created_at),
            })?;
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            sqlx::query(
                "INSERT INTO external_identities \
                 (external_identity_id, channel_account_id, craxii_id, user_id, \
                  external_subject_id, lifecycle_state, created_at, revoked_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(identity.external_identity_id.to_string())
            .bind(identity.channel_account_id.to_string())
            .bind(identity.craxii_id.to_string())
            .bind(identity.user_id.to_string())
            .bind(identity.external_subject_id.as_str())
            .bind(match identity.lifecycle {
                ExternalIdentityLifecycle::Active => "active",
                ExternalIdentityLifecycle::Revoked => "revoked",
            })
            .bind(identity.created_at.to_string())
            .bind(identity.revoked_at.map(|at| at.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            Ok(())
        })
    }

    fn load_external_identity(
        &self,
        id: ExternalIdentityId,
    ) -> ChannelIdentityFuture<'_, Option<ExternalIdentity>> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            let row =
                sqlx::query("SELECT * FROM external_identities WHERE external_identity_id = ?")
                    .bind(id.to_string())
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(map_sqlx)?;
            row.map(|row| {
                Ok(ExternalIdentity {
                    external_identity_id: parse_id(
                        &row.try_get::<String, _>("external_identity_id")?,
                    )?,
                    channel_account_id: parse_id(&row.try_get::<String, _>("channel_account_id")?)?,
                    craxii_id: parse_id(&row.try_get::<String, _>("craxii_id")?)?,
                    user_id: parse_id(&row.try_get::<String, _>("user_id")?)?,
                    external_subject_id: ExternalSubjectId::try_new(
                        row.try_get::<String, _>("external_subject_id")?,
                    )
                    .map_err(|_| inconsistent())?,
                    lifecycle: match row.try_get::<String, _>("lifecycle_state")?.as_str() {
                        "active" => ExternalIdentityLifecycle::Active,
                        "revoked" => ExternalIdentityLifecycle::Revoked,
                        _ => return Err(inconsistent()),
                    },
                    created_at: parse_id(&row.try_get::<String, _>("created_at")?)?,
                    revoked_at: row
                        .try_get::<Option<String>, _>("revoked_at")?
                        .map(|value| parse_id(&value))
                        .transpose()?,
                })
            })
            .transpose()
        })
    }

    fn persist_conversation_binding(
        &self,
        binding: ConversationBinding,
    ) -> ChannelIdentityFuture<'_, ()> {
        Box::pin(async move {
            lifecycle_shape(match binding.lifecycle {
                ConversationBindingLifecycle::Active => binding.revoked_at.is_none(),
                ConversationBindingLifecycle::Revoked => binding
                    .revoked_at
                    .is_some_and(|revoked| revoked >= binding.created_at),
            })?;
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            sqlx::query(
                "INSERT INTO conversation_bindings \
                 (conversation_binding_id, channel_account_id, external_identity_id, craxii_id, \
                  user_id, conversation_id, external_conversation_id, external_thread_id, \
                  lifecycle_state, created_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(binding.conversation_binding_id.to_string())
            .bind(binding.channel_account_id.to_string())
            .bind(binding.external_identity_id.to_string())
            .bind(binding.craxii_id.to_string())
            .bind(binding.user_id.to_string())
            .bind(binding.conversation_id.to_string())
            .bind(binding.external_conversation_id.as_str())
            .bind(binding.external_thread_id.as_ref().map(ExternalThreadId::as_str))
            .bind(match binding.lifecycle {
                ConversationBindingLifecycle::Active => "active",
                ConversationBindingLifecycle::Revoked => "revoked",
            })
            .bind(binding.created_at.to_string())
            .bind(binding.revoked_at.map(|at| at.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            Ok(())
        })
    }

    fn load_conversation_binding(
        &self,
        id: ConversationBindingId,
    ) -> ChannelIdentityFuture<'_, Option<ConversationBinding>> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            let row = sqlx::query(
                "SELECT * FROM conversation_bindings WHERE conversation_binding_id = ?",
            )
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            row.map(|row| {
                Ok(ConversationBinding {
                    conversation_binding_id: parse_id(
                        &row.try_get::<String, _>("conversation_binding_id")?,
                    )?,
                    channel_account_id: parse_id(&row.try_get::<String, _>("channel_account_id")?)?,
                    external_identity_id: parse_id(
                        &row.try_get::<String, _>("external_identity_id")?,
                    )?,
                    craxii_id: parse_id(&row.try_get::<String, _>("craxii_id")?)?,
                    user_id: parse_id(&row.try_get::<String, _>("user_id")?)?,
                    conversation_id: parse_id(&row.try_get::<String, _>("conversation_id")?)?,
                    external_conversation_id: ExternalConversationId::try_new(
                        row.try_get::<String, _>("external_conversation_id")?,
                    )
                    .map_err(|_| inconsistent())?,
                    external_thread_id: row
                        .try_get::<Option<String>, _>("external_thread_id")?
                        .map(ExternalThreadId::try_new)
                        .transpose()
                        .map_err(|_| inconsistent())?,
                    lifecycle: match row.try_get::<String, _>("lifecycle_state")?.as_str() {
                        "active" => ConversationBindingLifecycle::Active,
                        "revoked" => ConversationBindingLifecycle::Revoked,
                        _ => return Err(inconsistent()),
                    },
                    created_at: parse_id(&row.try_get::<String, _>("created_at")?)?,
                    revoked_at: row
                        .try_get::<Option<String>, _>("revoked_at")?
                        .map(|value| parse_id(&value))
                        .transpose()?,
                })
            })
            .transpose()
        })
    }

    fn persist_inbound_delivery(&self, delivery: InboundDelivery) -> ChannelIdentityFuture<'_, ()> {
        Box::pin(async move {
            validate_inbound_shape(&delivery)?;
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            sqlx::query(
                "INSERT INTO inbound_deliveries \
                 (inbound_delivery_id, channel_account_id, craxii_id, external_event_id, \
                  external_message_id, external_subject_id, external_conversation_id, \
                  external_thread_id, material_sha256, provider_occurred_at, received_at, \
                  receipt_state, classification, external_identity_id, conversation_binding_id, \
                  user_id, conversation_id, message_id, work_id, control_target_work_id, classified_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(delivery.inbound_delivery_id.to_string())
            .bind(delivery.channel_account_id.to_string())
            .bind(delivery.craxii_id.to_string())
            .bind(delivery.external_event_id.as_str())
            .bind(delivery.external_message_id.as_ref().map(ExternalMessageId::as_str))
            .bind(delivery.external_subject_id.as_str())
            .bind(delivery.external_conversation_id.as_str())
            .bind(delivery.external_thread_id.as_ref().map(ExternalThreadId::as_str))
            .bind(delivery.material_sha256.to_string())
            .bind(delivery.provider_occurred_at.map(|at| at.to_string()))
            .bind(delivery.received_at.to_string())
            .bind(match delivery.receipt_state {
                InboundReceiptState::Received => "received",
                InboundReceiptState::Classified => "classified",
            })
            .bind(delivery.classification.map(|value| match value {
                InboundClassification::Message => "message",
                InboundClassification::Control => "control",
                InboundClassification::Rejected => "rejected",
                InboundClassification::Unsupported => "unsupported",
            }))
            .bind(delivery.external_identity_id.map(|id| id.to_string()))
            .bind(delivery.conversation_binding_id.map(|id| id.to_string()))
            .bind(delivery.user_id.map(|id| id.to_string()))
            .bind(delivery.conversation_id.map(|id| id.to_string()))
            .bind(delivery.message_id.map(|id| id.to_string()))
            .bind(delivery.work_id.map(|id| id.to_string()))
            .bind(delivery.control_target_work_id.map(|id| id.to_string()))
            .bind(delivery.classified_at.map(|at| at.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_sqlx)?;
            Ok(())
        })
    }

    fn load_inbound_delivery(
        &self,
        id: InboundDeliveryId,
    ) -> ChannelIdentityFuture<'_, Option<InboundDelivery>> {
        Box::pin(async move {
            let mut connection = self.runtime.acquire().await.map_err(|_| {
                ChannelIdentityStoreError::new(ChannelIdentityStoreErrorKind::Storage)
            })?;
            let row = sqlx::query("SELECT * FROM inbound_deliveries WHERE inbound_delivery_id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *connection)
                .await
                .map_err(map_sqlx)?;
            row.map(decode_inbound_delivery).transpose()
        })
    }
}

fn decode_inbound_delivery(
    row: sqlx::sqlite::SqliteRow,
) -> Result<InboundDelivery, ChannelIdentityStoreError> {
    let delivery = InboundDelivery {
        inbound_delivery_id: parse_id(&row.try_get::<String, _>("inbound_delivery_id")?)?,
        channel_account_id: parse_id(&row.try_get::<String, _>("channel_account_id")?)?,
        craxii_id: parse_id(&row.try_get::<String, _>("craxii_id")?)?,
        external_event_id: ExternalEventId::try_new(row.try_get::<String, _>("external_event_id")?)
            .map_err(|_| inconsistent())?,
        external_message_id: row
            .try_get::<Option<String>, _>("external_message_id")?
            .map(ExternalMessageId::try_new)
            .transpose()
            .map_err(|_| inconsistent())?,
        external_subject_id: ExternalSubjectId::try_new(
            row.try_get::<String, _>("external_subject_id")?,
        )
        .map_err(|_| inconsistent())?,
        external_conversation_id: ExternalConversationId::try_new(
            row.try_get::<String, _>("external_conversation_id")?,
        )
        .map_err(|_| inconsistent())?,
        external_thread_id: row
            .try_get::<Option<String>, _>("external_thread_id")?
            .map(ExternalThreadId::try_new)
            .transpose()
            .map_err(|_| inconsistent())?,
        material_sha256: parse_id(&row.try_get::<String, _>("material_sha256")?)?,
        provider_occurred_at: row
            .try_get::<Option<String>, _>("provider_occurred_at")?
            .map(|value| parse_id(&value))
            .transpose()?,
        received_at: parse_id(&row.try_get::<String, _>("received_at")?)?,
        receipt_state: match row.try_get::<String, _>("receipt_state")?.as_str() {
            "received" => InboundReceiptState::Received,
            "classified" => InboundReceiptState::Classified,
            _ => return Err(inconsistent()),
        },
        classification: row
            .try_get::<Option<String>, _>("classification")?
            .map(|value| match value.as_str() {
                "message" => Ok(InboundClassification::Message),
                "control" => Ok(InboundClassification::Control),
                "rejected" => Ok(InboundClassification::Rejected),
                "unsupported" => Ok(InboundClassification::Unsupported),
                _ => Err(inconsistent()),
            })
            .transpose()?,
        external_identity_id: row
            .try_get::<Option<String>, _>("external_identity_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        conversation_binding_id: row
            .try_get::<Option<String>, _>("conversation_binding_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        user_id: row
            .try_get::<Option<String>, _>("user_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        conversation_id: row
            .try_get::<Option<String>, _>("conversation_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        message_id: row
            .try_get::<Option<String>, _>("message_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        work_id: row
            .try_get::<Option<String>, _>("work_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        control_target_work_id: row
            .try_get::<Option<String>, _>("control_target_work_id")?
            .map(|value| parse_id(&value))
            .transpose()?,
        classified_at: row
            .try_get::<Option<String>, _>("classified_at")?
            .map(|value| parse_id(&value))
            .transpose()?,
    };
    validate_inbound_shape(&delivery)?;
    Ok(delivery)
}
