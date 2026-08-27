use std::fmt::{Debug, Display, Formatter, Write as _};

use time::{OffsetDateTime, UtcOffset};

use crate::bootstrap::compatibility::{
    ARCHITECTURE_VERSION, CONFIGURATION_VERSION, MAX_SUPPORTED_SCHEMA_VERSION, PROTOCOL_VERSION,
};
use crate::bootstrap::config::ConfigFingerprint;
use crate::ports::clock::{Clock, ClockError};

const UNVERSIONED_REVISION: &str = "unversioned";

#[derive(Clone, Eq, PartialEq)]
pub struct BuildMetadata {
    package_version: String,
    git_revision: String,
    dirty: bool,
    target_triple: String,
    reproducible_build_timestamp: Option<OffsetDateTime>,
    architecture_version: &'static str,
    protocol_version: u64,
    configuration_version: u64,
    max_supported_schema_version: u64,
}

impl BuildMetadata {
    pub fn embedded() -> Result<Self, BuildMetadataError> {
        Self::from_values(
            env!("CRAXII_PACKAGE_VERSION"),
            env!("CRAXII_GIT_REVISION"),
            env!("CRAXII_GIT_DIRTY"),
            env!("CRAXII_BUILD_TARGET"),
            env!("CRAXII_BUILD_TIMESTAMP_EPOCH"),
        )
    }

    pub(crate) fn from_values(
        package_version: &str,
        git_revision: &str,
        dirty: &str,
        target_triple: &str,
        build_timestamp_epoch: &str,
    ) -> Result<Self, BuildMetadataError> {
        let package_version =
            normalize_required(package_version).ok_or(BuildMetadataError::InvalidPackageVersion)?;
        let git_revision = normalize_git_revision(git_revision)?;
        let dirty = match dirty {
            "true" => true,
            "false" => false,
            _ => return Err(BuildMetadataError::InvalidDirtyState),
        };
        let target_triple = normalize_required(target_triple)
            .filter(|target| !target.chars().any(char::is_whitespace))
            .ok_or(BuildMetadataError::InvalidTargetTriple)?;
        let reproducible_build_timestamp = parse_build_timestamp(build_timestamp_epoch)?;

        Ok(Self {
            package_version,
            git_revision,
            dirty,
            target_triple,
            reproducible_build_timestamp,
            architecture_version: ARCHITECTURE_VERSION,
            protocol_version: PROTOCOL_VERSION,
            configuration_version: CONFIGURATION_VERSION,
            max_supported_schema_version: MAX_SUPPORTED_SCHEMA_VERSION,
        })
    }

    pub fn validate_release_provenance(
        &self,
        policy: ReleaseProvenancePolicy,
    ) -> Result<(), ProvenanceError> {
        if self.git_revision == UNVERSIONED_REVISION {
            return Err(ProvenanceError::UnversionedRevision);
        }
        if self.dirty {
            return Err(ProvenanceError::DirtyArtifact);
        }
        if policy.requires_build_timestamp && self.reproducible_build_timestamp.is_none() {
            return Err(ProvenanceError::MissingBuildTimestamp);
        }
        Ok(())
    }

    pub fn package_version(&self) -> &str {
        &self.package_version
    }

    pub fn git_revision(&self) -> &str {
        &self.git_revision
    }

    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn target_triple(&self) -> &str {
        &self.target_triple
    }

    pub const fn reproducible_build_timestamp(&self) -> Option<OffsetDateTime> {
        self.reproducible_build_timestamp
    }

    pub const fn architecture_version(&self) -> &'static str {
        self.architecture_version
    }

    pub const fn protocol_version(&self) -> u64 {
        self.protocol_version
    }

    pub const fn configuration_version(&self) -> u64 {
        self.configuration_version
    }

    pub const fn max_supported_schema_version(&self) -> u64 {
        self.max_supported_schema_version
    }
}

impl Debug for BuildMetadata {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BuildMetadata")
            .field("package_version", &self.package_version)
            .field("git_revision", &self.git_revision)
            .field("dirty", &self.dirty)
            .field("target_triple", &self.target_triple)
            .field(
                "reproducible_build_timestamp",
                &self.reproducible_build_timestamp.map(format_utc_timestamp),
            )
            .field("architecture_version", &self.architecture_version)
            .field("protocol_version", &self.protocol_version)
            .field("configuration_version", &self.configuration_version)
            .field(
                "max_supported_schema_version",
                &self.max_supported_schema_version,
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseProvenancePolicy {
    requires_build_timestamp: bool,
}

impl ReleaseProvenancePolicy {
    pub const fn reproducible_release() -> Self {
        Self {
            requires_build_timestamp: true,
        }
    }

    pub const fn without_required_timestamp() -> Self {
        Self {
            requires_build_timestamp: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvenanceError {
    UnversionedRevision,
    DirtyArtifact,
    MissingBuildTimestamp,
}

impl Display for ProvenanceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnversionedRevision => "release provenance requires a versioned Git revision",
            Self::DirtyArtifact => "release provenance rejects a dirty artifact",
            Self::MissingBuildTimestamp => {
                "release provenance requires a SOURCE_DATE_EPOCH build timestamp"
            }
        })
    }
}

impl std::error::Error for ProvenanceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildMetadataError {
    InvalidPackageVersion,
    InvalidGitRevision,
    InvalidDirtyState,
    InvalidTargetTriple,
    InvalidBuildTimestamp,
}

impl Display for BuildMetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPackageVersion => "embedded package version is invalid",
            Self::InvalidGitRevision => "embedded Git revision is invalid",
            Self::InvalidDirtyState => "embedded Git dirty state is invalid",
            Self::InvalidTargetTriple => "embedded build target is invalid",
            Self::InvalidBuildTimestamp => "embedded reproducible build timestamp is invalid",
        })
    }
}

