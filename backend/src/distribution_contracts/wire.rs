//! Closed wire DTOs; validated parsing supplies syntax, never authentication.
use super::primitives::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtensionValue {
    Text(String),
    Boolean(bool),
    Integer(u32),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Environment {
    #[serde(rename = "development")]
    V0,
    #[serde(rename = "production")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvertisedVersion {
    pub major: u32,
    pub minor: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub session_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyBinding {
    pub binding_id: U4,
    pub binding_version: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum TextBlockType {
    #[serde(rename = "text")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextBlock {
    pub r#type: TextBlockType,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub version: u32,
    pub blocks: Vec<TextBlock>,
}
pub type Extensions = std::collections::BTreeMap<String, ExtensionValue>;
#[derive(Clone, Serialize, Deserialize)]
pub enum HelloProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HelloType {
    #[serde(rename = "hello")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HelloBodyCapabilitiesItem {
    #[serde(rename = "admission_v1")]
    V0,
    #[serde(rename = "cancel_v1")]
    V1,
    #[serde(rename = "result_v1")]
    V2,
    #[serde(rename = "status_v1")]
    V3,
    #[serde(rename = "drain_v1")]
    V4,
    #[serde(rename = "compute_v1")]
    V5,
    #[serde(rename = "model_v1")]
    V6,
    #[serde(rename = "signed_request_v1")]
    V7,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HelloBodyCanonicalizationVersionsItem {
    #[serde(rename = "craxii.cjson/1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HelloBodySignatureVersionsItem {
    #[serde(rename = "craxii.signature/1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloBody {
    pub release_id: String,
    pub release_sha256: WireDigest,
    pub supported_versions: Vec<AdvertisedVersion>,
    pub capabilities: Vec<HelloBodyCapabilitiesItem>,
    pub canonicalization_versions: Vec<HelloBodyCanonicalizationVersionsItem>,
    pub signature_versions: Vec<HelloBodySignatureVersionsItem>,
    pub sqlite_schema: PositiveI64,
    pub boot_id: WireUuid,
    pub runtime_id: U7,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol: HelloProtocol,
    pub version: Version,
    pub r#type: HelloType,
    pub body: HelloBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SelectedProtocolProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SelectedProtocolType {
    #[serde(rename = "selected_protocol")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SelectedProtocolBodyCanonicalizationVersion {
    #[serde(rename = "craxii.cjson/1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SelectedProtocolBodySignatureVersion {
    #[serde(rename = "craxii.signature/1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SelectedProtocolBodyRequiredCapabilitiesItem {
    #[serde(rename = "admission_v1")]
    V0,
    #[serde(rename = "cancel_v1")]
    V1,
    #[serde(rename = "result_v1")]
    V2,
    #[serde(rename = "status_v1")]
    V3,
    #[serde(rename = "drain_v1")]
    V4,
    #[serde(rename = "compute_v1")]
    V5,
    #[serde(rename = "model_v1")]
    V6,
    #[serde(rename = "signed_request_v1")]
    V7,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedProtocolBody {
    pub selected_version: Version,
    pub canonicalization_version: SelectedProtocolBodyCanonicalizationVersion,
    pub signature_version: SelectedProtocolBodySignatureVersion,
    pub required_capabilities: Vec<SelectedProtocolBodyRequiredCapabilitiesItem>,
    pub server_time: WireTimestamp,
    pub max_poll_wait_ms: u32,
    pub selection_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedProtocol {
    pub protocol: SelectedProtocolProtocol,
    pub version: Version,
    pub r#type: SelectedProtocolType,
    pub body: SelectedProtocolBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV0Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV0Reason {
    #[serde(rename = "schema_invalid")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV0Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV0 {
    pub code: SafeErrorV0Code,
    pub reason: SafeErrorV0Reason,
    pub disposition: SafeErrorV0Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV1Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV1Reason {
    #[serde(rename = "unsupported_version")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV1Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV1 {
    pub code: SafeErrorV1Code,
    pub reason: SafeErrorV1Reason,
    pub disposition: SafeErrorV1Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV2Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV2Reason {
    #[serde(rename = "unsupported_canonicalization")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV2Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV2 {
    pub code: SafeErrorV2Code,
    pub reason: SafeErrorV2Reason,
    pub disposition: SafeErrorV2Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV3Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV3Reason {
    #[serde(rename = "unsupported_signature")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV3Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV3 {
    pub code: SafeErrorV3Code,
    pub reason: SafeErrorV3Reason,
    pub disposition: SafeErrorV3Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV4Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV4Reason {
    #[serde(rename = "unsupported_capability")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV4Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV4 {
    pub code: SafeErrorV4Code,
    pub reason: SafeErrorV4Reason,
    pub disposition: SafeErrorV4Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV5Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV5Reason {
    #[serde(rename = "unauthenticated")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV5Disposition {
    #[serde(rename = "reauthenticate")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV5 {
    pub code: SafeErrorV5Code,
    pub reason: SafeErrorV5Reason,
    pub disposition: SafeErrorV5Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV6Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV6Reason {
    #[serde(rename = "session_expired")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV6Disposition {
    #[serde(rename = "reauthenticate")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV6 {
    pub code: SafeErrorV6Code,
    pub reason: SafeErrorV6Reason,
    pub disposition: SafeErrorV6Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV7Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV7Reason {
    #[serde(rename = "unauthorized_scope")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV7Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV7 {
    pub code: SafeErrorV7Code,
    pub reason: SafeErrorV7Reason,
    pub disposition: SafeErrorV7Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV8Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV8Reason {
    #[serde(rename = "customer_mismatch")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV8Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV8 {
    pub code: SafeErrorV8Code,
    pub reason: SafeErrorV8Reason,
    pub disposition: SafeErrorV8Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV9Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV9Reason {
    #[serde(rename = "installation_mismatch")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV9Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV9 {
    pub code: SafeErrorV9Code,
    pub reason: SafeErrorV9Reason,
    pub disposition: SafeErrorV9Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV10Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV10Reason {
    #[serde(rename = "stale_generation")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV10Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV10 {
    pub code: SafeErrorV10Code,
    pub reason: SafeErrorV10Reason,
    pub disposition: SafeErrorV10Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV11Code {
    #[serde(rename = "ownership_denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV11Reason {
    #[serde(rename = "revoked_authority")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV11Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV11 {
    pub code: SafeErrorV11Code,
    pub reason: SafeErrorV11Reason,
    pub disposition: SafeErrorV11Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV12Code {
    #[serde(rename = "lease_lost")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV12Reason {
    #[serde(rename = "stale_lease")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV12Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV12 {
    pub code: SafeErrorV12Code,
    pub reason: SafeErrorV12Reason,
    pub disposition: SafeErrorV12Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV13Code {
    #[serde(rename = "lease_lost")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV13Reason {
    #[serde(rename = "stale_resource_fence")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV13Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV13 {
    pub code: SafeErrorV13Code,
    pub reason: SafeErrorV13Reason,
    pub disposition: SafeErrorV13Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV14Code {
    #[serde(rename = "lease_lost")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV14Reason {
    #[serde(rename = "stale_resource_revision")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV14Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV14 {
    pub code: SafeErrorV14Code,
    pub reason: SafeErrorV14Reason,
    pub disposition: SafeErrorV14Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV15Code {
    #[serde(rename = "operation_identity_conflict")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV15Reason {
    #[serde(rename = "digest_conflict")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV15Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV15 {
    pub code: SafeErrorV15Code,
    pub reason: SafeErrorV15Reason,
    pub disposition: SafeErrorV15Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV16Code {
    #[serde(rename = "operation_identity_conflict")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV16Reason {
    #[serde(rename = "result_conflict")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV16Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV16 {
    pub code: SafeErrorV16Code,
    pub reason: SafeErrorV16Reason,
    pub disposition: SafeErrorV16Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV17Code {
    #[serde(rename = "operation_identity_conflict")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV17Reason {
    #[serde(rename = "result_revision_invalid")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV17Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV17 {
    pub code: SafeErrorV17Code,
    pub reason: SafeErrorV17Reason,
    pub disposition: SafeErrorV17Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV18Code {
    #[serde(rename = "outcome_unknown")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV18Reason {
    #[serde(rename = "provider_effect_unknown")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV18Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV18 {
    pub code: SafeErrorV18Code,
    pub reason: SafeErrorV18Reason,
    pub disposition: SafeErrorV18Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV19Code {
    #[serde(rename = "outcome_unknown")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV19Reason {
    #[serde(rename = "reconciliation_required")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV19Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV19 {
    pub code: SafeErrorV19Code,
    pub reason: SafeErrorV19Reason,
    pub disposition: SafeErrorV19Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV20Code {
    #[serde(rename = "outcome_unknown")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV20Reason {
    #[serde(rename = "restored_interval_missing")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV20Disposition {
    #[serde(rename = "reconcile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV20 {
    pub code: SafeErrorV20Code,
    pub reason: SafeErrorV20Reason,
    pub disposition: SafeErrorV20Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV21Code {
    #[serde(rename = "database_busy")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV21Reason {
    #[serde(rename = "backend_unavailable")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV21Disposition {
    #[serde(rename = "retry_same_identity")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV21 {
    pub code: SafeErrorV21Code,
    pub reason: SafeErrorV21Reason,
    pub disposition: SafeErrorV21Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV22Code {
    #[serde(rename = "database_busy")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV22Reason {
    #[serde(rename = "temporary_failure")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV22Disposition {
    #[serde(rename = "retry_same_identity")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV22 {
    pub code: SafeErrorV22Code,
    pub reason: SafeErrorV22Reason,
    pub disposition: SafeErrorV22Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV23Code {
    #[serde(rename = "support_needed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV23Reason {
    #[serde(rename = "manual_resolution")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV23Disposition {
    #[serde(rename = "support")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV23 {
    pub code: SafeErrorV23Code,
    pub reason: SafeErrorV23Reason,
    pub disposition: SafeErrorV23Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV24Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV24Reason {
    #[serde(rename = "compute_budget_exhausted")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV24Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV24 {
    pub code: SafeErrorV24Code,
    pub reason: SafeErrorV24Reason,
    pub disposition: SafeErrorV24Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV25Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV25Reason {
    #[serde(rename = "model_budget_unavailable")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV25Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV25 {
    pub code: SafeErrorV25Code,
    pub reason: SafeErrorV25Reason,
    pub disposition: SafeErrorV25Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV26Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV26Reason {
    #[serde(rename = "entitlement_expired")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV26Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV26 {
    pub code: SafeErrorV26Code,
    pub reason: SafeErrorV26Reason,
    pub disposition: SafeErrorV26Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV27Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV27Reason {
    #[serde(rename = "policy_unapproved")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV27Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV27 {
    pub code: SafeErrorV27Code,
    pub reason: SafeErrorV27Reason,
    pub disposition: SafeErrorV27Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV28Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV28Reason {
    #[serde(rename = "proof_missing")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV28Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV28 {
    pub code: SafeErrorV28Code,
    pub reason: SafeErrorV28Reason,
    pub disposition: SafeErrorV28Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV29Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV29Reason {
    #[serde(rename = "proof_expired")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV29Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV29 {
    pub code: SafeErrorV29Code,
    pub reason: SafeErrorV29Reason,
    pub disposition: SafeErrorV29Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV30Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV30Reason {
    #[serde(rename = "consent_required")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV30Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV30 {
    pub code: SafeErrorV30Code,
    pub reason: SafeErrorV30Reason,
    pub disposition: SafeErrorV30Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV31Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV31Reason {
    #[serde(rename = "compute_authorization_expired")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV31Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV31 {
    pub code: SafeErrorV31Code,
    pub reason: SafeErrorV31Reason,
    pub disposition: SafeErrorV31Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV32Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV32Reason {
    #[serde(rename = "authorization_epoch_stale")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV32Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV32 {
    pub code: SafeErrorV32Code,
    pub reason: SafeErrorV32Reason,
    pub disposition: SafeErrorV32Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV33Code {
    #[serde(rename = "dispatch_disabled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV33Reason {
    #[serde(rename = "terminal_cancellation")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV33Disposition {
    #[serde(rename = "terminal")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV33 {
    pub code: SafeErrorV33Code,
    pub reason: SafeErrorV33Reason,
    pub disposition: SafeErrorV33Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV34Code {
    #[serde(rename = "retry_exhausted")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV34Reason {
    #[serde(rename = "attempt_budget_exhausted")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV34Disposition {
    #[serde(rename = "support")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV34 {
    pub code: SafeErrorV34Code,
    pub reason: SafeErrorV34Reason,
    pub disposition: SafeErrorV34Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV35Code {
    #[serde(rename = "not_found")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV35Reason {
    #[serde(rename = "not_found")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV35Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV35 {
    pub code: SafeErrorV35Code,
    pub reason: SafeErrorV35Reason,
    pub disposition: SafeErrorV35Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV36Code {
    #[serde(rename = "schema_incompatible")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV36Reason {
    #[serde(rename = "schema_incompatible")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SafeErrorV36Disposition {
    #[serde(rename = "hold")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeErrorV36 {
    pub code: SafeErrorV36Code,
    pub reason: SafeErrorV36Reason,
    pub disposition: SafeErrorV36Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<CanonicalWorkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SafeError {
    V0(SafeErrorV0),
    V1(SafeErrorV1),
    V2(SafeErrorV2),
    V3(SafeErrorV3),
    V4(SafeErrorV4),
    V5(SafeErrorV5),
    V6(SafeErrorV6),
    V7(SafeErrorV7),
    V8(SafeErrorV8),
    V9(SafeErrorV9),
    V10(SafeErrorV10),
    V11(SafeErrorV11),
    V12(SafeErrorV12),
    V13(SafeErrorV13),
    V14(SafeErrorV14),
    V15(SafeErrorV15),
    V16(SafeErrorV16),
    V17(SafeErrorV17),
    V18(SafeErrorV18),
    V19(SafeErrorV19),
    V20(SafeErrorV20),
    V21(SafeErrorV21),
    V22(SafeErrorV22),
    V23(SafeErrorV23),
    V24(SafeErrorV24),
    V25(SafeErrorV25),
    V26(SafeErrorV26),
    V27(SafeErrorV27),
    V28(SafeErrorV28),
    V29(SafeErrorV29),
    V30(SafeErrorV30),
    V31(SafeErrorV31),
    V32(SafeErrorV32),
    V33(SafeErrorV33),
    V34(SafeErrorV34),
    V35(SafeErrorV35),
    V36(SafeErrorV36),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ErrorProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ErrorType {
    #[serde(rename = "error")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Error {
    pub protocol: ErrorProtocol,
    pub version: Version,
    pub r#type: ErrorType,
    pub body: SafeError,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionType {
    #[serde(rename = "protocol_rejection")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV0Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV0Reason {
    #[serde(rename = "unsupported_version")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV0Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejectionErrorV0 {
    pub code: ProtocolRejectionErrorV0Code,
    pub reason: ProtocolRejectionErrorV0Reason,
    pub disposition: ProtocolRejectionErrorV0Disposition,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV1Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV1Reason {
    #[serde(rename = "unsupported_canonicalization")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV1Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejectionErrorV1 {
    pub code: ProtocolRejectionErrorV1Code,
    pub reason: ProtocolRejectionErrorV1Reason,
    pub disposition: ProtocolRejectionErrorV1Disposition,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV2Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV2Reason {
    #[serde(rename = "unsupported_signature")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV2Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejectionErrorV2 {
    pub code: ProtocolRejectionErrorV2Code,
    pub reason: ProtocolRejectionErrorV2Reason,
    pub disposition: ProtocolRejectionErrorV2Disposition,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV3Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV3Reason {
    #[serde(rename = "unsupported_capability")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV3Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejectionErrorV3 {
    pub code: ProtocolRejectionErrorV3Code,
    pub reason: ProtocolRejectionErrorV3Reason,
    pub disposition: ProtocolRejectionErrorV3Disposition,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV4Code {
    #[serde(rename = "invalid_contract")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV4Reason {
    #[serde(rename = "schema_invalid")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProtocolRejectionErrorV4Disposition {
    #[serde(rename = "reject")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejectionErrorV4 {
    pub code: ProtocolRejectionErrorV4Code,
    pub reason: ProtocolRejectionErrorV4Reason,
    pub disposition: ProtocolRejectionErrorV4Disposition,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProtocolRejectionError {
    V0(ProtocolRejectionErrorV0),
    V1(ProtocolRejectionErrorV1),
    V2(ProtocolRejectionErrorV2),
    V3(ProtocolRejectionErrorV3),
    V4(ProtocolRejectionErrorV4),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRejection {
    pub r#type: ProtocolRejectionType,
    pub error: ProtocolRejectionError,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_versions: Option<Vec<AdvertisedVersion>>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentChallengeProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentChallengeType {
    #[serde(rename = "enrollment_challenge")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentChallengeBodyPurpose {
    #[serde(rename = "enroll")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentChallengeBody {
    pub audience: String,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub challenge_id: U4,
    pub selection_id: U4,
    pub nonce: Binary32,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub enrollment_id: U4,
    pub expected_instance_ref: U4,
    pub purpose: EnrollmentChallengeBodyPurpose,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentChallenge {
    pub protocol: EnrollmentChallengeProtocol,
    pub version: Version,
    pub r#type: EnrollmentChallengeType,
    pub body: EnrollmentChallengeBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentSignedPurpose {
    #[serde(rename = "enrollment")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentSigned {
    pub audience: String,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub challenge_id: U4,
    pub selection_id: U4,
    pub nonce: Binary32,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub enrollment_id: U4,
    pub expected_instance_ref: U4,
    pub signature_version: u32,
    pub purpose: EnrollmentSignedPurpose,
    pub key_id: WireDigest,
    pub public_key: Binary32,
    pub instance_identity_document_sha256: WireDigest,
    pub instance_identity_signature_sha256: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentProofProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentProofType {
    #[serde(rename = "enrollment_proof")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentProofBody {
    pub signed: EnrollmentSigned,
    pub instance_identity_document_b64: String,
    pub instance_identity_signature_b64: String,
    pub proof_signature: Binary64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentProof {
    pub protocol: EnrollmentProofProtocol,
    pub version: Version,
    pub r#type: EnrollmentProofType,
    pub body: EnrollmentProofBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentAckProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EnrollmentAckType {
    #[serde(rename = "enrollment_ack")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentAckBody {
    pub enrollment_id: U4,
    pub binding_id: U4,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub key_id: WireDigest,
    pub bound_at: WireTimestamp,
    pub duplicate: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentAck {
    pub protocol: EnrollmentAckProtocol,
    pub version: Version,
    pub r#type: EnrollmentAckType,
    pub body: EnrollmentAckBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionChallengeProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionChallengeType {
    #[serde(rename = "session_challenge")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionChallengeBodyPurpose {
    #[serde(rename = "session")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionChallengeBodyScopesItem {
    #[serde(rename = "receipt_commit")]
    V0,
    #[serde(rename = "result_export")]
    V1,
    #[serde(rename = "status")]
    V2,
    #[serde(rename = "drain")]
    V3,
    #[serde(rename = "cancel")]
    V4,
    #[serde(rename = "model_invoke")]
    V5,
    #[serde(rename = "compute_renew")]
    V6,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionChallengeBody {
    pub audience: String,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub challenge_id: U4,
    pub selection_id: U4,
    pub nonce: Binary32,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub purpose: SessionChallengeBodyPurpose,
    pub key_id: WireDigest,
    pub scopes: Vec<SessionChallengeBodyScopesItem>,
    pub revocation_epoch: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionChallenge {
    pub protocol: SessionChallengeProtocol,
    pub version: Version,
    pub r#type: SessionChallengeType,
    pub body: SessionChallengeBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionSignedPurpose {
    #[serde(rename = "session")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionSignedScopesItem {
    #[serde(rename = "receipt_commit")]
    V0,
    #[serde(rename = "result_export")]
    V1,
    #[serde(rename = "status")]
    V2,
    #[serde(rename = "drain")]
    V3,
    #[serde(rename = "cancel")]
    V4,
    #[serde(rename = "model_invoke")]
    V5,
    #[serde(rename = "compute_renew")]
    V6,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSigned {
    pub audience: String,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub challenge_id: U4,
    pub selection_id: U4,
    pub nonce: Binary32,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub purpose: SessionSignedPurpose,
    pub key_id: WireDigest,
    pub scopes: Vec<SessionSignedScopesItem>,
    pub revocation_epoch: PositiveI64,
    pub signature_version: u32,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionProofProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionProofType {
    #[serde(rename = "session_proof")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProofBody {
    pub signed: SessionSigned,
    pub proof_signature: Binary64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProof {
    pub protocol: SessionProofProtocol,
    pub version: Version,
    pub r#type: SessionProofType,
    pub body: SessionProofBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionGrantProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionGrantType {
    #[serde(rename = "session_grant")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SessionGrantBodyScopesItem {
    #[serde(rename = "receipt_commit")]
    V0,
    #[serde(rename = "result_export")]
    V1,
    #[serde(rename = "status")]
    V2,
    #[serde(rename = "drain")]
    V3,
    #[serde(rename = "cancel")]
    V4,
    #[serde(rename = "model_invoke")]
    V5,
    #[serde(rename = "compute_renew")]
    V6,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionGrantBody {
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub session_id: U4,
    pub selection_id: U4,
    pub session_credential: SessionCredential,
    pub key_id: WireDigest,
    pub scopes: Vec<SessionGrantBodyScopesItem>,
    pub revocation_epoch: PositiveI64,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionGrant {
    pub protocol: SessionGrantProtocol,
    pub version: Version,
    pub r#type: SessionGrantType,
    pub body: SessionGrantBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SensitiveSignedPurpose {
    #[serde(rename = "sensitive_request")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SensitiveSignedOperationType {
    #[serde(rename = "work_receipt")]
    V0,
    #[serde(rename = "cancel_receipt")]
    V1,
    #[serde(rename = "result_export")]
    V2,
    #[serde(rename = "drain_ack")]
    V3,
    #[serde(rename = "model_invoke")]
    V4,
    #[serde(rename = "compute_renew")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitiveSigned {
    pub signature_version: u32,
    pub purpose: SensitiveSignedPurpose,
    pub audience: String,
    pub environment: Environment,
    pub session_id: U4,
    pub installation_id: InstallationId,
    pub challenge_id: U4,
    pub request_id: U4,
    pub generation: PositiveI64,
    pub revocation_epoch: PositiveI64,
    pub key_id: WireDigest,
    pub nonce: Binary32,
    pub operation_type: SensitiveSignedOperationType,
    pub request_digest: WireDigest,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SensitiveRequestProofProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SensitiveRequestProofType {
    #[serde(rename = "sensitive_request_proof")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitiveRequestProofBody {
    pub signed: SensitiveSigned,
    pub proof_signature: Binary64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitiveRequestProof {
    pub protocol: SensitiveRequestProofProtocol,
    pub version: Version,
    pub r#type: SensitiveRequestProofType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: SensitiveRequestProofBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProfileMaterialV0Environment {
    #[serde(rename = "development")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProfileMaterialV0Kind {
    #[serde(rename = "development_fixture")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMaterialV0 {
    pub profile_version: u32,
    pub profile_id: U4,
    pub environment: ProfileMaterialV0Environment,
    pub audience: String,
    pub acceptance_account_authorization_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub work_id: CanonicalWorkId,
    pub period_id: U4,
    pub compute_grant_id: U4,
    pub model_config_id: U4,
    pub price_config_id: U4,
    pub material_digest: WireDigest,
    pub model_config_digest: WireDigest,
    pub price_config_digest: WireDigest,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub allowed_tool_ids: Vec<String>,
    pub max_tool_duration_ms: PositiveI64,
    pub kind: ProfileMaterialV0Kind,
    pub deterministic_program_id: String,
    pub config_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProfileMaterialV1Environment {
    #[serde(rename = "development")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ProfileMaterialV1Kind {
    #[serde(rename = "qualified_candidate")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMaterialV1 {
    pub profile_version: u32,
    pub profile_id: U4,
    pub environment: ProfileMaterialV1Environment,
    pub audience: String,
    pub acceptance_account_authorization_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub work_id: CanonicalWorkId,
    pub period_id: U4,
    pub compute_grant_id: U4,
    pub model_config_id: U4,
    pub price_config_id: U4,
    pub material_digest: WireDigest,
    pub model_config_digest: WireDigest,
    pub price_config_digest: WireDigest,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub allowed_tool_ids: Vec<String>,
    pub max_tool_duration_ms: PositiveI64,
    pub kind: ProfileMaterialV1Kind,
    pub qualification_evidence_id: U4,
    pub candidate_config_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProfileMaterial {
    V0(ProfileMaterialV0),
    V1(ProfileMaterialV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ExecutionAuthorizationV0Kind {
    #[serde(rename = "ordinary")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorizationV0 {
    pub kind: ExecutionAuthorizationV0Kind,
    pub period_id: U4,
    pub compute_grant_id: U4,
    pub authorization_epoch: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ExecutionAuthorizationV1Kind {
    #[serde(rename = "development_fixture")]
    V0,
    #[serde(rename = "qualified_candidate")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorizationV1 {
    pub kind: ExecutionAuthorizationV1Kind,
    pub profile_id: U4,
    pub profile_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutionAuthorization {
    V0(ExecutionAuthorizationV0),
    V1(ExecutionAuthorizationV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ExecutionProfileProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ExecutionProfileType {
    #[serde(rename = "execution_profile")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfileBody {
    pub profile: ProfileMaterial,
    pub signature: Binary64,
    pub key_id: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfile {
    pub protocol: ExecutionProfileProtocol,
    pub version: Version,
    pub r#type: ExecutionProfileType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ExecutionProfileBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkMaterial {
    pub material_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub ingress_id: U4,
    pub conversation_sequence: PositiveI64,
    pub reply_binding: ReplyBinding,
    pub content: Content,
    pub created_at: WireTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkDeliveryProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkDeliveryType {
    #[serde(rename = "work_delivery")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkDeliveryBody {
    pub delivery_id: U4,
    pub power_operation_id: U4,
    pub delivery_revision: PositiveI64,
    pub boot_id: WireUuid,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub admission_operation_id: U4,
    pub material: WorkMaterial,
    pub material_digest: WireDigest,
    pub execution_authorization: ExecutionAuthorization,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage_authorization_id: Option<U4>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkDelivery {
    pub protocol: WorkDeliveryProtocol,
    pub version: Version,
    pub r#type: WorkDeliveryType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: WorkDeliveryBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub work_id: CanonicalWorkId,
    pub origin_installation_id: InstallationId,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub local_message_id: U7,
    pub local_conversation_id: U7,
    pub conversation_sequence: PositiveI64,
    pub local_work_ordinal: PositiveI64,
    pub journal_offset: PositiveI64,
    pub origin_generation: PositiveI64,
    pub material_digest: WireDigest,
    pub admitted_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkReceiptProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkReceiptType {
    #[serde(rename = "work_receipt")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkReceiptBodyCurrentWorkState {
    #[serde(rename = "queued")]
    V0,
    #[serde(rename = "running")]
    V1,
    #[serde(rename = "waiting_on_model")]
    V2,
    #[serde(rename = "waiting_on_tool")]
    V3,
    #[serde(rename = "cancel_requested")]
    V4,
    #[serde(rename = "completed")]
    V5,
    #[serde(rename = "failed")]
    V6,
    #[serde(rename = "cancelled")]
    V7,
    #[serde(rename = "interrupted")]
    V8,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkReceiptBody {
    pub receipt: Receipt,
    pub duplicate: bool,
    pub current_work_state: WorkReceiptBodyCurrentWorkState,
    pub work_state_revision: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkReceipt {
    pub protocol: WorkReceiptProtocol,
    pub version: Version,
    pub r#type: WorkReceiptType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: WorkReceiptBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptQueryProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptQueryType {
    #[serde(rename = "receipt_query")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQueryBody {
    pub work_id: CanonicalWorkId,
    pub material_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQuery {
    pub protocol: ReceiptQueryProtocol,
    pub version: Version,
    pub r#type: ReceiptQueryType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ReceiptQueryBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptStatusProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptStatusType {
    #[serde(rename = "receipt_status")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptStatusBodyV0Status {
    #[serde(rename = "committed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStatusBodyV0 {
    pub status: ReceiptStatusBodyV0Status,
    pub receipt: Receipt,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ReceiptStatusBodyV1Status {
    #[serde(rename = "not_found")]
    V0,
    #[serde(rename = "reconciliation_required")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStatusBodyV1 {
    pub status: ReceiptStatusBodyV1Status,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReceiptStatusBody {
    V0(Box<ReceiptStatusBodyV0>),
    V1(ReceiptStatusBodyV1),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStatus {
    pub protocol: ReceiptStatusProtocol,
    pub version: Version,
    pub r#type: ReceiptStatusType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ReceiptStatusBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelMaterialCommand {
    #[serde(rename = "cancel")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelMaterialReason {
    #[serde(rename = "user_request")]
    V0,
    #[serde(rename = "authorization_expired")]
    V1,
    #[serde(rename = "operator_stop")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelMaterial {
    pub control_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub material_digest: WireDigest,
    pub command: CancelMaterialCommand,
    pub reason: CancelMaterialReason,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelDeliveryProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelDeliveryType {
    #[serde(rename = "cancel_delivery")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelDeliveryBody {
    pub delivery_id: U4,
    pub power_operation_id: U4,
    pub delivery_revision: PositiveI64,
    pub boot_id: WireUuid,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub control: CancelMaterial,
    pub control_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelDelivery {
    pub protocol: CancelDeliveryProtocol,
    pub version: Version,
    pub r#type: CancelDeliveryType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: CancelDeliveryBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptType {
    #[serde(rename = "cancel_receipt")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptBodyV0Outcome {
    #[serde(rename = "cancelled_before_admission")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReceiptBodyV0 {
    pub control_id: U4,
    pub work_id: CanonicalWorkId,
    pub control_digest: WireDigest,
    pub cleanup_pending: bool,
    pub committed_at: WireTimestamp,
    pub outcome: CancelReceiptBodyV0Outcome,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptBodyV1Outcome {
    #[serde(rename = "requested")]
    V0,
    #[serde(rename = "cancelled")]
    V1,
    #[serde(rename = "already_requested")]
    V2,
    #[serde(rename = "already_terminal")]
    V3,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptBodyV1WorkState {
    #[serde(rename = "queued")]
    V0,
    #[serde(rename = "running")]
    V1,
    #[serde(rename = "waiting_on_model")]
    V2,
    #[serde(rename = "waiting_on_tool")]
    V3,
    #[serde(rename = "cancel_requested")]
    V4,
    #[serde(rename = "completed")]
    V5,
    #[serde(rename = "failed")]
    V6,
    #[serde(rename = "cancelled")]
    V7,
    #[serde(rename = "interrupted")]
    V8,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReceiptBodyV1 {
    pub control_id: U4,
    pub work_id: CanonicalWorkId,
    pub control_digest: WireDigest,
    pub cleanup_pending: bool,
    pub committed_at: WireTimestamp,
    pub outcome: CancelReceiptBodyV1Outcome,
    pub local_work_id: crate::domain::WorkId,
    pub work_state: CancelReceiptBodyV1WorkState,
    pub work_state_revision: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CancelReceiptBodyV2Outcome {
    #[serde(rename = "reconciliation_required")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReceiptBodyV2 {
    pub control_id: U4,
    pub work_id: CanonicalWorkId,
    pub control_digest: WireDigest,
    pub cleanup_pending: bool,
    pub committed_at: WireTimestamp,
    pub outcome: CancelReceiptBodyV2Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_work_id: Option<crate::domain::WorkId>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CancelReceiptBody {
    V0(CancelReceiptBodyV0),
    V1(CancelReceiptBodyV1),
    V2(CancelReceiptBodyV2),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelReceipt {
    pub protocol: CancelReceiptProtocol,
    pub version: Version,
    pub r#type: CancelReceiptType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: CancelReceiptBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV0Provider {
    #[serde(rename = "telegram")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV0EventKind {
    #[serde(rename = "text")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV0Disposition {
    #[serde(rename = "accepted")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedIngressV0 {
    pub version: u32,
    pub ingress_id: U4,
    pub environment: Environment,
    pub provider: NormalizedIngressV0Provider,
    pub bot_id: PositiveI64,
    pub user_id: PositiveI64,
    pub chat_id: ChatId,
    pub update_id: DecimalI64,
    pub message_id: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_occurred_at: Option<WireTimestamp>,
    pub received_at: WireTimestamp,
    pub material_digest: WireDigest,
    pub event_kind: NormalizedIngressV0EventKind,
    pub disposition: NormalizedIngressV0Disposition,
    pub reply_binding_id: U4,
    pub reply_binding_version: PositiveI64,
    pub canonical_work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub conversation_sequence: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV1Provider {
    #[serde(rename = "telegram")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV1EventKind {
    #[serde(rename = "cancel")]
    V0,
    #[serde(rename = "link")]
    V1,
    #[serde(rename = "unsupported")]
    V2,
    #[serde(rename = "binding_revoked")]
    V3,
    #[serde(rename = "text")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum NormalizedIngressV1Disposition {
    #[serde(rename = "control")]
    V0,
    #[serde(rename = "link_pending")]
    V1,
    #[serde(rename = "unsupported")]
    V2,
    #[serde(rename = "denied")]
    V3,
    #[serde(rename = "held")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedIngressV1 {
    pub version: u32,
    pub ingress_id: U4,
    pub environment: Environment,
    pub provider: NormalizedIngressV1Provider,
    pub bot_id: PositiveI64,
    pub user_id: PositiveI64,
    pub chat_id: ChatId,
    pub update_id: DecimalI64,
    pub message_id: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_occurred_at: Option<WireTimestamp>,
    pub received_at: WireTimestamp,
    pub material_digest: WireDigest,
    pub event_kind: NormalizedIngressV1EventKind,
    pub disposition: NormalizedIngressV1Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_binding_id: Option<U4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_binding_version: Option<PositiveI64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NormalizedIngress {
    V0(NormalizedIngressV0),
    V1(NormalizedIngressV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ArtifactRefMediaType {
    #[serde(rename = "text/plain")]
    V0,
    #[serde(rename = "application/json")]
    V1,
    #[serde(rename = "application/octet-stream")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub artifact_id: U7,
    pub payload_sha256: WireDigest,
    pub byte_count: DecimalI64,
    pub media_type: ArtifactRefMediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV0ExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV0WorkState {
    #[serde(rename = "completed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV0TerminalReason {
    #[serde(rename = "answered")]
    V0,
    #[serde(rename = "refused")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSnapshotV0 {
    pub snapshot_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub origin_installation_id: InstallationId,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub export_id: U7,
    pub material_digest: WireDigest,
    pub origin_generation: PositiveI64,
    pub reply_binding: ReplyBinding,
    pub export_revision: ResultSnapshotV0ExportRevision,
    pub work_state_revision: PositiveI64,
    pub completed_at: WireTimestamp,
    pub content: Content,
    pub content_sha256: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
    pub work_state: ResultSnapshotV0WorkState,
    pub terminal_reason: ResultSnapshotV0TerminalReason,
    pub local_message_id: U7,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV1ExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV1WorkState {
    #[serde(rename = "failed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV1TerminalReason {
    #[serde(rename = "context_limit")]
    V0,
    #[serde(rename = "model_attempts_exhausted")]
    V1,
    #[serde(rename = "agent_loop_limit")]
    V2,
    #[serde(rename = "tool_calls_limit")]
    V3,
    #[serde(rename = "provider_failure")]
    V4,
    #[serde(rename = "invalid_model_output")]
    V5,
    #[serde(rename = "internal_failure")]
    V6,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSnapshotV1 {
    pub snapshot_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub origin_installation_id: InstallationId,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub export_id: U7,
    pub material_digest: WireDigest,
    pub origin_generation: PositiveI64,
    pub reply_binding: ReplyBinding,
    pub export_revision: ResultSnapshotV1ExportRevision,
    pub work_state_revision: PositiveI64,
    pub completed_at: WireTimestamp,
    pub content: Content,
    pub content_sha256: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
    pub work_state: ResultSnapshotV1WorkState,
    pub terminal_reason: ResultSnapshotV1TerminalReason,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV2ExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV2WorkState {
    #[serde(rename = "cancelled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV2TerminalReason {
    #[serde(rename = "user_request")]
    V0,
    #[serde(rename = "authorization_expired")]
    V1,
    #[serde(rename = "operator_stop")]
    V2,
    #[serde(rename = "graceful_shutdown")]
    V3,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSnapshotV2 {
    pub snapshot_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub origin_installation_id: InstallationId,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub export_id: U7,
    pub material_digest: WireDigest,
    pub origin_generation: PositiveI64,
    pub reply_binding: ReplyBinding,
    pub export_revision: ResultSnapshotV2ExportRevision,
    pub work_state_revision: PositiveI64,
    pub completed_at: WireTimestamp,
    pub content: Content,
    pub content_sha256: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
    pub work_state: ResultSnapshotV2WorkState,
    pub terminal_reason: ResultSnapshotV2TerminalReason,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV3ExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV3WorkState {
    #[serde(rename = "interrupted")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultSnapshotV3TerminalReason {
    #[serde(rename = "runtime_ownership_lost")]
    V0,
    #[serde(rename = "provider_outcome_unknown")]
    V1,
    #[serde(rename = "tool_interrupted_before_dispatch")]
    V2,
    #[serde(rename = "tool_outcome_unknown")]
    V3,
    #[serde(rename = "cleanup_unconfirmed")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSnapshotV3 {
    pub snapshot_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub conversation_id: CanonicalConversationId,
    pub origin_installation_id: InstallationId,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub export_id: U7,
    pub material_digest: WireDigest,
    pub origin_generation: PositiveI64,
    pub reply_binding: ReplyBinding,
    pub export_revision: ResultSnapshotV3ExportRevision,
    pub work_state_revision: PositiveI64,
    pub completed_at: WireTimestamp,
    pub content: Content,
    pub content_sha256: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
    pub work_state: ResultSnapshotV3WorkState,
    pub terminal_reason: ResultSnapshotV3TerminalReason,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResultSnapshot {
    V0(ResultSnapshotV0),
    V1(ResultSnapshotV1),
    V2(ResultSnapshotV2),
    V3(ResultSnapshotV3),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultExportProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultExportType {
    #[serde(rename = "result_export")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultExportBody {
    pub snapshot: ResultSnapshot,
    pub export_digest: WireDigest,
    pub upload_id: U4,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage_authorization_id: Option<U4>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultExport {
    pub protocol: ResultExportProtocol,
    pub version: Version,
    pub r#type: ResultExportType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ResultExportBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultQueryProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultQueryType {
    #[serde(rename = "result_query")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultQueryBodyExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultQueryBody {
    pub work_id: CanonicalWorkId,
    pub export_id: U7,
    pub export_revision: ResultQueryBodyExportRevision,
    pub export_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultQuery {
    pub protocol: ResultQueryProtocol,
    pub version: Version,
    pub r#type: ResultQueryType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ResultQueryBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultAcceptanceProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultAcceptanceType {
    #[serde(rename = "result_acceptance")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ResultAcceptanceBodyExportRevision {
    #[serde(rename = "1")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultAcceptanceBody {
    pub work_id: CanonicalWorkId,
    pub export_id: U7,
    pub export_revision: ResultAcceptanceBodyExportRevision,
    pub export_digest: WireDigest,
    pub acceptance_id: U4,
    pub outbox_source_id: U4,
    pub accepted_at: WireTimestamp,
    pub duplicate: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultAcceptance {
    pub protocol: ResultAcceptanceProtocol,
    pub version: Version,
    pub r#type: ResultAcceptanceType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ResultAcceptanceBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StorageRecovery {
    #[serde(rename = "pending")]
    V0,
    #[serde(rename = "complete")]
    V1,
    #[serde(rename = "failed")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub disk_lineage_id: U4,
    pub filesystem_uuid: WireUuid,
    pub sqlite_schema: PositiveI64,
    pub recovery: StorageRecovery,
    pub binds_verified: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HeartbeatProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HeartbeatType {
    #[serde(rename = "heartbeat")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum HeartbeatBodyReadiness {
    #[serde(rename = "unready")]
    V0,
    #[serde(rename = "storage_ready")]
    V1,
    #[serde(rename = "acceptance_ready")]
    V2,
    #[serde(rename = "operational_ready")]
    V3,
    #[serde(rename = "draining")]
    V4,
    #[serde(rename = "degraded")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatBody {
    pub power_operation_id: U4,
    pub boot_id: WireUuid,
    pub runtime_id: U7,
    pub heartbeat_sequence: PositiveI64,
    pub observed_at: WireTimestamp,
    pub storage: Storage,
    pub readiness: HeartbeatBodyReadiness,
    pub readiness_revision: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config_id: Option<U4>,
    pub active_work_count: DecimalI64,
    pub queued_work_count: DecimalI64,
    pub pending_receipt_ack_count: DecimalI64,
    pub pending_export_count: DecimalI64,
    pub descendant_count: DecimalI64,
    pub uncommitted_state: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub protocol: HeartbeatProtocol,
    pub version: Version,
    pub r#type: HeartbeatType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: HeartbeatBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkStatusProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkStatusType {
    #[serde(rename = "work_status")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum WorkStatusBodyWorkState {
    #[serde(rename = "queued")]
    V0,
    #[serde(rename = "running")]
    V1,
    #[serde(rename = "waiting_on_model")]
    V2,
    #[serde(rename = "waiting_on_tool")]
    V3,
    #[serde(rename = "cancel_requested")]
    V4,
    #[serde(rename = "completed")]
    V5,
    #[serde(rename = "failed")]
    V6,
    #[serde(rename = "cancelled")]
    V7,
    #[serde(rename = "interrupted")]
    V8,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkStatusBody {
    pub work_id: CanonicalWorkId,
    pub receipt_id: U7,
    pub work_state: WorkStatusBodyWorkState,
    pub work_state_revision: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkStatus {
    pub protocol: WorkStatusProtocol,
    pub version: Version,
    pub r#type: WorkStatusType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: WorkStatusBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerIdentityAccountState {
    #[serde(rename = "active")]
    V0,
    #[serde(rename = "disabled")]
    V1,
    #[serde(rename = "deleting")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerIdentity {
    pub customer_id: CentralCustomerId,
    pub issuer: String,
    pub subject: String,
    pub account_state: CustomerIdentityAccountState,
    pub revision: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusAccountState {
    #[serde(rename = "active")]
    V0,
    #[serde(rename = "disabled")]
    V1,
    #[serde(rename = "deleting")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusEntitlementState {
    #[serde(rename = "absent")]
    V0,
    #[serde(rename = "trial")]
    V1,
    #[serde(rename = "paid")]
    V2,
    #[serde(rename = "expired")]
    V3,
    #[serde(rename = "exhausted")]
    V4,
    #[serde(rename = "suspended")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusActivationPhase {
    #[serde(rename = "awaiting_link")]
    V0,
    #[serde(rename = "queued")]
    V1,
    #[serde(rename = "provisioning")]
    V2,
    #[serde(rename = "enrolled")]
    V3,
    #[serde(rename = "storage_ready")]
    V4,
    #[serde(rename = "ready")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusMachinePower {
    #[serde(rename = "unknown")]
    V0,
    #[serde(rename = "pending")]
    V1,
    #[serde(rename = "running")]
    V2,
    #[serde(rename = "stopping")]
    V3,
    #[serde(rename = "stopped")]
    V4,
    #[serde(rename = "terminated")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusReadiness {
    #[serde(rename = "unready")]
    V0,
    #[serde(rename = "storage_ready")]
    V1,
    #[serde(rename = "acceptance_ready")]
    V2,
    #[serde(rename = "operational_ready")]
    V3,
    #[serde(rename = "draining")]
    V4,
    #[serde(rename = "degraded")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusDisposition {
    #[serde(rename = "pending")]
    V0,
    #[serde(rename = "delayed")]
    V1,
    #[serde(rename = "degraded")]
    V2,
    #[serde(rename = "support_needed")]
    V3,
    #[serde(rename = "normal")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum CustomerStatusReason {
    #[serde(rename = "schema_invalid")]
    V0,
    #[serde(rename = "unsupported_version")]
    V1,
    #[serde(rename = "unsupported_canonicalization")]
    V2,
    #[serde(rename = "unsupported_signature")]
    V3,
    #[serde(rename = "unsupported_capability")]
    V4,
    #[serde(rename = "unauthenticated")]
    V5,
    #[serde(rename = "session_expired")]
    V6,
    #[serde(rename = "unauthorized_scope")]
    V7,
    #[serde(rename = "customer_mismatch")]
    V8,
    #[serde(rename = "installation_mismatch")]
    V9,
    #[serde(rename = "stale_generation")]
    V10,
    #[serde(rename = "revoked_authority")]
    V11,
    #[serde(rename = "stale_lease")]
    V12,
    #[serde(rename = "stale_resource_fence")]
    V13,
    #[serde(rename = "stale_resource_revision")]
    V14,
    #[serde(rename = "digest_conflict")]
    V15,
    #[serde(rename = "result_conflict")]
    V16,
    #[serde(rename = "result_revision_invalid")]
    V17,
    #[serde(rename = "provider_effect_unknown")]
    V18,
    #[serde(rename = "reconciliation_required")]
    V19,
    #[serde(rename = "restored_interval_missing")]
    V20,
    #[serde(rename = "backend_unavailable")]
    V21,
    #[serde(rename = "temporary_failure")]
    V22,
    #[serde(rename = "manual_resolution")]
    V23,
    #[serde(rename = "compute_budget_exhausted")]
    V24,
    #[serde(rename = "model_budget_unavailable")]
    V25,
    #[serde(rename = "entitlement_expired")]
    V26,
    #[serde(rename = "policy_unapproved")]
    V27,
    #[serde(rename = "proof_missing")]
    V28,
    #[serde(rename = "proof_expired")]
    V29,
    #[serde(rename = "consent_required")]
    V30,
    #[serde(rename = "compute_authorization_expired")]
    V31,
    #[serde(rename = "authorization_epoch_stale")]
    V32,
    #[serde(rename = "terminal_cancellation")]
    V33,
    #[serde(rename = "attempt_budget_exhausted")]
    V34,
    #[serde(rename = "not_found")]
    V35,
    #[serde(rename = "schema_incompatible")]
    V36,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerStatus {
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub account_state: CustomerStatusAccountState,
    pub entitlement_state: CustomerStatusEntitlementState,
    pub activation_phase: CustomerStatusActivationPhase,
    pub machine_power: CustomerStatusMachinePower,
    pub readiness: CustomerStatusReadiness,
    pub disposition: CustomerStatusDisposition,
    pub status_revision: PositiveI64,
    pub available_compute_seconds: DecimalI64,
    pub available_model_usd_micros: DecimalI64,
    pub reason: CustomerStatusReason,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainRequestProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainRequestType {
    #[serde(rename = "drain_request")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainRequestBodyReason {
    #[serde(rename = "idle")]
    V0,
    #[serde(rename = "budget")]
    V1,
    #[serde(rename = "backup")]
    V2,
    #[serde(rename = "update")]
    V3,
    #[serde(rename = "replace")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainRequestBody {
    pub drain_id: U4,
    pub lifecycle_operation_id: U4,
    pub power_operation_id: U4,
    pub resource_revision: PositiveI64,
    pub ingress_revision: DecimalI64,
    pub boot_id: WireUuid,
    pub reason: DrainRequestBodyReason,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainRequest {
    pub protocol: DrainRequestProtocol,
    pub version: Version,
    pub r#type: DrainRequestType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: DrainRequestBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainAckProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainAckType {
    #[serde(rename = "drain_ack")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainAckBody {
    pub drain_id: U4,
    pub lifecycle_operation_id: U4,
    pub power_operation_id: U4,
    pub resource_revision: PositiveI64,
    pub ingress_revision: DecimalI64,
    pub boot_id: WireUuid,
    pub drain_token: U4,
    pub local_revision: PositiveI64,
    pub claims_closed: bool,
    pub quiescent: bool,
    pub active_work_count: DecimalI64,
    pub queued_work_count: DecimalI64,
    pub pending_receipt_ack_count: DecimalI64,
    pub pending_export_count: DecimalI64,
    pub descendant_count: DecimalI64,
    pub uncommitted_state: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainAck {
    pub protocol: DrainAckProtocol,
    pub version: Version,
    pub r#type: DrainAckType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: DrainAckBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainCancelProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum DrainCancelType {
    #[serde(rename = "drain_cancel")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainCancelBody {
    pub drain_id: U4,
    pub lifecycle_operation_id: U4,
    pub power_operation_id: U4,
    pub resource_revision: PositiveI64,
    pub ingress_revision: DecimalI64,
    pub boot_id: WireUuid,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainCancel {
    pub protocol: DrainCancelProtocol,
    pub version: Version,
    pub r#type: DrainCancelType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: DrainCancelBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum PowerProjectionDesiredPower {
    #[serde(rename = "running")]
    V0,
    #[serde(rename = "stopped")]
    V1,
    #[serde(rename = "quarantined")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum PowerProjectionObservedPower {
    #[serde(rename = "unknown")]
    V0,
    #[serde(rename = "pending")]
    V1,
    #[serde(rename = "running")]
    V2,
    #[serde(rename = "stopping")]
    V3,
    #[serde(rename = "stopped")]
    V4,
    #[serde(rename = "terminated")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum PowerProjectionReadiness {
    #[serde(rename = "unready")]
    V0,
    #[serde(rename = "storage_ready")]
    V1,
    #[serde(rename = "acceptance_ready")]
    V2,
    #[serde(rename = "operational_ready")]
    V3,
    #[serde(rename = "draining")]
    V4,
    #[serde(rename = "degraded")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum PowerProjectionDisposition {
    #[serde(rename = "pending")]
    V0,
    #[serde(rename = "delayed")]
    V1,
    #[serde(rename = "degraded")]
    V2,
    #[serde(rename = "support_needed")]
    V3,
    #[serde(rename = "normal")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerProjection {
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub generation: PositiveI64,
    pub power_operation_id: U4,
    pub resource_revision: PositiveI64,
    pub desired_power: PowerProjectionDesiredPower,
    pub observed_power: PowerProjectionObservedPower,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<WireUuid>,
    pub readiness: PowerProjectionReadiness,
    pub disposition: PowerProjectionDisposition,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableJobInput {
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub obligation_id: U4,
    pub intent_id: U4,
    pub job_id: U4,
    pub resource_scope_id: U4,
    pub step: String,
    pub kind: String,
    pub contract_version: u32,
    pub request_digest: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<PositiveI64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthorityV0 {
    pub resource_scope_id: U4,
    pub operation_id: U4,
    pub resource_revision: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_generation: Option<PositiveI64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthorityV1 {
    pub resource_scope_id: U4,
    pub operation_id: U4,
    pub resource_revision: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_generation: Option<PositiveI64>,
    pub job_id: U4,
    pub job_attempt_id: U4,
    pub job_fence: PositiveI64,
    pub resource_fence: PositiveI64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceAuthority {
    V0(ResourceAuthorityV0),
    V1(ResourceAuthorityV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ExpectedCloudInstanceProvider {
    #[serde(rename = "aws")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCloudInstance {
    pub expectation_id: U4,
    pub installation_id: InstallationId,
    pub logical_agent_id: LogicalAgentId,
    pub customer_id: CentralCustomerId,
    pub launch_operation_id: U4,
    pub expected_disk_lineage_id: U4,
    pub generation: PositiveI64,
    pub provider: ExpectedCloudInstanceProvider,
    pub account_id: String,
    pub region: String,
    pub availability_zone: String,
    pub instance_id: String,
    pub ami_id: String,
    pub data_volume_id: String,
    pub expected_config_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV0Provider {
    #[serde(rename = "aws")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV0Phase {
    #[serde(rename = "pending")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupLineageV0 {
    pub lineage_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_installation_id: InstallationId,
    pub source_disk_lineage_id: U4,
    pub operation_id: U4,
    pub source_generation: PositiveI64,
    pub sqlite_schema: PositiveI64,
    pub provider: BackupLineageV0Provider,
    pub account_id: String,
    pub region: String,
    pub availability_zone: String,
    pub source_volume_id: String,
    pub filesystem_uuid: WireUuid,
    pub agent_release_id: String,
    pub agent_release_digest: WireDigest,
    pub contract_bundle_digest: WireDigest,
    pub phase: BackupLineageV0Phase,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV1Provider {
    #[serde(rename = "aws")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV1Phase {
    #[serde(rename = "available")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupLineageV1 {
    pub lineage_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_installation_id: InstallationId,
    pub source_disk_lineage_id: U4,
    pub operation_id: U4,
    pub source_generation: PositiveI64,
    pub sqlite_schema: PositiveI64,
    pub provider: BackupLineageV1Provider,
    pub account_id: String,
    pub region: String,
    pub availability_zone: String,
    pub source_volume_id: String,
    pub filesystem_uuid: WireUuid,
    pub agent_release_id: String,
    pub agent_release_digest: WireDigest,
    pub contract_bundle_digest: WireDigest,
    pub phase: BackupLineageV1Phase,
    pub snapshot_id: String,
    pub captured_at: WireTimestamp,
    pub consistency_evidence_id: U4,
    pub completion_evidence_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV2Provider {
    #[serde(rename = "aws")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BackupLineageV2Phase {
    #[serde(rename = "restore_verified")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupLineageV2 {
    pub lineage_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_installation_id: InstallationId,
    pub source_disk_lineage_id: U4,
    pub operation_id: U4,
    pub source_generation: PositiveI64,
    pub sqlite_schema: PositiveI64,
    pub provider: BackupLineageV2Provider,
    pub account_id: String,
    pub region: String,
    pub availability_zone: String,
    pub source_volume_id: String,
    pub filesystem_uuid: WireUuid,
    pub agent_release_id: String,
    pub agent_release_digest: WireDigest,
    pub contract_bundle_digest: WireDigest,
    pub phase: BackupLineageV2Phase,
    pub snapshot_id: String,
    pub captured_at: WireTimestamp,
    pub consistency_evidence_id: U4,
    pub completion_evidence_id: U4,
    pub restore_verification_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BackupLineage {
    V0(BackupLineageV0),
    V1(BackupLineageV1),
    V2(BackupLineageV2),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV0Currency {
    #[serde(rename = "USD")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV0Kind {
    #[serde(rename = "trial")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV0ComputeAllowanceSeconds {
    #[serde(rename = "43200")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV0ModelAllowanceUsdMicros {
    #[serde(rename = "5000000")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementPeriodV0 {
    pub period_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_evidence_id: U4,
    pub starts_at: WireTimestamp,
    pub ends_at: WireTimestamp,
    pub revision: PositiveI64,
    pub authorization_epoch: PositiveI64,
    pub currency: EntitlementPeriodV0Currency,
    pub kind: EntitlementPeriodV0Kind,
    pub compute_allowance_seconds: EntitlementPeriodV0ComputeAllowanceSeconds,
    pub model_allowance_usd_micros: EntitlementPeriodV0ModelAllowanceUsdMicros,
    pub policy_decision_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV1Currency {
    #[serde(rename = "USD")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV1Kind {
    #[serde(rename = "paid")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV1ComputeAllowanceSeconds {
    #[serde(rename = "648000")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EntitlementPeriodV1ModelAllowanceUsdMicros {
    #[serde(rename = "20000000")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementPeriodV1 {
    pub period_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_evidence_id: U4,
    pub starts_at: WireTimestamp,
    pub ends_at: WireTimestamp,
    pub revision: PositiveI64,
    pub authorization_epoch: PositiveI64,
    pub currency: EntitlementPeriodV1Currency,
    pub kind: EntitlementPeriodV1Kind,
    pub compute_allowance_seconds: EntitlementPeriodV1ComputeAllowanceSeconds,
    pub model_allowance_usd_micros: EntitlementPeriodV1ModelAllowanceUsdMicros,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntitlementPeriod {
    V0(EntitlementPeriodV0),
    V1(EntitlementPeriodV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BillingEvidenceCurrency {
    #[serde(rename = "USD")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BillingEvidenceMinorUnits {
    #[serde(rename = "4900")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum BillingEvidenceProvenance {
    #[serde(rename = "provider_signature")]
    V0,
    #[serde(rename = "provider_query")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingEvidence {
    pub evidence_id: U4,
    pub customer_id: CentralCustomerId,
    pub billable_period_id: U4,
    pub environment: Environment,
    pub provider_namespace: String,
    pub provider_event_id: String,
    pub currency: BillingEvidenceCurrency,
    pub minor_units: BillingEvidenceMinorUnits,
    pub scale: u32,
    pub evidence_digest: WireDigest,
    pub provenance: BillingEvidenceProvenance,
    pub observed_at: WireTimestamp,
    pub starts_at: WireTimestamp,
    pub ends_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EligibilityResultV0Decision {
    #[serde(rename = "denied")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EligibilityResultV0Reason {
    #[serde(rename = "policy_unapproved")]
    V0,
    #[serde(rename = "proof_missing")]
    V1,
    #[serde(rename = "proof_expired")]
    V2,
    #[serde(rename = "consent_required")]
    V3,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityResultV0 {
    pub decision: EligibilityResultV0Decision,
    pub reason: EligibilityResultV0Reason,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EligibilityResultV1Decision {
    #[serde(rename = "eligible")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EligibilityResultV1Environment {
    #[serde(rename = "development")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum EligibilityResultV1Provenance {
    #[serde(rename = "synthetic_test")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityResultV1 {
    pub decision_id: U4,
    pub policy_decision_id: U4,
    pub customer_id: CentralCustomerId,
    pub proof_id: U4,
    pub consent_receipt_id: U4,
    pub decision: EligibilityResultV1Decision,
    pub environment: EligibilityResultV1Environment,
    pub provenance: EligibilityResultV1Provenance,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EligibilityResult {
    V0(EligibilityResultV0),
    V1(EligibilityResultV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SpendReservationV0State {
    #[serde(rename = "reserved")]
    V0,
    #[serde(rename = "dispatch_intended")]
    V1,
    #[serde(rename = "uncertain")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendReservationV0 {
    pub reservation_id: U4,
    pub period_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_operation_id: U4,
    pub attempt_id: U7,
    pub amount_usd_micros: DecimalI64,
    pub revision: PositiveI64,
    pub state: SpendReservationV0State,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SpendReservationV1State {
    #[serde(rename = "settled")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendReservationV1 {
    pub reservation_id: U4,
    pub period_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_operation_id: U4,
    pub attempt_id: U7,
    pub amount_usd_micros: DecimalI64,
    pub revision: PositiveI64,
    pub state: SpendReservationV1State,
    pub settlement_id: U4,
    pub evidence_id: U4,
    pub actual_usd_micros: DecimalI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum SpendReservationV2State {
    #[serde(rename = "released")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendReservationV2 {
    pub reservation_id: U4,
    pub period_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub source_operation_id: U4,
    pub attempt_id: U7,
    pub amount_usd_micros: DecimalI64,
    pub revision: PositiveI64,
    pub state: SpendReservationV2State,
    pub release_evidence_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpendReservation {
    V0(SpendReservationV0),
    V1(SpendReservationV1),
    V2(SpendReservationV2),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum PriceCategoryUnit {
    #[serde(rename = "token")]
    V0,
    #[serde(rename = "request")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCategory {
    pub category: String,
    pub quantity: DecimalI64,
    pub numerator: DecimalI64,
    pub denominator: PositiveI64,
    pub unit: PriceCategoryUnit,
    pub price_config_id: U4,
    pub price_config_digest: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ComputeGrantScopesItem {
    #[serde(rename = "status")]
    V0,
    #[serde(rename = "receipt")]
    V1,
    #[serde(rename = "export")]
    V2,
    #[serde(rename = "cancel")]
    V3,
    #[serde(rename = "bounded_execute")]
    V4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeGrant {
    pub grant_version: u32,
    pub audience: String,
    pub environment: Environment,
    pub grant_id: U4,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub installation_id: InstallationId,
    pub power_operation_id: U4,
    pub period_id: U4,
    pub compute_reservation_id: U4,
    pub generation: PositiveI64,
    pub authorization_epoch: PositiveI64,
    pub max_elapsed_ms: PositiveI64,
    pub boot_id: WireUuid,
    pub issued_at: WireTimestamp,
    pub expires_at: WireTimestamp,
    pub permitted_work_ids: Vec<U4>,
    pub scopes: Vec<ComputeGrantScopesItem>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ComputeAuthorizationProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ComputeAuthorizationType {
    #[serde(rename = "compute_authorization")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeAuthorizationBody {
    pub grant: ComputeGrant,
    pub signature: Binary64,
    pub key_id: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeAuthorization {
    pub protocol: ComputeAuthorizationProtocol,
    pub version: Version,
    pub r#type: ComputeAuthorizationType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ComputeAuthorizationBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ComputeRenewProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ComputeRenewType {
    #[serde(rename = "compute_renew")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeRenewBody {
    pub grant_id: U4,
    pub period_id: U4,
    pub power_operation_id: U4,
    pub authorization_epoch: PositiveI64,
    pub boot_id: WireUuid,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeRenew {
    pub protocol: ComputeRenewProtocol,
    pub version: Version,
    pub r#type: ComputeRenewType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ComputeRenewBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV0Kind {
    #[serde(rename = "message")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV0Role {
    #[serde(rename = "system")]
    V0,
    #[serde(rename = "developer")]
    V1,
    #[serde(rename = "user")]
    V2,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV0 {
    pub kind: InputItemV0Kind,
    pub role: InputItemV0Role,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV1Kind {
    #[serde(rename = "prior_assistant")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV1 {
    pub kind: InputItemV1Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV2Kind {
    #[serde(rename = "historical_refusal")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV2 {
    pub kind: InputItemV2Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV3Kind {
    #[serde(rename = "historical_reasoning_summary")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV3 {
    pub kind: InputItemV3Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV4Kind {
    #[serde(rename = "tool_call")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV4 {
    pub kind: InputItemV4Kind,
    pub call_id: String,
    pub name: String,
    pub raw_arguments: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV5Kind {
    #[serde(rename = "tool_result")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV5 {
    pub kind: InputItemV5Kind,
    pub call_id: String,
    pub result_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV6Kind {
    #[serde(rename = "structured_data")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV6 {
    pub kind: InputItemV6Kind,
    pub data_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV7Kind {
    #[serde(rename = "synthetic_runtime_status")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV7 {
    pub kind: InputItemV7Kind,
    pub status: String,
    pub details_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum InputItemV8Kind {
    #[serde(rename = "provider_opaque_continuation")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputItemV8 {
    pub kind: InputItemV8Kind,
    pub provider_namespace: String,
    pub type_label: String,
    pub opaque: String,
    pub opaque_sha256: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputItem {
    V0(InputItemV0),
    V1(InputItemV1),
    V2(InputItemV2),
    V3(InputItemV3),
    V4(InputItemV4),
    V5(InputItemV5),
    V6(InputItemV6),
    V7(InputItemV7),
    V8(InputItemV8),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV0Kind {
    #[serde(rename = "text")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV0 {
    pub kind: OutputItemV0Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV1Kind {
    #[serde(rename = "refusal")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV1 {
    pub kind: OutputItemV1Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV2Kind {
    #[serde(rename = "reasoning_summary")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV2 {
    pub kind: OutputItemV2Kind,
    pub content_parts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV3Kind {
    #[serde(rename = "tool_call")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV3 {
    pub kind: OutputItemV3Kind,
    pub call_id: String,
    pub name: String,
    pub raw_arguments: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV4Kind {
    #[serde(rename = "structured_data")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV4 {
    pub kind: OutputItemV4Kind,
    pub data_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV5Kind {
    #[serde(rename = "provider_opaque")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV5 {
    pub kind: OutputItemV5Kind,
    pub provider_namespace: String,
    pub type_label: String,
    pub opaque: String,
    pub opaque_sha256: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum OutputItemV6Kind {
    #[serde(rename = "unknown_provider_item")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputItemV6 {
    pub kind: OutputItemV6Kind,
    pub provider_namespace: String,
    pub type_label: String,
    pub opaque: String,
    pub opaque_sha256: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputItem {
    V0(OutputItemV0),
    V1(OutputItemV1),
    V2(OutputItemV2),
    V3(OutputItemV3),
    V4(OutputItemV4),
    V5(OutputItemV5),
    V6(OutputItemV6),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub version: PositiveI64,
    pub schema_version: u32,
    pub schema_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelRequestToolChoice {
    #[serde(rename = "none")]
    V0,
    #[serde(rename = "automatic")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    pub instructions: Vec<String>,
    pub ordered_input_items: Vec<InputItem>,
    pub tool_definitions: Vec<ToolDefinition>,
    pub requested_output_tokens: PositiveI64,
    pub tool_choice: ModelRequestToolChoice,
    pub parallel_tool_calls: bool,
    pub reasoning_continuation: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvocationMaterial {
    pub invocation_version: u32,
    pub environment: Environment,
    pub customer_id: CentralCustomerId,
    pub logical_agent_id: LogicalAgentId,
    pub work_id: CanonicalWorkId,
    pub period_id: U4,
    pub model_config_id: U4,
    pub price_config_id: U4,
    pub receipt_id: U7,
    pub local_work_id: crate::domain::WorkId,
    pub logical_invocation_id: U7,
    pub attempt_id: U7,
    pub context_manifest_id: U7,
    pub material_digest: WireDigest,
    pub attempt_ordinal: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of_attempt_id: Option<U7>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_authorization_id: Option<U4>,
    pub model_config_digest: WireDigest,
    pub price_config_digest: WireDigest,
    pub request: ModelRequest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelInvokeProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelInvokeType {
    #[serde(rename = "model_invoke")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvokeBody {
    pub invocation: ModelInvocationMaterial,
    pub invocation_digest: WireDigest,
    pub compute_grant_id: U4,
    pub authorization_epoch: PositiveI64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<U4>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvoke {
    pub protocol: ModelInvokeProtocol,
    pub version: Version,
    pub r#type: ModelInvokeType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ModelInvokeBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum UsageOutcomeV0UsageStatus {
    #[serde(rename = "reported")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageOutcomeV0 {
    pub usage_status: UsageOutcomeV0UsageStatus,
    pub evidence_id: U4,
    pub evidence_digest: WireDigest,
    pub categories: Vec<PriceCategory>,
    pub settlement_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum UsageOutcomeV1UsageStatus {
    #[serde(rename = "unavailable")]
    V0,
    #[serde(rename = "unknown")]
    V1,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum UsageOutcomeV1Liability {
    #[serde(rename = "retained")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageOutcomeV1 {
    pub usage_status: UsageOutcomeV1UsageStatus,
    pub reservation_id: U4,
    pub liability: UsageOutcomeV1Liability,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UsageOutcome {
    V0(UsageOutcomeV0),
    V1(UsageOutcomeV1),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV0Kind {
    #[serde(rename = "response_started")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV0 {
    pub kind: StreamEventV0Kind,
    pub model_config_id: U4,
    pub reservation_id: U4,
    pub model_config_digest: WireDigest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<U4>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV1Kind {
    #[serde(rename = "text_delta")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV1 {
    pub kind: StreamEventV1Kind,
    pub item_ordinal: DecimalI64,
    pub delta: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV2Kind {
    #[serde(rename = "reasoning_summary_delta")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV2 {
    pub kind: StreamEventV2Kind,
    pub item_ordinal: DecimalI64,
    pub delta: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV3Kind {
    #[serde(rename = "refusal_delta")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV3 {
    pub kind: StreamEventV3Kind,
    pub item_ordinal: DecimalI64,
    pub delta: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV4Kind {
    #[serde(rename = "tool_call_started")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV4 {
    pub kind: StreamEventV4Kind,
    pub item_ordinal: DecimalI64,
    pub call_id: String,
    pub name: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV5Kind {
    #[serde(rename = "tool_argument_delta")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV5 {
    pub kind: StreamEventV5Kind,
    pub item_ordinal: DecimalI64,
    pub call_id: String,
    pub delta: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV6Kind {
    #[serde(rename = "tool_call_completed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV6 {
    pub kind: StreamEventV6Kind,
    pub item_ordinal: DecimalI64,
    pub call_id: String,
    pub name: String,
    pub raw_arguments: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV7Kind {
    #[serde(rename = "refusal_completed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV7 {
    pub kind: StreamEventV7Kind,
    pub item_ordinal: DecimalI64,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV8Kind {
    #[serde(rename = "structured_data")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV8 {
    pub kind: StreamEventV8Kind,
    pub item_ordinal: DecimalI64,
    pub data_json: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV9Kind {
    #[serde(rename = "usage")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV9OutcomeUsageStatus {
    #[serde(rename = "reported")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV9Outcome {
    pub usage_status: StreamEventV9OutcomeUsageStatus,
    pub evidence_id: U4,
    pub evidence_digest: WireDigest,
    pub categories: Vec<PriceCategory>,
    pub settlement_id: U4,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV9 {
    pub kind: StreamEventV9Kind,
    pub outcome: StreamEventV9Outcome,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV10Kind {
    #[serde(rename = "usage_unavailable")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV10 {
    pub kind: StreamEventV10Kind,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV11Kind {
    #[serde(rename = "completed")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV11StopReason {
    #[serde(rename = "completed")]
    V0,
    #[serde(rename = "tool_continuation")]
    V1,
    #[serde(rename = "refusal")]
    V2,
    #[serde(rename = "incomplete_provider_limit")]
    V3,
    #[serde(rename = "cancelled")]
    V4,
    #[serde(rename = "provider_failure")]
    V5,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV11 {
    pub kind: StreamEventV11Kind,
    pub output_items: Vec<OutputItem>,
    pub stop_reason: StreamEventV11StopReason,
    pub usage: UsageOutcome,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV12Kind {
    #[serde(rename = "provider_error")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV12ErrorKind {
    #[serde(rename = "authentication")]
    V0,
    #[serde(rename = "authorization")]
    V1,
    #[serde(rename = "invalid_request")]
    V2,
    #[serde(rename = "unknown_model")]
    V3,
    #[serde(rename = "rate_limited")]
    V4,
    #[serde(rename = "temporarily_unavailable")]
    V5,
    #[serde(rename = "transport_before_response")]
    V6,
    #[serde(rename = "transport_after_possible_processing")]
    V7,
    #[serde(rename = "timeout_before_output")]
    V8,
    #[serde(rename = "timeout_after_output")]
    V9,
    #[serde(rename = "malformed_response")]
    V10,
    #[serde(rename = "malformed_completed_tool_arguments")]
    V11,
    #[serde(rename = "output_too_large")]
    V12,
    #[serde(rename = "unsupported_response_item")]
    V13,
    #[serde(rename = "context_error")]
    V14,
    #[serde(rename = "safety_refusal")]
    V15,
    #[serde(rename = "cancelled")]
    V16,
    #[serde(rename = "provider_outcome_unknown")]
    V17,
    #[serde(rename = "internal_provider_error")]
    V18,
    #[serde(rename = "script_mismatch")]
    V19,
    #[serde(rename = "invalid_scripted_provider_program")]
    V20,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV12Certainty {
    #[serde(rename = "definitely_not_sent")]
    V0,
    #[serde(rename = "definite_provider_failure")]
    V1,
    #[serde(rename = "semantic_output_observed")]
    V2,
    #[serde(rename = "outcome_unknown")]
    V3,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV12 {
    pub kind: StreamEventV12Kind,
    pub error_kind: StreamEventV12ErrorKind,
    pub certainty: StreamEventV12Certainty,
    pub resolution: SafeError,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV13Kind {
    #[serde(rename = "unknown_provider_event")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV13 {
    pub kind: StreamEventV13Kind,
    pub provider_namespace: String,
    pub type_label: String,
    pub opaque: String,
    pub opaque_sha256: WireDigest,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV14Kind {
    #[serde(rename = "outcome_unknown")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum StreamEventV14Liability {
    #[serde(rename = "retained")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEventV14 {
    pub kind: StreamEventV14Kind,
    pub reservation_id: U4,
    pub liability: StreamEventV14Liability,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamEvent {
    V0(StreamEventV0),
    V1(StreamEventV1),
    V2(StreamEventV2),
    V3(StreamEventV3),
    V4(StreamEventV4),
    V5(StreamEventV5),
    V6(StreamEventV6),
    V7(StreamEventV7),
    V8(StreamEventV8),
    V9(StreamEventV9),
    V10(StreamEventV10),
    V11(StreamEventV11),
    V12(StreamEventV12),
    V13(StreamEventV13),
    V14(StreamEventV14),
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelEventProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelEventType {
    #[serde(rename = "model_event")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEventBody {
    pub attempt_id: U7,
    pub logical_invocation_id: U7,
    pub event_sequence: PositiveI64,
    pub event: StreamEvent,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvent {
    pub protocol: ModelEventProtocol,
    pub version: Version,
    pub r#type: ModelEventType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ModelEventBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelOutcomeProtocol {
    #[serde(rename = "craxii.worker")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum ModelOutcomeType {
    #[serde(rename = "model_outcome")]
    V0,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOutcomeBody {
    pub attempt_id: U7,
    pub logical_invocation_id: U7,
    pub outcome: UsageOutcome,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelOutcome {
    pub protocol: ModelOutcomeProtocol,
    pub version: Version,
    pub r#type: ModelOutcomeType,
    pub request_id: U4,
    pub environment: Environment,
    pub authority: Authority,
    pub body: ModelOutcomeBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Hello(Hello),
    SelectedProtocol(SelectedProtocol),
    Error(Error),
    EnrollmentChallenge(EnrollmentChallenge),
    EnrollmentProof(EnrollmentProof),
    EnrollmentAck(EnrollmentAck),
    SessionChallenge(SessionChallenge),
    SessionProof(SessionProof),
    SessionGrant(SessionGrant),
    SensitiveRequestProof(SensitiveRequestProof),
    ExecutionProfile(ExecutionProfile),
    WorkDelivery(WorkDelivery),
    WorkReceipt(WorkReceipt),
    ReceiptQuery(ReceiptQuery),
    ReceiptStatus(ReceiptStatus),
    CancelDelivery(CancelDelivery),
    CancelReceipt(CancelReceipt),
    ResultExport(ResultExport),
    ResultQuery(ResultQuery),
    ResultAcceptance(ResultAcceptance),
    Heartbeat(Heartbeat),
    WorkStatus(WorkStatus),
    DrainRequest(DrainRequest),
    DrainAck(DrainAck),
    DrainCancel(DrainCancel),
    ComputeAuthorization(ComputeAuthorization),
    ComputeRenew(ComputeRenew),
    ModelInvoke(ModelInvoke),
    ModelEvent(ModelEvent),
    ModelOutcome(ModelOutcome),
    ProtocolRejection(ProtocolRejection),
}
use super::{
    ContractError, ContractResult,
    canonical::{DigestDomain, canonical_bytes, canonical_with_limits, digest_material, sha256},
    invalid,
    primitives::{binary, timestamp_micros},
    raw_json::{LARGE_LIMITS, Limits, parse_bounded},
    signing::key_id,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};
const SCHEMAS: &[&str] = &[
    include_str!("../../../contracts/distribution/1.0.0/schemas/primitives.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/negotiation.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/authentication.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/work.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/result.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/status.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/lifecycle.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/accounting.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/compute.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/model.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/execution-profile.schema.json"),
    include_str!("../../../contracts/distribution/1.0.0/schemas/error.schema.json"),
];
static VALIDATORS: OnceLock<ContractResult<BTreeMap<String, jsonschema::Validator>>> =
    OnceLock::new();
fn validators() -> ContractResult<&'static BTreeMap<String, jsonschema::Validator>> {
    VALIDATORS
        .get_or_init(|| {
            let schemas = SCHEMAS
                .iter()
                .map(|s| serde_json::from_str::<Value>(s).map_err(|_| invalid()))
                .collect::<ContractResult<Vec<_>>>()?;
            let mut registry = jsonschema::Registry::new();
            for s in &schemas {
                registry = registry
                    .add(s["$id"].as_str().ok_or_else(invalid)?, s.clone())
                    .map_err(|_| invalid())?;
            }
            let registry = registry.prepare().map_err(|_| invalid())?;
            let mut result = BTreeMap::new();
            for s in &schemas {
                for name in s["definitions"].as_object().ok_or_else(invalid)?.keys() {
                    let uri = format!(
                        "{}#/definitions/{}",
                        s["$id"].as_str().ok_or_else(invalid)?,
                        name
                    );
                    let mut options = jsonschema::draft7::options()
                        .offline()
                        .with_registry(&registry)
                        .should_validate_formats(true)
                        .should_ignore_unknown_formats(false);
                    for kind in [
                        "U4",
                        "U7",
                        "Uuid",
                        "D0",
                        "Dp",
                        "U64",
                        "ChatId",
                        "Timestamp",
                        "Digest",
                        "Binary32",
                        "Binary64",
                    ] {
                        options = options
                            .with_format(format!("craxii_{kind}"), move |v| format_valid(kind, v));
                    }
                    for max in [
                        19usize, 20, 27, 32, 36, 43, 64, 86, 128, 256, 512, 65536, 21846, 10923,
                        2097152,
                    ] {
                        options = options
                            .with_format(format!("craxii_utf8_{max}"), move |v| v.len() <= max);
                    }
                    let validator = options.build(&json!({"$ref":uri})).map_err(|_| invalid())?;
                    result.insert(uri, validator);
                }
            }
            Ok(result)
        })
        .as_ref()
        .map_err(|e| *e)
}
pub fn validate_definition(family: &str, name: &str, value: &Value) -> ContractResult<()> {
    canonical_bytes(value)?;
    let uri = format!("urn:craxii:distribution:1.0.0:{family}#/definitions/{name}");
    if !validators()?.get(&uri).ok_or_else(invalid)?.is_valid(value) {
        return Err(invalid());
    }
    Ok(())
}
pub fn family_limits(kind: &str) -> Limits {
    let bytes = match kind {
        "error" | "protocol_rejection" => 4096,
        k if k.starts_with("enrollment_") => 65536,
        "model_invoke" => 1_048_576,
        "model_event" | "model_outcome" => 65536,
        "work_delivery" | "result_export" => 524288,
        _ => 16384,
    };
    Limits {
        bytes,
        nodes: if bytes > 65536 { 32768 } else { 4096 },
        depth: 32,
    }
}
fn text(v: &Value) -> ContractResult<&str> {
    v.as_str().ok_or_else(invalid)
}
pub fn check_semantics(value: &Value) -> ContractResult<()> {
    if !value.is_object() {
        return Err(invalid());
    }
    fn walk(v: &Value, key: &str, total: &mut usize) -> ContractResult<()> {
        match v {
            Value::String(s)
                if [
                    "text",
                    "delta",
                    "raw_arguments",
                    "result_json",
                    "data_json",
                    "details_json",
                    "opaque",
                    "schema_json",
                    "content_parts",
                    "instructions",
                ]
                .contains(&key) =>
            {
                if s.len() > 65536 {
                    return Err(invalid());
                }
                *total += s.len();
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, key, total)?;
                }
            }
            Value::Object(o) => {
                if let (Some(a), Some(b)) = (o.get("issued_at"), o.get("expires_at"))
                    && timestamp_micros(text(a)?)? >= timestamp_micros(text(b)?)?
                {
                    return Err(invalid());
                }
                for duration in ["max_elapsed_ms", "max_tool_duration_ms"] {
                    if let (Some(amount), Some(start), Some(end)) =
                        (o.get(duration), o.get("issued_at"), o.get("expires_at"))
                        && decimal(text(amount)?, "Dp")? * 1000
                            > timestamp_micros(text(end)?)? - timestamp_micros(text(start)?)?
                    {
                        return Err(invalid());
                    }
                }
                if let (Some(a), Some(b)) = (o.get("starts_at"), o.get("ends_at")) {
                    let start = timestamp_micros(text(a)?)?;
                    let end = timestamp_micros(text(b)?)?;
                    if start >= end
                        || (o.get("kind") == Some(&json!("trial")) && end - start != 604800000000)
                    {
                        return Err(invalid());
                    }
                }
                if let Some(ex) = o.get("extensions") {
                    if canonical_bytes(ex)?.len() > 1024 {
                        return Err(invalid());
                    }
                    for k in ex.as_object().ok_or_else(invalid)?.keys() {
                        let stem = k
                            .strip_prefix("x_")
                            .unwrap_or(k)
                            .split('_')
                            .next()
                            .unwrap_or("");
                        if [
                            "auth",
                            "credential",
                            "token",
                            "secret",
                            "key",
                            "session",
                            "routing",
                            "destination",
                            "policy",
                            "budget",
                            "limit",
                            "url",
                            "header",
                            "command",
                            "execute",
                            "provider",
                            "generation",
                            "epoch",
                            "customer",
                            "installation",
                            "logical",
                            "work",
                            "scope",
                            "entitlement",
                            "compute",
                            "model",
                            "price",
                            "grant",
                            "signature",
                            "audience",
                            "nonce",
                            "permission",
                            "revocation",
                            "boot",
                        ]
                        .contains(&stem)
                        {
                            return Err(invalid());
                        }
                    }
                }
                if let Some(content) = o.get("content") {
                    let mut size = 0;
                    for b in content["blocks"].as_array().ok_or_else(invalid)? {
                        size += text(&b["text"])?.len();
                    }
                    if size > 65536 {
                        return Err(invalid());
                    }
                }
                if let Some(n) = o.get("item_ordinal")
                    && decimal(text(n)?, "D0")? > 63
                {
                    return Err(invalid());
                }
                if let (Some(pk), Some(id)) = (o.get("public_key"), o.get("key_id"))
                    && key_id(text(pk)?)? != text(id)?
                {
                    return Err(invalid());
                }
                if let (Some(opaque), Some(hash)) = (o.get("opaque"), o.get("opaque_sha256"))
                    && sha256(text(opaque)?.as_bytes()) != text(hash)?
                {
                    return Err(invalid());
                }
                if (o.contains_key("retry_of_attempt_id")
                    || o.contains_key("retry_authorization_id"))
                    && (!o.contains_key("retry_of_attempt_id")
                        || !o.contains_key("retry_authorization_id")
                        || o.get("retry_of_attempt_id") == o.get("attempt_id"))
                {
                    return Err(invalid());
                }
                for (k, x) in o {
                    walk(x, k, total)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut total = 0;
    walk(value, "", &mut total)?;
    if total > 524288 {
        return Err(invalid());
    }
    let body = value.get("body").unwrap_or(value);
    for (field, hash, domain) in [
        ("material", "material_digest", DigestDomain::Work),
        ("control", "control_digest", DigestDomain::Cancel),
        ("snapshot", "export_digest", DigestDomain::Result),
        (
            "invocation",
            "invocation_digest",
            DigestDomain::ModelInvocation,
        ),
    ] {
        if let Some(m) = body.get(field)
            && digest_material(domain, m)? != text(&body[hash])?
        {
            return Err(invalid());
        }
    }
    if let Some(s) = body.get("snapshot")
        && sha256(&canonical_bytes(&s["content"])?) != text(&s["content_sha256"])?
    {
        return Err(invalid());
    }
    if let Some(a) = value.get("authority") {
        for field in [
            "material",
            "control",
            "snapshot",
            "invocation",
            "grant",
            "profile",
        ] {
            if let Some(m) = body.get(field) {
                for k in ["customer_id", "logical_agent_id"] {
                    if m[k] != a[k] {
                        return Err(ContractError("ownership_denied"));
                    }
                }
                if m.get("environment")
                    .is_some_and(|e| e != &value["environment"])
                {
                    return Err(ContractError("ownership_denied"));
                }
                if ["profile", "grant"].contains(&field)
                    && (m["installation_id"] != a["installation_id"]
                        || m["generation"] != a["generation"])
                {
                    return Err(ContractError("ownership_denied"));
                }
            }
        }
    }
    if body.get("instance_identity_document_b64").is_some() {
        for (field, max) in [
            ("instance_identity_document", 16384),
            ("instance_identity_signature", 8192),
        ] {
            let b = binary(text(&body[format!("{field}_b64")])?, None)?;
            if b.len() > max || sha256(&b) != text(&body["signed"][format!("{field}_sha256")])? {
                return Err(invalid());
            }
        }
    }
    if value["type"] == "cancel_receipt" {
        if body["outcome"] == "cancelled"
            && (body["work_state"] != "cancelled" || body["cleanup_pending"] != false)
        {
            return Err(invalid());
        }
        if body["outcome"] == "already_terminal"
            && !matches!(
                text(&body["work_state"])?,
                "completed" | "failed" | "cancelled" | "interrupted"
            )
        {
            return Err(invalid());
        }
    }
    if value["type"] == "drain_ack"
        && body["quiescent"] == true
        && (body["claims_closed"] != true
            || body["uncommitted_state"] != false
            || [
                "active_work_count",
                "queued_work_count",
                "pending_receipt_ack_count",
                "pending_export_count",
                "descendant_count",
            ]
            .iter()
            .any(|k| body[k] != "0"))
    {
        return Err(invalid());
    }
    if value["type"] == "heartbeat"
        && matches!(
            body["readiness"].as_str(),
            Some("storage_ready" | "acceptance_ready" | "operational_ready")
        )
        && (body["storage"]["recovery"] != "complete"
            || body["storage"]["binds_verified"] != true
            || (body["readiness"] == "operational_ready" && body.get("model_config_id").is_none()))
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn decode_message(bytes: &[u8]) -> ContractResult<Message> {
    let value = parse_bounded(bytes, LARGE_LIMITS)?;
    let kind = text(&value["type"])?;
    let limits = family_limits(kind);
    parse_bounded(bytes, limits)?;
    canonical_with_limits(&value, limits)?;
    let name = kind
        .split('_')
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<String>();
    let uri = validators()?
        .keys()
        .find(|k| k.ends_with(&format!("/{}", name)))
        .ok_or_else(invalid)?;
    if !validators()?[uri].is_valid(&value) {
        return Err(invalid());
    }
    check_semantics(&value)?;
    serde_json::from_value(value).map_err(|_| invalid())
}
pub fn encode_message(message: &Message) -> ContractResult<Vec<u8>> {
    let value = serde_json::to_value(message).map_err(|_| invalid())?;
    let bytes = canonical_bytes(&value)?;
    decode_message(&bytes)?;
    Ok(bytes)
}
