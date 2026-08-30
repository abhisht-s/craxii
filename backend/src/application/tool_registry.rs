//! Immutable provider-neutral V0 tool definitions and canonical argument validation.

use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::domain::{
    LogicalPathKind, LogicalPathReference, PrivilegeMode, SchemaVersion, Sha256Digest, ToolName,
    ToolVersion,
};
use crate::ports::workstation::{
    HARD_EXECUTION_COMMAND_MAX_BYTES, HARD_EXECUTION_STREAM_CAPTURE_BYTES,
    HARD_EXECUTION_TIMEOUT_MS, HARD_FILE_READ_MAX_BYTES,
};
use crate::ports::workstation_preparation::RequiredWorkstationCapability;

/// Exact maximum encoded model argument bytes accepted at the Stage 14 boundary.
///
/// This envelope is intentionally larger than the 64-KiB command field because JSON escaping can
/// expand an otherwise schema-valid ASCII command by as much as six bytes per character.
pub const MAX_RAW_TOOL_ARGUMENT_BYTES: usize = 524_288;

/// Bounded read-file model projection; complete larger content is artifact-backed.
pub const READ_FILE_PROJECTION_BYTES: usize = 32_768;

/// Fixed V0 implementation version for both built-ins.
pub const V0_TOOL_IMPLEMENTATION_VERSION: &str = "1.0.0";

/// Fixed V0 schema version for both built-ins.
pub const V0_TOOL_SCHEMA_VERSION: i64 = 1;

/// Exact schema/decoder semantics derived from validated runtime configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolSemanticPolicy {
    pub read_file_default_bytes: u64,
    pub read_file_max_bytes: u64,
    pub run_shell_command_max_bytes: u64,
    pub run_shell_default_timeout_ms: u64,
    pub run_shell_max_timeout_ms: u64,
}

impl ToolSemanticPolicy {
    fn validate(self) -> Result<Self, ToolRegistryError> {
        if self.read_file_default_bytes == 0
            || self.read_file_default_bytes > self.read_file_max_bytes
            || self.read_file_max_bytes > HARD_FILE_READ_MAX_BYTES
            || self.run_shell_command_max_bytes == 0
            || self.run_shell_command_max_bytes > HARD_EXECUTION_COMMAND_MAX_BYTES as u64
            || self.run_shell_default_timeout_ms == 0
            || self.run_shell_default_timeout_ms > self.run_shell_max_timeout_ms
            || self.run_shell_max_timeout_ms > HARD_EXECUTION_TIMEOUT_MS
            || !self.run_shell_default_timeout_ms.is_multiple_of(1_000)
            || !self.run_shell_max_timeout_ms.is_multiple_of(1_000)
        {
            return Err(ToolRegistryError::InvalidSemanticPolicy);
        }
        Ok(self)
    }
}

const WORKSPACE_PATH_PATTERN: &str =
    r"^(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*$";
const ABSOLUTE_PATH_PATTERN: &str =
    r"^/(?:$|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)$";
const CWD_PATTERN: &str = r"^(?:/(?:$|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)$";
const ASCII_NON_NUL_PATTERN: &str = r"^[\u0001-\u007f]+$";

/// Internal handler identity excluded from semantic definition serialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandlerIdentity {
    ReadFile,
    RunShell,
}

/// Stable side-effect classification exposed by a trusted definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolSideEffect {
    None,
    Possible,
}

impl ToolSideEffect {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Possible => "possible",
        }
    }
}

/// One immutable trusted definition. Runtime identities never enter this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDefinition {
    name: ToolName,
    implementation_version: ToolVersion,
    schema_version: SchemaVersion,
    description: &'static str,
    input_schema: Value,
    required_capability: RequiredWorkstationCapability,
    side_effect: ToolSideEffect,
    privilege_modes: &'static [PrivilegeMode],
    default_timeout_ms: Option<u64>,
    hard_timeout_ms: Option<u64>,
    output_policy: Value,
    handler: HandlerIdentity,
    semantic_policy: ToolSemanticPolicy,
}

impl ToolDefinition {
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    pub const fn implementation_version(&self) -> &ToolVersion {
        &self.implementation_version
    }

    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub const fn description(&self) -> &'static str {
        self.description
    }

    pub const fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    pub const fn required_capability(&self) -> RequiredWorkstationCapability {
        self.required_capability
    }

    pub const fn side_effect(&self) -> ToolSideEffect {
        self.side_effect
    }

    pub const fn privilege_modes(&self) -> &'static [PrivilegeMode] {
        self.privilege_modes
    }

    pub const fn default_timeout_ms(&self) -> Option<u64> {
        self.default_timeout_ms
    }

    pub const fn hard_timeout_ms(&self) -> Option<u64> {
        self.hard_timeout_ms
    }

    pub const fn output_policy(&self) -> &Value {
        &self.output_policy
    }

    pub(crate) const fn handler(&self) -> HandlerIdentity {
        self.handler
    }

    fn semantic_value(&self) -> Value {
        canonicalize_json(json!({
            "default_timeout_ms": self.default_timeout_ms,
            "description": self.description,
            "hard_timeout_ms": self.hard_timeout_ms,
            "implementation_version": self.implementation_version.as_str(),
            "input_schema": self.input_schema,
            "name": self.name.as_str(),
            "output_policy": self.output_policy,
            "privilege_modes": self.privilege_modes.iter().map(|mode| match mode {
                PrivilegeMode::User => "user",
                PrivilegeMode::Administrative => "administrative",
            }).collect::<Vec<_>>(),
            "required_capability": match self.required_capability {
                RequiredWorkstationCapability::FilesystemRead => "filesystem_read",
                RequiredWorkstationCapability::ForegroundExecute => "foreground_execute",
            },
            "schema_version": self.schema_version.get(),
            "side_effect": self.side_effect.as_str(),
        }))
    }
}