impl std::error::Error for BuildMetadataError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessMetadata {
    build: BuildMetadata,
    configuration_fingerprint: String,
    process_started_at_utc: OffsetDateTime,
}

impl ProcessMetadata {
    pub fn capture(
        build: BuildMetadata,
        configuration_fingerprint: &ConfigFingerprint,
        clock: &dyn Clock,
    ) -> Result<Self, ClockError> {
        let process_started_at_utc = clock
            .utc_now()?
            .checked_to_offset(UtcOffset::UTC)
            .ok_or(ClockError::WallTimeOutOfRange)?;
        Ok(Self {
            build,
            configuration_fingerprint: configuration_fingerprint.as_str().to_owned(),
            process_started_at_utc,
        })
    }

    pub fn build(&self) -> &BuildMetadata {
        &self.build
    }

    pub fn configuration_fingerprint(&self) -> &str {
        &self.configuration_fingerprint
    }

    pub const fn process_started_at_utc(&self) -> OffsetDateTime {
        self.process_started_at_utc
    }
}

fn normalize_required(value: &str) -> Option<String> {
    let normalized = value.trim();
    (!normalized.is_empty()).then(|| normalized.to_owned())
}

fn normalize_git_revision(value: &str) -> Result<String, BuildMetadataError> {
    let normalized = value.trim();
    if normalized == UNVERSIONED_REVISION {
        return Ok(normalized.to_owned());
    }
    if (7..=64).contains(&normalized.len())
        && normalized.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(normalized.to_ascii_lowercase());
    }
    Err(BuildMetadataError::InvalidGitRevision)
}

fn parse_build_timestamp(value: &str) -> Result<Option<OffsetDateTime>, BuildMetadataError> {
    if value.is_empty() {
        return Ok(None);
    }
    let seconds = value
        .parse::<i64>()
        .map_err(|_| BuildMetadataError::InvalidBuildTimestamp)?;
    OffsetDateTime::from_unix_timestamp(seconds)
        .map(Some)
        .map_err(|_| BuildMetadataError::InvalidBuildTimestamp)
}

