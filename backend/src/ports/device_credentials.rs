//! Narrow dependency-neutral persistence boundary for offline device credentials.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::domain::{DeviceDisplayName, DeviceId, DeviceTokenHash, UtcTimestamp};

pub type DeviceCredentialFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DeviceCredentialStoreError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceCredentialStoreErrorKind {
    Storage,
    Conflict,
    Inconsistent,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DeviceCredentialStoreError {
    kind: DeviceCredentialStoreErrorKind,
}

impl DeviceCredentialStoreError {
    #[must_use]
    pub const fn new(kind: DeviceCredentialStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> DeviceCredentialStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for DeviceCredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            DeviceCredentialStoreErrorKind::Storage => "device credential storage failure",
            DeviceCredentialStoreErrorKind::Conflict => "device credential conflict",
            DeviceCredentialStoreErrorKind::Inconsistent => "device credential inconsistency",
        })
    }
}

impl fmt::Debug for DeviceCredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for DeviceCredentialStoreError {}

pub struct ProvisionDeviceIntent {
    pub device_id: DeviceId,
    pub display_name: DeviceDisplayName,
    pub token_hash: DeviceTokenHash,
    pub created_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceSummary {
    pub device_id: DeviceId,
    pub display_name: DeviceDisplayName,
    pub created_at: UtcTimestamp,
    pub last_seen_at: Option<UtcTimestamp>,
    pub revoked_at: Option<UtcTimestamp>,
}

impl DeviceSummary {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceCredentialMatch {
    pub device_id: DeviceId,
    pub matched_hash: DeviceTokenHash,
    pub revoked_at: Option<UtcTimestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevokeDeviceOutcome {
    Revoked(DeviceSummary),
    AlreadyRevoked(DeviceSummary),
    NotFound,
}

pub trait DeviceCredentialStore: Send + Sync {
    fn provision_device(
        &self,
        intent: ProvisionDeviceIntent,
    ) -> DeviceCredentialFuture<'_, DeviceSummary>;

    fn lookup_device_by_token_hash(
        &self,
        token_hash: DeviceTokenHash,
    ) -> DeviceCredentialFuture<'_, Option<DeviceCredentialMatch>>;

    fn list_devices(&self) -> DeviceCredentialFuture<'_, Vec<DeviceSummary>>;

    fn revoke_device(
        &self,
        device_id: DeviceId,
        revoked_at: UtcTimestamp,
    ) -> DeviceCredentialFuture<'_, RevokeDeviceOutcome>;

    fn best_effort_touch_last_seen(
        &self,
        device_id: DeviceId,
        observed_at: UtcTimestamp,
    ) -> DeviceCredentialFuture<'_, ()>;
}