/// Startup-built ordered registry with no mutation surface.
#[derive(Debug)]
pub struct ToolRegistry {
    definitions: Box<[ToolDefinition]>,
    fingerprint: Sha256Digest,
}

impl ToolRegistry {
    /// Constructs exactly the two production V0 built-ins in frozen order.
    pub fn v0(policy: ToolSemanticPolicy) -> Result<Self, ToolRegistryError> {
        let policy = policy.validate()?;
        Self::try_new(vec![
            read_file_definition(policy),
            run_shell_definition(policy),
        ])
    }

    fn try_new(definitions: Vec<ToolDefinition>) -> Result<Self, ToolRegistryError> {
        if definitions.is_empty() {
            return Err(ToolRegistryError::Empty);
        }
        let mut names = BTreeSet::new();
        for definition in &definitions {
            if !names.insert(definition.name.as_str()) {
                return Err(ToolRegistryError::DuplicateName);
            }
        }
        let semantic = Value::Array(
            definitions
                .iter()
                .map(ToolDefinition::semantic_value)
                .collect(),
        );
        let bytes = serde_json::to_vec(&semantic).expect("semantic tool definitions serialize");
        Ok(Self {
            definitions: definitions.into_boxed_slice(),
            fingerprint: Sha256Digest::hash_bytes(&bytes),
        })
    }

    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub fn lookup(&self, name: &ToolName) -> Option<&ToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name() == name)
    }

    pub const fn fingerprint(&self) -> Sha256Digest {
        self.fingerprint
    }

    pub const fn semantic_policy(&self) -> ToolSemanticPolicy {
        self.definitions[0].semantic_policy
    }
}

/// Construction failures retain no rejected definition data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolRegistryError {
    Empty,
    DuplicateName,
    InvalidSemanticPolicy,
}

/// Exact typed input for `read_file` after validation/default injection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadFileInput {
    path: LogicalPathReference,
    max_bytes: u64,
}

impl ReadFileInput {
    pub const fn path(&self) -> &LogicalPathReference {
        &self.path
    }

    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

/// Exact typed input for `run_shell` after validation/default injection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunShellInput {
    command: String,
    requested_cwd: Option<LogicalPathReference>,
    requested_privilege: PrivilegeMode,
    timeout_ms: u64,
}

impl RunShellInput {
    pub fn command(&self) -> &str {
        &self.command
    }

    pub const fn requested_cwd(&self) -> Option<&LogicalPathReference> {
        self.requested_cwd.as_ref()
    }

    pub const fn requested_privilege(&self) -> PrivilegeMode {
        self.requested_privilege
    }

    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
}

/// Closed V0 typed input inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidatedToolInput {
    ReadFile(ReadFileInput),
    RunShell(RunShellInput),
}

impl ValidatedToolInput {
    pub const fn requested_privilege(&self) -> PrivilegeMode {
        match self {
            Self::ReadFile(_) => PrivilegeMode::User,
            Self::RunShell(input) => input.requested_privilege(),
        }
    }

    pub const fn requested_timeout_ms(&self) -> Option<u64> {
        match self {
            Self::ReadFile(_) => None,
            Self::RunShell(input) => Some(input.timeout_ms()),
        }
    }
}

/// Validated typed input plus the exact normalized durable argument identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedToolArguments {
    input: ValidatedToolInput,
    canonical_json: String,
    sha256: Sha256Digest,
}

impl ValidatedToolArguments {
    pub const fn input(&self) -> &ValidatedToolInput {
        &self.input
    }

    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }

    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }
}

/// Stable safe validation reasons; rejected argument content is never retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolArgumentErrorKind {
    Oversized,
    DuplicateKey,
    MalformedJson,
    NonObject,
    SchemaMismatch,
    SemanticInvalid,
}

/// Redacted argument boundary error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolArgumentError {
    kind: ToolArgumentErrorKind,
}

impl ToolArgumentError {
    const fn new(kind: ToolArgumentErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> ToolArgumentErrorKind {
        self.kind
    }
}

impl Display for ToolArgumentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid tool arguments")
    }
}