pub(crate) fn format_utc_timestamp(timestamp: OffsetDateTime) -> String {
    let timestamp = timestamp.to_offset(UtcOffset::UTC);
    let month = u8::from(timestamp.month());
    let mut output = String::with_capacity(27);
    write!(
        &mut output,
        "{:04}-{month:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        timestamp.year(),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
        timestamp.microsecond(),
    )
    .expect("writing to a String cannot fail");
    output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::bootstrap::config;
    use crate::ports::clock::TestClock;

    const REVISION: &str = "2A69E5DD8D0A4F5F923405245E1C75D07DDC73C1";

    fn metadata(revision: &str, dirty: &str, epoch: &str) -> BuildMetadata {
        BuildMetadata::from_values("0.0.1", revision, dirty, "aarch64-apple-darwin", epoch).unwrap()
    }

    #[test]
    fn embedded_metadata_has_package_revision_dirty_and_target_fields() {
        let metadata = BuildMetadata::embedded().unwrap();
        assert_eq!(metadata.package_version(), env!("CARGO_PKG_VERSION"));
        assert!(!metadata.git_revision().is_empty());
        assert!(!metadata.target_triple().is_empty());
        assert_eq!(metadata.is_dirty(), env!("CRAXII_GIT_DIRTY") == "true");
        assert_eq!(metadata.architecture_version(), ARCHITECTURE_VERSION);
        assert_eq!(metadata.protocol_version(), PROTOCOL_VERSION);
        assert_eq!(metadata.configuration_version(), CONFIGURATION_VERSION);
        assert_eq!(
            metadata.max_supported_schema_version(),
            MAX_SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn git_revision_and_dirty_state_are_normalized_separately() {
        let metadata = metadata(REVISION, "true", "");
        assert_eq!(metadata.git_revision(), REVISION.to_ascii_lowercase());
        assert!(metadata.is_dirty());
        assert_eq!(
            BuildMetadata::from_values("0.0.1", "main-dirty", "false", "target", ""),
            Err(BuildMetadataError::InvalidGitRevision)
        );
    }

    #[test]
    fn source_date_epoch_is_optional_and_checked_when_present() {
        assert_eq!(
            metadata(REVISION, "false", "").reproducible_build_timestamp(),
            None
        );
        assert_eq!(
            metadata(REVISION, "false", "1700000000")
                .reproducible_build_timestamp()
                .unwrap()
                .unix_timestamp(),
            1_700_000_000
        );
        assert_eq!(
            BuildMetadata::from_values("0.0.1", REVISION, "false", "target", "invalid"),
            Err(BuildMetadataError::InvalidBuildTimestamp)
        );
    }

    #[test]
    fn release_provenance_rejects_each_forbidden_condition() {
        let policy = ReleaseProvenancePolicy::reproducible_release();
        assert_eq!(
            metadata("unversioned", "false", "1700000000").validate_release_provenance(policy),
            Err(ProvenanceError::UnversionedRevision)
        );
        assert_eq!(
            metadata(REVISION, "true", "1700000000").validate_release_provenance(policy),
            Err(ProvenanceError::DirtyArtifact)
        );
        assert_eq!(
            metadata(REVISION, "false", "").validate_release_provenance(policy),
            Err(ProvenanceError::MissingBuildTimestamp)
        );
        assert!(
            metadata(REVISION, "false", "1700000000")
                .validate_release_provenance(policy)
                .is_ok()
        );
    }

    #[test]
    fn process_start_and_compatibility_fingerprint_come_from_inputs() {
        let config =
            config::parse(include_str!("../../tests/fixtures/config/valid/local.toml")).unwrap();
        let started_at = OffsetDateTime::from_unix_timestamp(1_700_000_123).unwrap();
        let clock = TestClock::new(started_at, Duration::from_secs(99));
        let process = ProcessMetadata::capture(
            metadata(REVISION, "false", "1700000000"),
            config.fingerprint(),
            &clock,
        )
        .unwrap();

        assert_eq!(process.process_started_at_utc(), started_at);
        assert_eq!(
            process.configuration_fingerprint(),
            config.fingerprint().as_str()
        );
        assert_eq!(
            process.build().configuration_version(),
            config.configuration_version()
        );
    }

    #[test]
    fn process_start_is_normalized_from_positive_offset_to_utc() {
        let local = OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap()
            .to_offset(UtcOffset::from_hms(5, 30, 0).unwrap());
        let process = process_metadata_at(local);

        assert_eq!(process.process_started_at_utc().offset(), UtcOffset::UTC);
        assert_eq!(
            format_utc_timestamp(process.process_started_at_utc()),
            "2023-11-14T22:13:20.000000Z"
        );
    }

    #[test]
    fn process_start_is_normalized_from_negative_offset_to_utc() {
        let local = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .to_offset(UtcOffset::from_hms(-7, 0, 0).unwrap());
        let process = process_metadata_at(local);

        assert_eq!(process.process_started_at_utc().offset(), UtcOffset::UTC);
        assert_eq!(
            format_utc_timestamp(process.process_started_at_utc()),
            "1969-12-31T23:59:59.000000Z"
        );
    }

    #[test]
    fn utc_process_start_is_unchanged() {
        let utc = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(process_metadata_at(utc).process_started_at_utc(), utc);
    }

    #[test]
    fn utc_formatter_normalizes_offset_inputs_instead_of_relabeling_wall_time() {
        let instant = OffsetDateTime::from_unix_timestamp(0)
            .unwrap()
            .to_offset(UtcOffset::from_hms(5, 30, 0).unwrap());

        assert_eq!(format_utc_timestamp(instant), "1970-01-01T00:00:00.000000Z");
    }

    #[test]
    fn out_of_range_build_epoch_remains_a_typed_error() {
        assert_eq!(
            BuildMetadata::from_values("0.0.1", REVISION, "false", "target", &i64::MAX.to_string(),),
            Err(BuildMetadataError::InvalidBuildTimestamp)
        );
    }

    #[test]
    fn utc_formatter_handles_an_extreme_offset_without_relabeling_it() {
        let extreme = OffsetDateTime::from_unix_timestamp(253_402_300_799)
            .unwrap()
            .to_offset(UtcOffset::from_hms(-23, -59, -59).unwrap());
        let output = format_utc_timestamp(extreme);

        assert!(output.starts_with("9999-12-31T23:59:59"));
        assert!(output.ends_with('Z'));
    }

    #[test]
    fn utc_format_has_fixed_microsecond_precision() {
        let timestamp =
            OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_456_789).unwrap();
        assert_eq!(
            format_utc_timestamp(timestamp),
            "2023-11-14T22:13:20.123456Z"
        );
    }

    fn process_metadata_at(started_at: OffsetDateTime) -> ProcessMetadata {
        let config =
            config::parse(include_str!("../../tests/fixtures/config/valid/local.toml")).unwrap();
        let clock = TestClock::new(started_at, Duration::ZERO);
        ProcessMetadata::capture(
            metadata(REVISION, "false", "1700000000"),
            config.fingerprint(),
            &clock,
        )
        .unwrap()
    }
}
