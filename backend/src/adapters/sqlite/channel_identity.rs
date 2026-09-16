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
    ChannelIdentityStoreErrorKind,
};

use super::SqliteRuntime;

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
            row.map(|row| {
                Ok(ChannelAccount {
                    channel_account_id: parse_id(&row.try_get::<String, _>("channel_account_id")?)?,
                    craxii_id: parse_id(&row.try_get::<String, _>("craxii_id")?)?,
                    provider_id: ChannelProviderId::try_new(
                        row.try_get::<String, _>("provider_key")?,
                    )
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
            })
            .transpose()
        })
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