impl std::error::Error for ToolArgumentError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReadFileInput {
    path: String,
    #[serde(default)]
    path_kind: RawPathKind,
    max_bytes: Option<u64>,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawPathKind {
    #[default]
    WorkspaceRelative,
    Absolute,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRunShellInput {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    privilege: RawPrivilege,
    timeout_seconds: Option<u64>,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawPrivilege {
    #[default]
    User,
    Administrative,
}

/// Parses one known definition with duplicate rejection and normalized defaults.
pub fn validate_arguments(
    definition: &ToolDefinition,
    raw: &[u8],
) -> Result<ValidatedToolArguments, ToolArgumentError> {
    if raw.len() > MAX_RAW_TOOL_ARGUMENT_BYTES {
        return Err(ToolArgumentError::new(ToolArgumentErrorKind::Oversized));
    }
    let parsed = serde_json::from_slice::<NoDuplicateValue>(raw)
        .map_err(classify_json_error)?
        .0;
    if !parsed.is_object() {
        return Err(ToolArgumentError::new(ToolArgumentErrorKind::NonObject));
    }
    match definition.handler() {
        HandlerIdentity::ReadFile => validate_read_file(definition, parsed),
        HandlerIdentity::RunShell => validate_run_shell(definition, parsed),
    }
}

fn classify_json_error(error: serde_json::Error) -> ToolArgumentError {
    let kind = if error.to_string().contains("duplicate object key") {
        ToolArgumentErrorKind::DuplicateKey
    } else {
        ToolArgumentErrorKind::MalformedJson
    };
    ToolArgumentError::new(kind)
}

fn validate_read_file(
    definition: &ToolDefinition,
    value: Value,
) -> Result<ValidatedToolArguments, ToolArgumentError> {
    let raw: RawReadFileInput = serde_json::from_value(value)
        .map_err(|_| ToolArgumentError::new(ToolArgumentErrorKind::SchemaMismatch))?;
    let max_bytes = raw
        .max_bytes
        .unwrap_or(definition.semantic_policy.read_file_default_bytes);
    if max_bytes == 0
        || max_bytes > definition.semantic_policy.read_file_max_bytes
        || !schema_path_valid(&raw.path, raw.path_kind)
    {
        return Err(ToolArgumentError::new(
            ToolArgumentErrorKind::SemanticInvalid,
        ));
    }
    let path = match raw.path_kind {
        RawPathKind::WorkspaceRelative => LogicalPathReference::workspace_relative(raw.path),
        RawPathKind::Absolute => LogicalPathReference::absolute(raw.path),
    }
    .map_err(|_| ToolArgumentError::new(ToolArgumentErrorKind::SemanticInvalid))?;
    let path_kind = match path.kind() {
        LogicalPathKind::WorkspaceRelative => "workspace_relative",
        LogicalPathKind::Absolute => "absolute",
    };
    normalized(
        ValidatedToolInput::ReadFile(ReadFileInput {
            path: path.clone(),
            max_bytes,
        }),
        json!({
            "max_bytes": max_bytes,
            "path": path.canonical(),
            "path_kind": path_kind,
        }),
    )
}

fn validate_run_shell(
    definition: &ToolDefinition,
    value: Value,
) -> Result<ValidatedToolArguments, ToolArgumentError> {
    let raw: RawRunShellInput = serde_json::from_value(value)
        .map_err(|_| ToolArgumentError::new(ToolArgumentErrorKind::SchemaMismatch))?;
    let timeout_seconds = raw
        .timeout_seconds
        .unwrap_or(definition.semantic_policy.run_shell_default_timeout_ms / 1_000);
    if raw.command.is_empty()
        || raw.command.len() > definition.semantic_policy.run_shell_command_max_bytes as usize
        || !raw.command.is_ascii()
        || raw.command.contains('\0')
        || raw.cwd.as_deref().is_some_and(|cwd| {
            !schema_path_valid(
                cwd,
                if cwd.starts_with('/') {
                    RawPathKind::Absolute
                } else {
                    RawPathKind::WorkspaceRelative
                },
            )
        })
        || timeout_seconds == 0
        || timeout_seconds > definition.semantic_policy.run_shell_max_timeout_ms / 1_000
    {
        return Err(ToolArgumentError::new(
            ToolArgumentErrorKind::SemanticInvalid,
        ));
    }
    let requested_cwd = raw
        .cwd
        .map(|cwd| {
            if cwd.starts_with('/') {
                LogicalPathReference::absolute(cwd)
            } else {
                LogicalPathReference::workspace_relative(cwd)
            }
        })
        .transpose()
        .map_err(|_| ToolArgumentError::new(ToolArgumentErrorKind::SemanticInvalid))?;
    let privilege = match raw.privilege {
        RawPrivilege::User => PrivilegeMode::User,
        RawPrivilege::Administrative => PrivilegeMode::Administrative,
    };
    let timeout_ms = timeout_seconds
        .checked_mul(1_000)
        .ok_or_else(|| ToolArgumentError::new(ToolArgumentErrorKind::SemanticInvalid))?;
    normalized(
        ValidatedToolInput::RunShell(RunShellInput {
            command: raw.command.clone(),
            requested_cwd: requested_cwd.clone(),
            requested_privilege: privilege,
            timeout_ms,
        }),
        json!({
            "command": raw.command,
            "cwd": requested_cwd.as_ref().map(LogicalPathReference::canonical),
            "privilege": match privilege {
                PrivilegeMode::User => "user",
                PrivilegeMode::Administrative => "administrative",
            },
            "timeout_seconds": timeout_seconds,
        }),
    )
}

fn schema_path_valid(value: &str, kind: RawPathKind) -> bool {
    if value.is_empty() || value.len() > crate::domain::MAX_LOGICAL_PATH_BYTES || !value.is_ascii()
    {
        return false;
    }
    let (absolute, body) = if let Some(body) = value.strip_prefix('/') {
        (true, body)
    } else {
        (false, value)
    };
    if absolute != matches!(kind, RawPathKind::Absolute) {
        return false;
    }
    if absolute && body.is_empty() {
        return true;
    }
    body.split('/').all(|segment| {
        !segment.is_empty()
            && !matches!(segment, "." | "..")
            && segment
                .bytes()
                .all(|byte| (0x20..=0x7e).contains(&byte) && !matches!(byte, b'/' | b'\\'))
    })
}

fn normalized(
    input: ValidatedToolInput,
    value: Value,
) -> Result<ValidatedToolArguments, ToolArgumentError> {
    let canonical_json = serde_json::to_string(&canonicalize_json(value))
        .map_err(|_| ToolArgumentError::new(ToolArgumentErrorKind::SemanticInvalid))?;
    let sha256 = Sha256Digest::hash_bytes(canonical_json.as_bytes());
    Ok(ValidatedToolArguments {
        input,
        canonical_json,
        sha256,
    })
}

fn read_file_definition(policy: ToolSemanticPolicy) -> ToolDefinition {
    ToolDefinition {
        name: ToolName::try_new("read_file").expect("static tool name"),
        implementation_version: ToolVersion::try_new(V0_TOOL_IMPLEMENTATION_VERSION)
            .expect("static tool version"),
        schema_version: SchemaVersion::try_new(V0_TOOL_SCHEMA_VERSION)
            .expect("static schema version"),
        description: "Read one bounded UTF-8 file through the configured workstation.",
        input_schema: canonicalize_json(json!({
            "oneOf": [
                {
                    "additionalProperties": false,
                    "properties": {
                        "max_bytes": {"default": policy.read_file_default_bytes, "maximum": policy.read_file_max_bytes, "minimum": 1, "type": "integer"},
                        "path": {"maxLength": 4096, "minLength": 1, "pattern": WORKSPACE_PATH_PATTERN, "type": "string"},
                        "path_kind": {"default": "workspace_relative", "enum": ["workspace_relative"], "type": "string"}
                    },
                    "required": ["path"],
                    "type": "object"
                },
                {
                    "additionalProperties": false,
                    "properties": {
                        "max_bytes": {"default": policy.read_file_default_bytes, "maximum": policy.read_file_max_bytes, "minimum": 1, "type": "integer"},
                        "path": {"maxLength": 4096, "minLength": 1, "pattern": ABSOLUTE_PATH_PATTERN, "type": "string"},
                        "path_kind": {"enum": ["absolute"], "type": "string"}
                    },
                    "required": ["path", "path_kind"],
                    "type": "object"
                }
            ],
            "type": "object"
        })),
        required_capability: RequiredWorkstationCapability::FilesystemRead,
        side_effect: ToolSideEffect::None,
        privilege_modes: &[PrivilegeMode::User],
        default_timeout_ms: None,
        hard_timeout_ms: None,
        output_policy: canonicalize_json(json!({
            "inline_projection_bytes": READ_FILE_PROJECTION_BYTES,
            "overflow": "canonical_evidence_artifact"
        })),
        handler: HandlerIdentity::ReadFile,
        semantic_policy: policy,
    }
}

fn run_shell_definition(policy: ToolSemanticPolicy) -> ToolDefinition {
    ToolDefinition {
        name: ToolName::try_new("run_shell").expect("static tool name"),
        implementation_version: ToolVersion::try_new(V0_TOOL_IMPLEMENTATION_VERSION)
            .expect("static tool version"),
        schema_version: SchemaVersion::try_new(V0_TOOL_SCHEMA_VERSION)
            .expect("static schema version"),
        description: "Run one bounded foreground Bash command through the configured workstation.",
        input_schema: canonicalize_json(json!({
            "additionalProperties": false,
            "properties": {
                "command": {"maxLength": policy.run_shell_command_max_bytes, "minLength": 1, "pattern": ASCII_NON_NUL_PATTERN, "type": "string"},
                "cwd": {"maxLength": 4096, "minLength": 1, "pattern": CWD_PATTERN, "type": ["string", "null"]},
                "privilege": {"default": "user", "enum": ["user", "administrative"], "type": "string"},
                "timeout_seconds": {"default": policy.run_shell_default_timeout_ms / 1_000, "maximum": policy.run_shell_max_timeout_ms / 1_000, "minimum": 1, "type": "integer"}
            },
            "required": ["command"],
            "type": "object"
        })),
        required_capability: RequiredWorkstationCapability::ForegroundExecute,
        side_effect: ToolSideEffect::Possible,
        privilege_modes: &[PrivilegeMode::User, PrivilegeMode::Administrative],
        default_timeout_ms: Some(policy.run_shell_default_timeout_ms),
        hard_timeout_ms: Some(policy.run_shell_max_timeout_ms),
        output_policy: canonicalize_json(json!({
            "stderr_capture_bytes": HARD_EXECUTION_STREAM_CAPTURE_BYTES,
            "stderr_projection_bytes": crate::ports::workstation::EXECUTION_STREAM_PROJECTION_BYTES,
            "stdout_capture_bytes": HARD_EXECUTION_STREAM_CAPTURE_BYTES,
            "stdout_projection_bytes": crate::ports::workstation::EXECUTION_STREAM_PROJECTION_BYTES
        })),
        handler: HandlerIdentity::RunShell,
        semantic_policy: policy,
    }
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_json(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

struct NoDuplicateValue(Value);

impl<'de> Deserialize<'de> for NoDuplicateValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = NoDuplicateValue;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::Number(value.into())))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::Number(value.into())))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .map(NoDuplicateValue)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::String(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(NoDuplicateValue(Value::Null))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<NoDuplicateValue>()? {
                    values.push(value.0);
                }
                Ok(NoDuplicateValue(Value::Array(values)))
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some(key) = object.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate object key"));
                    }
                    let value = object.next_value::<NoDuplicateValue>()?;
                    values.insert(key, value.0);
                }
                Ok(NoDuplicateValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ToolSemanticPolicy {
        ToolSemanticPolicy {
            read_file_default_bytes: 1_048_576,
            read_file_max_bytes: 8_388_608,
            run_shell_command_max_bytes: 65_536,
            run_shell_default_timeout_ms: 120_000,
            run_shell_max_timeout_ms: 900_000,
        }
    }

    fn registry() -> ToolRegistry {
        ToolRegistry::v0(policy()).unwrap()
    }

    #[test]
    fn v0_registry_inventory_order_versions_and_lookup_are_exact() {
        let registry = registry();
        let definitions = registry.definitions();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].name().as_str(), "read_file");
        assert_eq!(definitions[1].name().as_str(), "run_shell");
        for definition in definitions {
            assert_eq!(definition.implementation_version().as_str(), "1.0.0");
            assert_eq!(definition.schema_version().get(), 1);
            assert!(registry.lookup(definition.name()).is_some());
        }
        assert!(
            registry
                .lookup(&ToolName::try_new("unknown_tool").unwrap())
                .is_none()
        );
    }

    #[test]
    fn duplicate_registry_is_rejected_and_fingerprint_is_deterministic_and_sensitive() {
        let duplicate = vec![
            read_file_definition(policy()),
            read_file_definition(policy()),
        ];
        assert_eq!(
            ToolRegistry::try_new(duplicate).unwrap_err(),
            ToolRegistryError::DuplicateName
        );
        assert_eq!(registry().fingerprint(), registry().fingerprint());
        let baseline = registry().fingerprint();
        let mut changed = read_file_definition(policy());
        changed.description = "Semantically changed description.";
        let changed = ToolRegistry::try_new(vec![changed, run_shell_definition(policy())]).unwrap();
        assert_ne!(baseline, changed.fingerprint());

        let mut changed_policy = policy();
        changed_policy.read_file_default_bytes = 65_536;
        assert_ne!(
            baseline,
            ToolRegistry::v0(changed_policy).unwrap().fingerprint()
        );
    }

    #[test]
    fn schemas_are_canonical_and_cover_decoder_defaults_and_bounds() {
        let registry = registry();
        for definition in registry.definitions() {
            let serialized = serde_json::to_string(definition.input_schema()).unwrap();
            assert_eq!(
                serde_json::to_string(&canonicalize_json(
                    serde_json::from_str(&serialized).unwrap()
                ))
                .unwrap(),
                serialized
            );
            if definition.name().as_str() == "read_file" {
                assert_eq!(
                    definition.input_schema()["oneOf"][0]["additionalProperties"],
                    false
                );
                assert_eq!(
                    definition.input_schema()["oneOf"][1]["additionalProperties"],
                    false
                );
            } else {
                assert_eq!(definition.input_schema()["additionalProperties"], false);
            }
        }
        let read = &registry.definitions()[0];
        let parsed = validate_arguments(read, br#"{"path":"src/lib.rs"}"#).unwrap();
        assert_eq!(
            parsed.canonical_json(),
            r#"{"max_bytes":1048576,"path":"src/lib.rs","path_kind":"workspace_relative"}"#
        );
        let shell = &registry.definitions()[1];
        let parsed = validate_arguments(shell, br#"{"command":"printf ok"}"#).unwrap();
        assert_eq!(
            parsed.canonical_json(),
            r#"{"command":"printf ok","cwd":null,"privilege":"user","timeout_seconds":120}"#
        );
    }

    #[test]
    fn validation_rejects_all_structural_and_semantic_boundaries() {
        let registry = registry();
        let read = &registry.definitions()[0];
        let shell = &registry.definitions()[1];
        for raw in [
            &b"{"[..],
            &b"[]"[..],
            &b"{}"[..],
            &br#"{"path":1}"#[..],
            &br#"{"path":"x","extra":true}"#[..],
            &br#"{"path":"x","path":"y"}"#[..],
            &br#"{"path":"x","path_kind":"other"}"#[..],
            &br#"{"path":"x","max_bytes":0}"#[..],
        ] {
            assert!(validate_arguments(read, raw).is_err(), "accepted {raw:?}");
        }
        assert_eq!(
            validate_arguments(read, &vec![b' '; MAX_RAW_TOOL_ARGUMENT_BYTES + 1])
                .unwrap_err()
                .kind(),
            ToolArgumentErrorKind::Oversized
        );
        for raw in [
            br#"{"command":"","timeout_seconds":1}"#.as_slice(),
            br#"{"command":"x","timeout_seconds":0}"#.as_slice(),
            br#"{"command":"x","timeout_seconds":901}"#.as_slice(),
            br#"{"command":"x","privilege":"root"}"#.as_slice(),
            br#"{"command":"x","environment":{"SECRET":"x"}}"#.as_slice(),
        ] {
            assert!(validate_arguments(shell, raw).is_err(), "accepted {raw:?}");
        }
        let oversized_command = serde_json::to_vec(&json!({
            "command": "x".repeat(HARD_EXECUTION_COMMAND_MAX_BYTES + 1)
        }))
        .unwrap();
        assert!(validate_arguments(shell, &oversized_command).is_err());
    }

    #[test]
    fn duplicate_keys_are_rejected_recursively_before_typed_decode() {
        let registry = registry();
        let read = &registry.definitions()[0];
        for raw in [
            br#"{"path":"x","path":"y"}"#.as_slice(),
            br#"{"path":"x","nested":{"a":1,"a":2}}"#.as_slice(),
        ] {
            assert_eq!(
                validate_arguments(read, raw).unwrap_err().kind(),
                ToolArgumentErrorKind::DuplicateKey
            );
        }
    }

    fn emitted_schema_accepts(schema: &Value, instance: &Value) -> Result<bool, String> {
        let object = schema
            .as_object()
            .ok_or_else(|| "schema node is not an object".to_owned())?;
        const SUPPORTED: &[&str] = &[
            "additionalProperties",
            "default",
            "enum",
            "maximum",
            "maxLength",
            "minimum",
            "minLength",
            "oneOf",
            "pattern",
            "properties",
            "required",
            "type",
        ];
        if let Some(keyword) = object
            .keys()
            .find(|keyword| !SUPPORTED.contains(&keyword.as_str()))
        {
            return Err(format!("unsupported emitted schema keyword: {keyword}"));
        }

        if let Some(types) = object.get("type") {
            let type_matches = match types {
                Value::String(expected) => emitted_type_matches(expected, instance)?,
                Value::Array(expected) => {
                    let mut matched = false;
                    for expected in expected {
                        let expected = expected
                            .as_str()
                            .ok_or_else(|| "schema type array contains a non-string".to_owned())?;
                        matched |= emitted_type_matches(expected, instance)?;
                    }
                    matched
                }
                _ => return Err("schema type must be a string or string array".to_owned()),
            };
            if !type_matches {
                return Ok(false);
            }
        }
        if let Some(variants) = object.get("oneOf") {
            let variants = variants
                .as_array()
                .ok_or_else(|| "schema oneOf must be an array".to_owned())?;
            let mut matches = 0;
            for variant in variants {
                matches += usize::from(emitted_schema_accepts(variant, instance)?);
            }
            if matches != 1 {
                return Ok(false);
            }
        }
        if let Some(values) = object.get("enum") {
            let values = values
                .as_array()
                .ok_or_else(|| "schema enum must be an array".to_owned())?;
            if !values.contains(instance) {
                return Ok(false);
            }
        }
        if let Some(minimum) = object.get("minimum") {
            let Some(instance) = emitted_integer(instance) else {
                return Ok(false);
            };
            if instance
                < emitted_integer(minimum).ok_or_else(|| "minimum is not an integer".to_owned())?
            {
                return Ok(false);
            }
        }
        if let Some(maximum) = object.get("maximum") {
            let Some(instance) = emitted_integer(instance) else {
                return Ok(false);
            };
            if instance
                > emitted_integer(maximum).ok_or_else(|| "maximum is not an integer".to_owned())?
            {
                return Ok(false);
            }
        }
        if let Some(minimum) = object.get("minLength") {
            let Some(instance) = instance.as_str() else {
                return Ok(false);
            };
            let minimum = minimum
                .as_u64()
                .ok_or_else(|| "minLength is not an unsigned integer".to_owned())?;
            if (instance.chars().count() as u64) < minimum {
                return Ok(false);
            }
        }
        if let Some(maximum) = object.get("maxLength") {
            let Some(instance) = instance.as_str() else {
                return Ok(false);
            };
            let maximum = maximum
                .as_u64()
                .ok_or_else(|| "maxLength is not an unsigned integer".to_owned())?;
            if instance.chars().count() as u64 > maximum {
                return Ok(false);
            }
        }
        if let Some(pattern) = object.get("pattern") {
            let Some(instance) = instance.as_str() else {
                return Ok(false);
            };
            let pattern = pattern
                .as_str()
                .ok_or_else(|| "pattern is not a string".to_owned())?;
            if !emitted_pattern_matches(pattern, instance)? {
                return Ok(false);
            }
        }
        if let Some(instance) = instance.as_object() {
            let properties = match object.get("properties") {
                Some(properties) => Some(
                    properties
                        .as_object()
                        .ok_or_else(|| "properties is not an object".to_owned())?,
                ),
                None => None,
            };
            if let Some(required) = object.get("required") {
                let required = required
                    .as_array()
                    .ok_or_else(|| "required is not an array".to_owned())?;
                for key in required {
                    let key = key
                        .as_str()
                        .ok_or_else(|| "required contains a non-string".to_owned())?;
                    if !instance.contains_key(key) {
                        return Ok(false);
                    }
                }
            }
            if object.get("additionalProperties") == Some(&Value::Bool(false))
                && instance
                    .keys()
                    .any(|key| properties.is_none_or(|properties| !properties.contains_key(key)))
            {
                return Ok(false);
            }
            if let Some(properties) = properties {
                for (key, property_schema) in properties {
                    if let Some(value) = instance.get(key)
                        && !emitted_schema_accepts(property_schema, value)?
                    {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn emitted_type_matches(expected: &str, instance: &Value) -> Result<bool, String> {
        match expected {
            "integer" => Ok(emitted_integer(instance).is_some()),
            "null" => Ok(instance.is_null()),
            "object" => Ok(instance.is_object()),
            "string" => Ok(instance.is_string()),
            other => Err(format!("unsupported emitted schema type: {other}")),
        }
    }

    fn emitted_integer(value: &Value) -> Option<i128> {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    }

    fn emitted_pattern_matches(pattern: &str, value: &str) -> Result<bool, String> {
        match pattern {
            r"^[\u0001-\u007f]+$" => Ok(!value.is_empty()
                && value
                    .chars()
                    .all(|character| (1..=0x7f).contains(&(character as u32)))),
            r"^(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*$" => {
                Ok(emitted_relative_path_matches(value))
            }
            r"^/(?:$|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)$" => {
                Ok(emitted_absolute_path_matches(value))
            }
            r"^(?:/(?:$|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)|(?!\.{1,2}(?:/|$))(?!.*\/\.{1,2}(?:/|$))[ !-\.0-\[\]-~]+(?:/[ !-\.0-\[\]-~]+)*)$" => {
                Ok(emitted_absolute_path_matches(value) || emitted_relative_path_matches(value))
            }
            other => Err(format!("unsupported emitted schema pattern: {other}")),
        }
    }

    fn emitted_absolute_path_matches(value: &str) -> bool {
        value == "/"
            || value
                .strip_prefix('/')
                .is_some_and(emitted_relative_path_matches)
    }

    fn emitted_relative_path_matches(value: &str) -> bool {
        !value.is_empty()
            && value.split('/').all(|segment| {
                !matches!(segment, "" | "." | "..")
                    && segment
                        .bytes()
                        .all(|byte| (b' '..=b'~').contains(&byte) && !matches!(byte, b'/' | b'\\'))
            })
    }

    fn assert_schema_decoder_case(
        label: &str,
        definition: &ToolDefinition,
        value: Value,
        expected: bool,
    ) {
        let schema = emitted_schema_accepts(definition.input_schema(), &value)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        let decoder = validate_arguments(definition, &serde_json::to_vec(&value).unwrap()).is_ok();
        assert_eq!(schema, expected, "{label}: emitted schema result differs");
        assert_eq!(decoder, expected, "{label}: typed decoder result differs");
        assert_eq!(schema, decoder, "{label}: schema/decoder divergence");
    }

    #[test]
    fn emitted_schema_evaluator_matches_typed_decoder_boundary_matrix() {
        let registry = registry();
        let read = &registry.definitions()[0];
        let shell = &registry.definitions()[1];
        let path_max = 4_096;
        let command_max = policy().run_shell_command_max_bytes as usize;

        for (label, value, expected) in [
            ("read shortest path", json!({"path":"a"}), true),
            (
                "read maximum path",
                json!({"path":"a".repeat(path_max)}),
                true,
            ),
            (
                "read path over maximum",
                json!({"path":"a".repeat(path_max + 1)}),
                false,
            ),
            ("read NUL", json!({"path":"a\0b"}), false),
            ("read backslash", json!({"path":"a\\b"}), false),
            ("read traversal", json!({"path":"../a"}), false),
            (
                "read workspace relative",
                json!({"path":"src/lib.rs","path_kind":"workspace_relative"}),
                true,
            ),
            (
                "read absolute",
                json!({"path":"/etc/hosts","path_kind":"absolute"}),
                true,
            ),
            (
                "read path kind mismatch",
                json!({"path":"/etc/hosts","path_kind":"workspace_relative"}),
                false,
            ),
            (
                "read max bytes minimum",
                json!({"path":"a","max_bytes":1}),
                true,
            ),
            (
                "read configured default",
                json!({"path":"a","max_bytes":policy().read_file_default_bytes}),
                true,
            ),
            (
                "read hard maximum",
                json!({"path":"a","max_bytes":HARD_FILE_READ_MAX_BYTES}),
                true,
            ),
            (
                "read over hard maximum",
                json!({"path":"a","max_bytes":HARD_FILE_READ_MAX_BYTES + 1}),
                false,
            ),
            (
                "read unknown field",
                json!({"path":"a","unknown":true}),
                false,
            ),
        ] {
            assert_schema_decoder_case(label, read, value, expected);
        }

        for (label, value, expected) in [
            ("shell command length one", json!({"command":"a"}), true),
            (
                "shell command exact maximum",
                json!({"command":"a".repeat(command_max)}),
                true,
            ),
            (
                "shell command over maximum",
                json!({"command":"a".repeat(command_max + 1)}),
                false,
            ),
            (
                "shell multibyte near maximum",
                json!({"command":format!("{}é", "a".repeat(command_max - 1))}),
                false,
            ),
            (
                "shell pure multibyte",
                json!({"command":"é".repeat(command_max / 2)}),
                false,
            ),
            ("shell NUL", json!({"command":"a\0b"}), false),
            ("shell cwd shortest", json!({"command":"a","cwd":"a"}), true),
            (
                "shell cwd exact maximum",
                json!({"command":"a","cwd":"a".repeat(path_max)}),
                true,
            ),
            (
                "shell cwd over maximum",
                json!({"command":"a","cwd":"a".repeat(path_max + 1)}),
                false,
            ),
            (
                "shell timeout minimum",
                json!({"command":"a","timeout_seconds":1}),
                true,
            ),
            (
                "shell configured default",
                json!({"command":"a","timeout_seconds":policy().run_shell_default_timeout_ms / 1_000}),
                true,
            ),
            (
                "shell hard maximum",
                json!({"command":"a","timeout_seconds":HARD_EXECUTION_TIMEOUT_MS / 1_000}),
                true,
            ),
            (
                "shell over hard maximum",
                json!({"command":"a","timeout_seconds":HARD_EXECUTION_TIMEOUT_MS / 1_000 + 1}),
                false,
            ),
            (
                "shell user privilege",
                json!({"command":"a","privilege":"user"}),
                true,
            ),
            (
                "shell administrative privilege",
                json!({"command":"a","privilege":"administrative"}),
                true,
            ),
            (
                "shell unknown field",
                json!({"command":"a","unknown":true}),
                false,
            ),
        ] {
            assert_schema_decoder_case(label, shell, value, expected);
        }

        let mut unknown = shell.input_schema().clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("format".to_owned(), Value::String("opaque".to_owned()));
        assert!(emitted_schema_accepts(&unknown, &json!({"command":"a"})).is_err());
    }

    #[test]
    fn emitted_schema_mutations_prove_evaluator_decoder_independence() {
        let registry = registry();
        let read = &registry.definitions()[0];
        let shell = &registry.definitions()[1];

        let mut command_wider = shell.input_schema().clone();
        command_wider["properties"]["command"]["maxLength"] =
            Value::from(policy().run_shell_command_max_bytes + 1);
        let wider_command =
            json!({"command":"a".repeat(policy().run_shell_command_max_bytes as usize + 1)});
        assert!(emitted_schema_accepts(&command_wider, &wider_command).unwrap());
        assert!(validate_arguments(shell, &serde_json::to_vec(&wider_command).unwrap()).is_err());

        let mut path_wider = read.input_schema().clone();
        path_wider["oneOf"][0]["properties"]["path"]
            .as_object_mut()
            .unwrap()
            .remove("pattern");
        let traversal = json!({"path":"../escape"});
        assert!(emitted_schema_accepts(&path_wider, &traversal).unwrap());
        assert!(validate_arguments(read, &serde_json::to_vec(&traversal).unwrap()).is_err());

        let mut timeout_wider = shell.input_schema().clone();
        timeout_wider["properties"]["timeout_seconds"]["maximum"] =
            Value::from(HARD_EXECUTION_TIMEOUT_MS / 1_000 + 1);
        let overtime =
            json!({"command":"a","timeout_seconds":HARD_EXECUTION_TIMEOUT_MS / 1_000 + 1});
        assert!(emitted_schema_accepts(&timeout_wider, &overtime).unwrap());
        assert!(validate_arguments(shell, &serde_json::to_vec(&overtime).unwrap()).is_err());
    }

    #[test]
    fn configured_semantic_defaults_drive_schema_decoder_normalization_and_fingerprint() {
        let configured = ToolSemanticPolicy {
            read_file_default_bytes: 65_536,
            read_file_max_bytes: 131_072,
            run_shell_command_max_bytes: 4_096,
            run_shell_default_timeout_ms: 30_000,
            run_shell_max_timeout_ms: 60_000,
        };
        let registry = ToolRegistry::v0(configured).unwrap();
        let read = &registry.definitions()[0];
        let shell = &registry.definitions()[1];
        assert_eq!(
            read.input_schema()["oneOf"][0]["properties"]["max_bytes"]["default"],
            65_536
        );
        assert_eq!(
            read.input_schema()["oneOf"][0]["properties"]["max_bytes"]["maximum"],
            131_072
        );
        assert_eq!(
            shell.input_schema()["properties"]["timeout_seconds"]["default"],
            30
        );
        assert_eq!(
            shell.input_schema()["properties"]["timeout_seconds"]["maximum"],
            60
        );
        assert_eq!(
            validate_arguments(read, br#"{"path":"x"}"#)
                .unwrap()
                .canonical_json(),
            r#"{"max_bytes":65536,"path":"x","path_kind":"workspace_relative"}"#
        );
        assert_eq!(
            validate_arguments(shell, br#"{"command":"true"}"#)
                .unwrap()
                .canonical_json(),
            r#"{"command":"true","cwd":null,"privilege":"user","timeout_seconds":30}"#
        );
        assert!(validate_arguments(read, br#"{"path":"x","max_bytes":131073}"#).is_err());
        assert!(validate_arguments(shell, br#"{"command":"true","timeout_seconds":61}"#).is_err());
        assert_ne!(registry.fingerprint(), self::registry().fingerprint());
    }
}
