//! Dependency-neutral bounded artifact-byte storage boundary.

use std::collections::BTreeSet;
use std::fmt::{Debug, Display, Formatter};

use crate::domain::{
    ArtifactId, ArtifactStorageKey, CanonicalByteCount, Sha256Digest, UtcTimestamp,
};

/// Closed safe failure classes for artifact-byte operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactStoreErrorKind {
    InvalidRequest,
    UnsafeRoot,
    UnsupportedFilesystem,
    Storage,
    Integrity,
    Collision,
}

/// Redacted artifact-store error with no physical path or OS detail.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ArtifactStoreError {
    kind: ArtifactStoreErrorKind,
}

impl ArtifactStoreError {
    #[must_use]
    pub const fn new(kind: ArtifactStoreErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> ArtifactStoreErrorKind {
        self.kind
    }
}

impl Display for ArtifactStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self.kind {
            ArtifactStoreErrorKind::InvalidRequest => "invalid artifact capture request",
            ArtifactStoreErrorKind::UnsafeRoot => "unsafe artifact storage root",
            ArtifactStoreErrorKind::UnsupportedFilesystem => {
                "unsupported artifact storage filesystem"
            }
            ArtifactStoreErrorKind::Storage => "artifact storage failure",
            ArtifactStoreErrorKind::Integrity => "artifact integrity failure",
            ArtifactStoreErrorKind::Collision => "artifact content collision",
        })
    }
}

impl Debug for ArtifactStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl std::error::Error for ArtifactStoreError {}

/// Required bounded-capture identity and hard limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginArtifactCapture {
    pub artifact_id: ArtifactId,
    pub hard_capture_limit: CanonicalByteCount,
}

/// Durable captured-byte descriptor returned only after full publication verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizedArtifact {
    artifact_id: ArtifactId,
    object: ArtifactObjectReference,
    observed_byte_count: CanonicalByteCount,
    truncated: bool,
}

impl FinalizedArtifact {
    /// Mints the durable-publication capability after the local adapter has synced and verified it.
    ///
    /// Crate visibility keeps this usable by the trusted adapter implementation without exposing a
    /// production-facing fabrication route. Repository checks additionally confine call sites to
    /// the local artifact adapter.
    #[must_use]
    pub(crate) const fn from_durable_publication(
        artifact_id: ArtifactId,
        storage_key: ArtifactStorageKey,
        sha256: Sha256Digest,
        captured_byte_count: CanonicalByteCount,
        observed_byte_count: CanonicalByteCount,
        truncated: bool,
    ) -> Self {
        Self {
            artifact_id,
            object: ArtifactObjectReference {
                storage_key,
                sha256,
                captured_byte_count,
            },
            observed_byte_count,
            truncated,
        }
    }

    #[must_use]
    pub const fn artifact_id(&self) -> ArtifactId {
        self.artifact_id
    }

    #[must_use]
    pub const fn storage_key(&self) -> &ArtifactStorageKey {
        self.object.storage_key()
    }

    #[must_use]
    pub const fn sha256(&self) -> Sha256Digest {
        self.object.sha256()
    }

    #[must_use]
    pub const fn captured_byte_count(&self) -> CanonicalByteCount {
        self.object.captured_byte_count()
    }

    #[must_use]
    pub const fn observed_byte_count(&self) -> CanonicalByteCount {
        self.observed_byte_count
    }

    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Borrows the non-capability object identity used for verified reads and startup checks.
    #[must_use]
    pub const fn object_reference(&self) -> &ArtifactObjectReference {
        &self.object
    }
}

/// Logical identity of one persisted content-addressed object.
///
/// Unlike [`FinalizedArtifact`], this is not proof that the current process published bytes. It is
/// reconstructed from already-committed metadata solely so the artifact adapter can verify/read
/// that object without receiving SQLite or physical-path types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactObjectReference {
    storage_key: ArtifactStorageKey,
    sha256: Sha256Digest,
    captured_byte_count: CanonicalByteCount,
}

impl ArtifactObjectReference {
    #[must_use]
    pub(crate) const fn from_persisted_metadata(
        storage_key: ArtifactStorageKey,
        sha256: Sha256Digest,
        captured_byte_count: CanonicalByteCount,
    ) -> Self {
        Self {
            storage_key,
            sha256,
            captured_byte_count,
        }
    }

    #[must_use]
    pub const fn storage_key(&self) -> &ArtifactStorageKey {
        &self.storage_key
    }

    #[must_use]
    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }

    #[must_use]
    pub const fn captured_byte_count(&self) -> CanonicalByteCount {
        self.captured_byte_count
    }
}

/// Bounded in-progress capture. Chunks after the hard limit remain observed but are not stored.
pub trait ArtifactCapture: Send {
    fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), ArtifactStoreError>;
    fn finalize(self: Box<Self>) -> Result<FinalizedArtifact, ArtifactStoreError>;
}

/// One expected nonfatal startup residue classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactOrphan {
    TempPartial {
        artifact_id: ArtifactId,
        age_seconds: u64,
    },
    FinalUnreferenced {
        storage_key: ArtifactStorageKey,
        age_seconds: u64,
    },
}

impl ArtifactOrphan {
    pub const MAINTENANCE_GRACE_SECONDS: u64 = 24 * 60 * 60;

    /// Pure first-pass eligibility; maintenance must still recheck SQLite immediately before delete.
    #[must_use]
    pub const fn eligible_for_maintenance(&self) -> bool {
        matches!(
            self,
            Self::FinalUnreferenced { age_seconds, .. }
                if *age_seconds >= Self::MAINTENANCE_GRACE_SECONDS
        )
    }
}

/// Stable logical orphan report; no absolute path crosses the port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactOrphanReport {
    pub referenced_final_count: u64,
    pub orphans: Vec<ArtifactOrphan>,
}

/// Content-addressed bytes boundary. Metadata remains owned by named StateStore transactions.
pub trait ArtifactStore: Send + Sync {
    fn begin_capture(
        &self,
        request: BeginArtifactCapture,
    ) -> Result<Box<dyn ArtifactCapture>, ArtifactStoreError>;

    fn verify(&self, artifact: &ArtifactObjectReference) -> Result<(), ArtifactStoreError>;

    fn read_verified(
        &self,
        artifact: &ArtifactObjectReference,
    ) -> Result<Vec<u8>, ArtifactStoreError>;

    fn scan_orphans(
        &self,
        referenced: &BTreeSet<ArtifactStorageKey>,
        observed_at: UtcTimestamp,
    ) -> Result<ArtifactOrphanReport, ArtifactStoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_eligibility_is_final_only_and_exactly_twenty_four_hours() {
        let artifact_id = ArtifactId::generate();
        assert!(
            !ArtifactOrphan::TempPartial {
                artifact_id,
                age_seconds: ArtifactOrphan::MAINTENANCE_GRACE_SECONDS * 2,
            }
            .eligible_for_maintenance()
        );
        let key = ArtifactStorageKey::from_digest(Sha256Digest::hash_bytes(b"x"));
        assert!(
            !ArtifactOrphan::FinalUnreferenced {
                storage_key: key.clone(),
                age_seconds: ArtifactOrphan::MAINTENANCE_GRACE_SECONDS - 1,
            }
            .eligible_for_maintenance()
        );
        assert!(
            ArtifactOrphan::FinalUnreferenced {
                storage_key: key,
                age_seconds: ArtifactOrphan::MAINTENANCE_GRACE_SECONDS,
            }
            .eligible_for_maintenance()
        );
    }
}
