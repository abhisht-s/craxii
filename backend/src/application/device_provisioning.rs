//! Offline application service for replacement-based device credential administration.

use std::fmt;
use std::io::{self, Write};

use crate::domain::{BearerToken, DeviceDisplayName, DeviceId, UtcTimestamp};
use crate::ports::device_credentials::{
    DeviceCredentialStore, DeviceCredentialStoreErrorKind, DeviceSummary, ProvisionDeviceIntent,
    RevokeDeviceOutcome,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceAdministrationErrorKind {
    EntropyUnavailable,
    CredentialConflict,
    StorageFailure,
    StorageInconsistent,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DeviceAdministrationError {
    kind: DeviceAdministrationErrorKind,
}

impl DeviceAdministrationError {
    const fn new(kind: DeviceAdministrationErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> DeviceAdministrationErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self.kind {
            DeviceAdministrationErrorKind::EntropyUnavailable => "entropy_unavailable",
            DeviceAdministrationErrorKind::CredentialConflict => "credential_conflict",
            DeviceAdministrationErrorKind::StorageFailure => "storage_failure",
            DeviceAdministrationErrorKind::StorageInconsistent => "storage_inconsistent",
        }
    }
}

impl fmt::Display for DeviceAdministrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl fmt::Debug for DeviceAdministrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for DeviceAdministrationError {}

pub struct ProvisionedDevice {
    pub summary: DeviceSummary,
    bearer: BearerToken,
}

impl fmt::Debug for ProvisionedDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionedDevice")
            .field("summary", &self.summary)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

impl ProvisionedDevice {
    /// Consumes the only returned raw-secret wrapper and writes one token-only result line.
    pub fn write_bearer_once(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(self.bearer.into_issuance_text().as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}

pub struct DeviceProvisioningService<'a, S> {
    store: &'a S,
}

impl<'a, S> DeviceProvisioningService<'a, S>
where
    S: DeviceCredentialStore,
{
    #[must_use]
    pub const fn new(store: &'a S) -> Self {
        Self { store }
    }

    pub async fn provision(
        &self,
        display_name: DeviceDisplayName,
        created_at: UtcTimestamp,
    ) -> Result<ProvisionedDevice, DeviceAdministrationError> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| {
            DeviceAdministrationError::new(DeviceAdministrationErrorKind::EntropyUnavailable)
        })?;
        let bearer = BearerToken::from_random_bytes(random);
        self.provision_token(display_name, created_at, bearer).await
    }

    async fn provision_token(
        &self,
        display_name: DeviceDisplayName,
        created_at: UtcTimestamp,
        bearer: BearerToken,
    ) -> Result<ProvisionedDevice, DeviceAdministrationError> {
        let summary = self
            .store
            .provision_device(ProvisionDeviceIntent {
                device_id: DeviceId::generate(),
                display_name,
                token_hash: bearer.token_hash(),
                created_at,
            })
            .await
            .map_err(map_store_error)?;
        Ok(ProvisionedDevice { summary, bearer })
    }

    pub async fn list(&self) -> Result<Vec<DeviceSummary>, DeviceAdministrationError> {
        self.store.list_devices().await.map_err(map_store_error)
    }

    pub async fn revoke(
        &self,
        device_id: DeviceId,
        revoked_at: UtcTimestamp,
    ) -> Result<RevokeDeviceOutcome, DeviceAdministrationError> {
        self.store
            .revoke_device(device_id, revoked_at)
            .await
            .map_err(map_store_error)
    }

    #[cfg(test)]
    pub(crate) async fn provision_fixture_token(
        &self,
        display_name: DeviceDisplayName,
        created_at: UtcTimestamp,
        bearer: BearerToken,
    ) -> Result<ProvisionedDevice, DeviceAdministrationError> {
        self.provision_token(display_name, created_at, bearer).await
    }
}

fn map_store_error(
    error: crate::ports::device_credentials::DeviceCredentialStoreError,
) -> DeviceAdministrationError {
    let kind = match error.kind() {
        DeviceCredentialStoreErrorKind::Storage => DeviceAdministrationErrorKind::StorageFailure,
        DeviceCredentialStoreErrorKind::Conflict => {
            DeviceAdministrationErrorKind::CredentialConflict
        }
        DeviceCredentialStoreErrorKind::Inconsistent => {
            DeviceAdministrationErrorKind::StorageInconsistent
        }
    };
    DeviceAdministrationError::new(kind)
}
