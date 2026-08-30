use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};

use url::{Host, Url};

use crate::bootstrap::compatibility::CONFIGURATION_VERSION;
use crate::bootstrap::credential::{CredentialRef, CredentialSourceConfig};

use super::error::ConfigError;
use super::fingerprint::ConfigFingerprint;
use super::raw::{
    RawAgentLimits, RawConfig, RawCredentials, RawDeviceAuth, RawFailpointMode, RawLimits,
    RawModelCapabilities, RawModelGateway, RawModelTarget, RawModels, RawPaths, RawProtocolLimits,
    RawServer, RawShell, RawShutdown, RawSqlite, RawToolLimits, RawTracing, RawWorkstation,
};

const MAX_POOL_CONNECTIONS: u64 = 4;
const BUSY_TIMEOUT_MS: u64 = 5_000;
const WAL_AUTOCHECKPOINT_PAGES: u64 = 1_000;

const MAX_MODEL_STEPS_PER_WORK: u64 = 16;
const MAX_MODEL_ATTEMPTS_PER_WORK: u64 = 32;
const MAX_TOOL_CALLS_PER_WORK: u64 = 32;
const MAX_ORDERED_OUTPUT_ITEMS_PER_RESPONSE: u64 = 64;
const MAX_RAW_TOOL_ARGUMENT_BYTES: u64 = 65_536;
const MAX_WORK_ITEM_DURATION_MS: u64 = 1_800_000;

const MAX_PROVIDER_ATTEMPTS: u64 = 3;
const MAX_INVOCATION_TIMEOUT_MS: u64 = 300_000;
const MAX_RESPONSE_IDLE_TIMEOUT_MS: u64 = 60_000;

const MAX_READ_FILE_BYTES: u64 = 8_388_608;
const MAX_SHELL_COMMAND_BYTES: u64 = 65_536;
const MAX_SHELL_TIMEOUT_MS: u64 = 900_000;
const MAX_CAPTURE_BYTES: u64 = 8_388_608;
const MAX_INLINE_MODEL_RESULT_BYTES: u64 = 65_536;
const MAX_PER_STREAM_PROJECTION_BYTES: u64 = 32_768;

const MAX_WEBSOCKET_DURABLE_PAYLOAD_BYTES: u64 = 262_144;
const MAX_USER_TEXT_MESSAGE_BYTES: u64 = 65_536;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NormalizedUrl(String);

impl NormalizedUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub struct ValidatedConfig {
    pub(super) data: ConfigData,
    fingerprint: ConfigFingerprint,
}

impl ValidatedConfig {
    pub(super) fn from_raw(raw: RawConfig) -> Result<Self, ConfigError> {
        if raw.configuration_version != CONFIGURATION_VERSION as i64 {
            return Err(ConfigError::UnsupportedConfigurationVersion {
                found: raw.configuration_version,
                supported: CONFIGURATION_VERSION,
            });
        }

        let data = ConfigData {
            configuration_version: CONFIGURATION_VERSION,
            failpoint_mode: validate_failpoint_mode(raw.failpoint_mode),
            server: validate_server(raw.server)?,
            paths: validate_paths(raw.paths)?,
            sqlite: validate_sqlite(raw.sqlite)?,
            workstation: validate_workstation(raw.workstation)?,
            credentials: validate_credentials(raw.credentials)?,
            models: ModelsConfig {
                default_target: String::new(),
                targets: Vec::new(),
            },
            model_gateway: validate_model_gateway(raw.model_gateway)?,
            limits: validate_limits(raw.limits)?,
            shell: validate_shell(raw.shell)?,
            device_auth: validate_device_auth(raw.device_auth)?,
            tracing: validate_tracing(raw.tracing)?,
            shutdown: validate_shutdown(raw.shutdown)?,
        };

        let mut data = ConfigData {
            models: validate_models(raw.models, &data.credentials)?,
            ..data
        };
        validate_cross_field_limits(&data)?;

        data.models
            .targets
            .sort_by(|left, right| left.id.cmp(&right.id));
        let fingerprint = ConfigFingerprint::from_config(&data);
        Ok(Self { data, fingerprint })
    }

    pub fn configuration_version(&self) -> u64 {
        self.data.configuration_version
    }

    pub fn failpoint_mode(&self) -> FailpointMode {
        self.data.failpoint_mode
    }

    pub fn server(&self) -> &ServerConfig {
        &self.data.server
    }

    pub fn paths(&self) -> &PathsConfig {
        &self.data.paths
    }

    pub fn sqlite(&self) -> &SqliteConfig {
        &self.data.sqlite
    }

    pub fn workstation(&self) -> &WorkstationConfig {
        &self.data.workstation
    }

    pub fn credentials(&self) -> &CredentialsConfig {
        &self.data.credentials
    }

    pub fn models(&self) -> &ModelsConfig {
        &self.data.models
    }

    pub fn model_gateway(&self) -> &ModelGatewayConfig {
        &self.data.model_gateway
    }

    pub fn limits(&self) -> &LimitsConfig {
        &self.data.limits
    }

    pub fn shell(&self) -> &ShellConfig {
        &self.data.shell
    }

    pub fn device_auth(&self) -> &DeviceAuthConfig {
        &self.data.device_auth
    }

    pub fn tracing(&self) -> &TracingConfig {
        &self.data.tracing
    }

    pub fn shutdown(&self) -> &ShutdownConfig {
        &self.data.shutdown
    }

    pub fn fingerprint(&self) -> &ConfigFingerprint {
        &self.fingerprint
    }
}

#[derive(Clone)]
pub(super) struct ConfigData {
    pub(super) configuration_version: u64,
    pub(super) failpoint_mode: FailpointMode,
    pub(super) server: ServerConfig,
    pub(super) paths: PathsConfig,
    pub(super) sqlite: SqliteConfig,
    pub(super) workstation: WorkstationConfig,
    pub(super) credentials: CredentialsConfig,
    pub(super) models: ModelsConfig,
    pub(super) model_gateway: ModelGatewayConfig,
    pub(super) limits: LimitsConfig,
    pub(super) shell: ShellConfig,
    pub(super) device_auth: DeviceAuthConfig,
    pub(super) tracing: TracingConfig,
    pub(super) shutdown: ShutdownConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailpointMode {
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerConfig {
    pub(super) bind_address: SocketAddr,
    pub(super) public_base_url: NormalizedUrl,
}

impl ServerConfig {
    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    pub fn public_base_url(&self) -> &NormalizedUrl {
        &self.public_base_url
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathsConfig {
    pub(super) state_root: PathBuf,
    pub(super) artifact_root: PathBuf,
    pub(super) primary_workspace_root: PathBuf,
}

impl PathsConfig {
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    pub fn primary_workspace_root(&self) -> &Path {
        &self.primary_workspace_root
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteConfig {
    pub(super) pool_connections: u64,
    pub(super) busy_timeout_ms: u64,
    pub(super) wal_autocheckpoint_pages: u64,
}

impl SqliteConfig {
    pub fn pool_connections(&self) -> u64 {
        self.pool_connections
    }

    pub fn busy_timeout_ms(&self) -> u64 {
        self.busy_timeout_ms
    }

    pub fn wal_autocheckpoint_pages(&self) -> u64 {
        self.wal_autocheckpoint_pages
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkstationIdentitySource {
    StateStore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkstationConfig {
    pub(super) identity_source: WorkstationIdentitySource,
    pub(super) initial_generation: u64,
    pub(super) primary_workspace_logical_name: String,
}

impl WorkstationConfig {
    pub fn identity_source(&self) -> WorkstationIdentitySource {
        self.identity_source
    }

    pub fn initial_generation(&self) -> u64 {
        self.initial_generation
    }

    pub fn primary_workspace_logical_name(&self) -> &str {
        &self.primary_workspace_logical_name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialsConfig {
    pub(super) source: CredentialSourceConfig,
    pub(super) declared: Vec<CredentialRef>,
}

impl CredentialsConfig {
    pub fn source(&self) -> &CredentialSourceConfig {
        &self.source
    }

    pub fn declared(&self) -> &[CredentialRef] {
        &self.declared
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelProvider {
    OpenAi,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilities {
    pub(super) text_input: bool,
    pub(super) text_output: bool,
    pub(super) custom_tool_calling: bool,
    pub(super) streaming: bool,
    pub(super) ordered_output_items: bool,
    pub(super) structured_output: bool,
    pub(super) reasoning_continuation: bool,
}

impl ModelCapabilities {
    pub fn text_input(&self) -> bool {
        self.text_input
    }

    pub fn text_output(&self) -> bool {
        self.text_output
    }

    pub fn custom_tool_calling(&self) -> bool {
        self.custom_tool_calling
    }

    pub fn streaming(&self) -> bool {
        self.streaming
    }

    pub fn ordered_output_items(&self) -> bool {
        self.ordered_output_items
    }

    pub fn structured_output(&self) -> bool {
        self.structured_output
    }

    pub fn reasoning_continuation(&self) -> bool {
        self.reasoning_continuation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTargetConfig {
    pub(super) id: String,
    pub(super) config_version: u64,
    pub(super) enabled: bool,
    pub(super) provider: ModelProvider,
    pub(super) provider_model_id: String,
    pub(super) endpoint: NormalizedUrl,
    pub(super) credential: CredentialRef,
    pub(super) token_estimator: String,
    pub(super) context_window_tokens: u64,
    pub(super) max_output_tokens: u64,
    pub(super) requested_output_tokens: u64,
    pub(super) reasoning_continuation: bool,
    pub(super) capabilities: ModelCapabilities,
}

impl ModelTargetConfig {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn config_version(&self) -> u64 {
        self.config_version
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn provider(&self) -> ModelProvider {
        self.provider
    }

    pub fn provider_model_id(&self) -> &str {
        &self.provider_model_id
    }

    pub fn endpoint(&self) -> &NormalizedUrl {
        &self.endpoint
    }

    pub fn credential(&self) -> &CredentialRef {
        &self.credential
    }

    pub fn token_estimator(&self) -> &str {
        &self.token_estimator
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens
    }

    pub fn max_output_tokens(&self) -> u64 {
        self.max_output_tokens
    }

    pub fn requested_output_tokens(&self) -> u64 {
        self.requested_output_tokens
    }

    pub fn reasoning_continuation_required(&self) -> bool {
        self.reasoning_continuation
    }

    pub fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelsConfig {
    pub(super) default_target: String,
    pub(super) targets: Vec<ModelTargetConfig>,
}

impl ModelsConfig {
    pub fn default_target(&self) -> &str {
        &self.default_target
    }

    pub fn targets(&self) -> &[ModelTargetConfig] {
        &self.targets
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelGatewayConfig {
    pub(super) max_attempts_per_invocation: u64,
    pub(super) invocation_timeout_ms: u64,
    pub(super) response_idle_timeout_ms: u64,
}

impl ModelGatewayConfig {
    pub fn max_attempts_per_invocation(&self) -> u64 {
        self.max_attempts_per_invocation
    }

    pub fn invocation_timeout_ms(&self) -> u64 {
        self.invocation_timeout_ms
    }

    pub fn response_idle_timeout_ms(&self) -> u64 {
        self.response_idle_timeout_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLimits {
    pub(super) max_model_steps_per_work: u64,
    pub(super) max_model_attempts_per_work: u64,
    pub(super) max_tool_calls_per_work: u64,
    pub(super) max_ordered_output_items_per_response: u64,
    pub(super) max_raw_tool_argument_bytes: u64,
    pub(super) max_work_item_duration_ms: u64,
}

impl AgentLimits {
    pub fn max_model_steps_per_work(&self) -> u64 {
        self.max_model_steps_per_work
    }

    pub fn max_model_attempts_per_work(&self) -> u64 {
        self.max_model_attempts_per_work
    }

    pub fn max_tool_calls_per_work(&self) -> u64 {
        self.max_tool_calls_per_work
    }

    pub fn max_ordered_output_items_per_response(&self) -> u64 {
        self.max_ordered_output_items_per_response
    }

    pub fn max_raw_tool_argument_bytes(&self) -> u64 {
        self.max_raw_tool_argument_bytes
    }

    pub fn max_work_item_duration_ms(&self) -> u64 {
        self.max_work_item_duration_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolLimits {
    pub(super) read_file_default_bytes: u64,
    pub(super) read_file_max_bytes: u64,
    pub(super) run_shell_command_max_bytes: u64,
    pub(super) run_shell_default_timeout_ms: u64,
    pub(super) run_shell_max_timeout_ms: u64,
    pub(super) stdout_capture_bytes: u64,
    pub(super) stderr_capture_bytes: u64,
    pub(super) inline_model_result_bytes: u64,
    pub(super) per_stream_projection_bytes: u64,
}

impl ToolLimits {
    pub fn read_file_default_bytes(&self) -> u64 {
        self.read_file_default_bytes
    }

    pub fn read_file_max_bytes(&self) -> u64 {
        self.read_file_max_bytes
    }

    pub fn run_shell_command_max_bytes(&self) -> u64 {
        self.run_shell_command_max_bytes
    }

    pub fn run_shell_default_timeout_ms(&self) -> u64 {
        self.run_shell_default_timeout_ms
    }

    pub fn run_shell_max_timeout_ms(&self) -> u64 {
        self.run_shell_max_timeout_ms
    }

    pub fn stdout_capture_bytes(&self) -> u64 {
        self.stdout_capture_bytes
    }

    pub fn stderr_capture_bytes(&self) -> u64 {
        self.stderr_capture_bytes
    }

    pub fn inline_model_result_bytes(&self) -> u64 {
        self.inline_model_result_bytes
    }

    pub fn per_stream_projection_bytes(&self) -> u64 {
        self.per_stream_projection_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolLimits {
    pub(super) websocket_durable_payload_bytes: u64,
    pub(super) user_text_message_bytes: u64,
}

impl ProtocolLimits {
    pub fn websocket_durable_payload_bytes(&self) -> u64 {
        self.websocket_durable_payload_bytes
    }

    pub fn user_text_message_bytes(&self) -> u64 {
        self.user_text_message_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitsConfig {
    pub(super) agent: AgentLimits,
    pub(super) tools: ToolLimits,
    pub(super) protocol: ProtocolLimits,
}

impl LimitsConfig {
    pub fn agent(&self) -> &AgentLimits {
        &self.agent
    }

    pub fn tools(&self) -> &ToolLimits {
        &self.tools
    }

    pub fn protocol(&self) -> &ProtocolLimits {
        &self.protocol
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellEnvironmentPolicy {
    Clean,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellConfig {
    pub(super) executable: PathBuf,
    pub(super) environment_policy: ShellEnvironmentPolicy,
    pub(super) inherited_variables: Vec<String>,
    pub(super) administrative_enabled: bool,
    pub(super) delegated_cgroup_root: Option<PathBuf>,
}

impl ShellConfig {
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn environment_policy(&self) -> ShellEnvironmentPolicy {
        self.environment_policy
    }

    pub fn inherited_variables(&self) -> &[String] {
        &self.inherited_variables
    }

    pub fn administrative_enabled(&self) -> bool {
        self.administrative_enabled
    }

    pub fn delegated_cgroup_root(&self) -> Option<&Path> {
        self.delegated_cgroup_root.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceAuthSource {
    ProvisionedSqlite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceAuthConfig {
    pub(super) source: DeviceAuthSource,
}

impl DeviceAuthConfig {
    pub fn source(&self) -> DeviceAuthSource {
        self.source
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TracingFormat {
    Pretty,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TracingFilter {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TracingConfig {
    pub(super) format: TracingFormat,
    pub(super) filter: TracingFilter,
}

impl TracingConfig {
    pub fn format(&self) -> TracingFormat {
        self.format
    }

    pub fn filter(&self) -> TracingFilter {
        self.filter
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownConfig {
    pub(super) grace_period_ms: u64,
}

impl ShutdownConfig {
    pub fn grace_period_ms(&self) -> u64 {
        self.grace_period_ms
    }
}

fn validate_failpoint_mode(raw: RawFailpointMode) -> FailpointMode {
    match raw {
        RawFailpointMode::Disabled => FailpointMode::Disabled,
    }
}

fn validate_server(raw: RawServer) -> Result<ServerConfig, ConfigError> {
    let bind_address =
        raw.bind_address
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::UnsafeBind {
                reason: "bind_address must be an IP socket address",
            })?;
    if bind_address.port() == 0 {
        return Err(ConfigError::UnsafeBind {
            reason: "port zero is not allowed",
        });
    }
    if !bind_address.ip().is_loopback() {
        return Err(ConfigError::UnsafeBind {
            reason: "bind_address must be loopback",
        });
    }

    let public_base_url = validate_public_url(&raw.public_base_url)?;
    Ok(ServerConfig {
        bind_address,
        public_base_url,
    })
}

fn validate_public_url(value: &str) -> Result<NormalizedUrl, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidPublicUrl {
        reason: "must be an absolute HTTP or HTTPS URL",
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::InvalidPublicUrl {
            reason: "scheme must be HTTP or HTTPS",
        });
    }
    if url.host().is_none() {
        return Err(ConfigError::InvalidPublicUrl {
            reason: "host is required",
        });
    }
    reject_url_extras(&url).map_err(|reason| ConfigError::InvalidPublicUrl { reason })?;
    if url.path() != "/" {
        return Err(ConfigError::InvalidPublicUrl {
            reason: "only the root base path is supported",
        });
    }
    if url.scheme() == "http" && !url_host_is_loopback(&url) {
        return Err(ConfigError::InvalidPublicUrl {
            reason: "HTTP is allowed only for localhost or loopback",
        });
    }
    Ok(NormalizedUrl(url.to_string()))
}

fn reject_url_extras(url: &Url) -> Result<(), &'static str> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err("userinfo is not allowed");
    }
    if url.query().is_some() {
        return Err("query is not allowed");
    }
    if url.fragment().is_some() {
        return Err("fragment is not allowed");
    }
    Ok(())
}

fn url_host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

fn validate_paths(raw: RawPaths) -> Result<PathsConfig, ConfigError> {
    let state_root = validate_lexical_path("paths.state_root", &raw.state_root)?;
    let artifact_root = validate_lexical_path("paths.artifact_root", &raw.artifact_root)?;
    let primary_workspace_root =
        validate_lexical_path("paths.primary_workspace_root", &raw.primary_workspace_root)?;

    if state_root == artifact_root {
        return Err(ConfigError::InvalidPath {
            field: "paths.artifact_root",
            reason: "state_root and artifact_root must differ",
        });
    }
    if state_root.starts_with(&artifact_root) {
        return Err(ConfigError::InvalidPath {
            field: "paths.artifact_root",
            reason: "artifact_root may be below state_root, but not its parent",
        });
    }
    if paths_overlap(&primary_workspace_root, &state_root)
        || paths_overlap(&primary_workspace_root, &artifact_root)
    {
        return Err(ConfigError::InvalidPath {
            field: "paths.primary_workspace_root",
            reason: "workspace root must be disjoint from state and artifact roots",
        });
    }

    Ok(PathsConfig {
        state_root,
        artifact_root,
        primary_workspace_root,
    })
}

fn validate_lexical_path(field: &'static str, value: &str) -> Result<PathBuf, ConfigError> {
    if value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidPath {
            field,
            reason: "control characters are not allowed",
        });
    }
    if value
        .split('/')
        .any(|component| matches!(component, "." | ".."))
    {
        return Err(ConfigError::InvalidPath {
            field,
            reason: "dot and parent components are not allowed",
        });
    }

    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ConfigError::InvalidPath {
            field,
            reason: "path must be absolute",
        });
    }

    let mut normalized = PathBuf::from("/");
    let mut normal_components = 0_u64;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                normalized.push(component);
                normal_components += 1;
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(ConfigError::InvalidPath {
                    field,
                    reason: "path contains an unsupported component",
                });
            }
        }
    }
    if normal_components == 0 {
        return Err(ConfigError::InvalidPath {
            field,
            reason: "filesystem root is not allowed",
        });
    }
    Ok(normalized)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_sqlite(raw: RawSqlite) -> Result<SqliteConfig, ConfigError> {
    let pool_connections = bounded(
        raw.pool_connections,
        "sqlite.pool_connections",
        MAX_POOL_CONNECTIONS,
    )?;
    let busy_timeout_ms = fixed_sqlite(
        raw.busy_timeout_ms,
        "sqlite.busy_timeout_ms",
        BUSY_TIMEOUT_MS,
    )?;
    let wal_autocheckpoint_pages = fixed_sqlite(
        raw.wal_autocheckpoint_pages,
        "sqlite.wal_autocheckpoint_pages",
        WAL_AUTOCHECKPOINT_PAGES,
    )?;
    Ok(SqliteConfig {
        pool_connections,
        busy_timeout_ms,
        wal_autocheckpoint_pages,
    })
}

fn fixed_sqlite(raw: i64, field: &'static str, expected: u64) -> Result<u64, ConfigError> {
    if raw != expected as i64 {
        return Err(ConfigError::InvalidSqliteTuning {
            field,
            reason: "V0 requires the architecture-fixed value",
        });
    }
    Ok(expected)
}

fn validate_workstation(raw: RawWorkstation) -> Result<WorkstationConfig, ConfigError> {
    let identity_source = match raw.identity_source.as_str() {
        "state_store" => WorkstationIdentitySource::StateStore,
        _ => {
            return Err(ConfigError::InvalidLogicalName {
                field: "workstation.identity_source",
            });
        }
    };
    if raw.initial_generation <= 0 {
        return Err(ConfigError::InvalidWorkstationGeneration);
    }
    validate_logical_name(
        "workstation.primary_workspace_logical_name",
        &raw.primary_workspace_logical_name,
    )?;
    Ok(WorkstationConfig {
        identity_source,
        initial_generation: raw.initial_generation as u64,
        primary_workspace_logical_name: raw.primary_workspace_logical_name,
    })
}

fn validate_credentials(raw: RawCredentials) -> Result<CredentialsConfig, ConfigError> {
    let source = match raw.source.as_str() {
        "local_directory" => {
            let directory = raw.directory.ok_or(ConfigError::InvalidCredentialSource {
                reason: "local_directory requires directory",
            })?;
            CredentialSourceConfig::LocalDirectory {
                directory: validate_lexical_path("credentials.directory", &directory)?,
            }
        }
        "systemd" => {
            if raw.directory.is_some() {
                return Err(ConfigError::InvalidCredentialSource {
                    reason: "systemd does not accept a TOML credential directory",
                });
            }
            CredentialSourceConfig::Systemd
        }
        _ => {
            return Err(ConfigError::InvalidCredentialSource {
                reason: "source must be local_directory or systemd",
            });
        }
    };

    let mut seen = BTreeSet::new();
    let mut declared = Vec::with_capacity(raw.declared.len());
    for value in raw.declared {
        if !is_logical_name(&value) {
            return Err(ConfigError::InvalidCredentialRef {
                field: "credentials.declared",
            });
        }
        if !seen.insert(value.clone()) {
            return Err(ConfigError::DuplicateCredentialDeclaration { credential: value });
        }
        declared.push(CredentialRef::new(value));
    }
    declared.sort();
    Ok(CredentialsConfig { source, declared })
}

fn validate_models(
    raw: RawModels,
    credentials: &CredentialsConfig,
) -> Result<ModelsConfig, ConfigError> {
    validate_logical_name("models.default_target", &raw.default_target)?;
    let declared_credentials: BTreeSet<&str> = credentials
        .declared
        .iter()
        .map(CredentialRef::as_str)
        .collect();
    let mut seen = BTreeSet::new();
    let mut targets = Vec::with_capacity(raw.targets.len());
    for target in raw.targets {
        if !is_logical_name(&target.id) {
            return Err(ConfigError::InvalidLogicalName {
                field: "models.targets.id",
            });
        }
        if !seen.insert(target.id.clone()) {
            return Err(ConfigError::DuplicateModelTarget { target: target.id });
        }
        targets.push(validate_model_target(target, &declared_credentials)?);
    }

    let Some(default) = targets
        .iter()
        .find(|target| target.id == raw.default_target)
    else {
        return Err(ConfigError::MissingDefaultTarget {
            target: raw.default_target,
        });
    };
    if !default.enabled {
        return Err(ConfigError::DisabledDefaultTarget {
            target: raw.default_target,
        });
    }

    Ok(ModelsConfig {
        default_target: raw.default_target,
        targets,
    })
}

fn validate_model_target(
    raw: RawModelTarget,
    declared_credentials: &BTreeSet<&str>,
) -> Result<ModelTargetConfig, ConfigError> {
    let target = raw.id.clone();
    if raw.config_version < 1 {
        return Err(invalid_model(
            &target,
            "config_version",
            "must be at least 1",
        ));
    }
    let provider = match raw.provider.as_str() {
        "openai" => ModelProvider::OpenAi,
        _ => {
            return Err(invalid_model(
                &target,
                "provider",
                "V0 supports only openai",
            ));
        }
    };
    if raw.provider_model_id.is_empty()
        || raw.provider_model_id.trim() != raw.provider_model_id
        || raw.provider_model_id.chars().any(char::is_control)
    {
        return Err(invalid_model(
            &target,
            "provider_model_id",
            "must be nonempty, trimmed, and control-free",
        ));
    }
    let endpoint = validate_provider_url(&target, &raw.endpoint)?;
    if !is_logical_name(&raw.credential) {
        return Err(ConfigError::InvalidCredentialRef {
            field: "models.targets.credential",
        });
    }
    if !declared_credentials.contains(raw.credential.as_str()) {
        return Err(ConfigError::UndeclaredCredentialRef {
            credential: raw.credential,
            target,
        });
    }
    if !is_logical_name(&raw.token_estimator) {
        return Err(invalid_model(
            &raw.id,
            "token_estimator",
            "must be a logical identifier",
        ));
    }

    let context_window_tokens =
        positive_model_value(raw.context_window_tokens, &raw.id, "context_window_tokens")?;
    let max_output_tokens =
        positive_model_value(raw.max_output_tokens, &raw.id, "max_output_tokens")?;
    let requested_output_tokens = positive_model_value(
        raw.requested_output_tokens,
        &raw.id,
        "requested_output_tokens",
    )?;
    if requested_output_tokens > max_output_tokens {
        return Err(invalid_model(
            &raw.id,
            "requested_output_tokens",
            "must not exceed max_output_tokens",
        ));
    }
    if max_output_tokens >= context_window_tokens {
        return Err(invalid_model(
            &raw.id,
            "max_output_tokens",
            "must be less than context_window_tokens",
        ));
    }
    if requested_output_tokens >= context_window_tokens {
        return Err(invalid_model(
            &raw.id,
            "requested_output_tokens",
            "must be less than context_window_tokens",
        ));
    }

    let capabilities = validate_capabilities(raw.capabilities);
    if raw.reasoning_continuation && !capabilities.reasoning_continuation {
        return Err(ConfigError::InvalidModelCapabilityRelationship { target: raw.id });
    }

    Ok(ModelTargetConfig {
        id: raw.id,
        config_version: raw.config_version as u64,
        enabled: raw.enabled,
        provider,
        provider_model_id: raw.provider_model_id,
        endpoint,
        credential: CredentialRef::new(raw.credential),
        token_estimator: raw.token_estimator,
        context_window_tokens,
        max_output_tokens,
        requested_output_tokens,
        reasoning_continuation: raw.reasoning_continuation,
        capabilities,
    })
}

fn invalid_model(target: &str, field: &'static str, reason: &'static str) -> ConfigError {
    ConfigError::InvalidModelTarget {
        target: target.to_owned(),
        field,
        reason,
    }
}

fn positive_model_value(raw: i64, target: &str, field: &'static str) -> Result<u64, ConfigError> {
    if raw <= 0 {
        return Err(invalid_model(target, field, "must be positive"));
    }
    Ok(raw as u64)
}

fn validate_provider_url(target: &str, value: &str) -> Result<NormalizedUrl, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidProviderUrl {
        target: target.to_owned(),
        reason: "must be an absolute HTTPS URL",
    })?;
    if url.scheme() != "https" || url.host().is_none() {
        return Err(ConfigError::InvalidProviderUrl {
            target: target.to_owned(),
            reason: "HTTPS scheme and host are required",
        });
    }
    reject_url_extras(&url).map_err(|reason| ConfigError::InvalidProviderUrl {
        target: target.to_owned(),
        reason,
    })?;
    Ok(NormalizedUrl(url.to_string()))
}

fn validate_capabilities(raw: RawModelCapabilities) -> ModelCapabilities {
    ModelCapabilities {
        text_input: raw.text_input,
        text_output: raw.text_output,
        custom_tool_calling: raw.custom_tool_calling,
        streaming: raw.streaming,
        ordered_output_items: raw.ordered_output_items,
        structured_output: raw.structured_output,
        reasoning_continuation: raw.reasoning_continuation,
    }
}

fn validate_model_gateway(raw: RawModelGateway) -> Result<ModelGatewayConfig, ConfigError> {
    let max_attempts_per_invocation = bounded(
        raw.max_attempts_per_invocation,
        "model_gateway.max_attempts_per_invocation",
        MAX_PROVIDER_ATTEMPTS,
    )?;
    let invocation_timeout_ms = bounded(
        raw.invocation_timeout_ms,
        "model_gateway.invocation_timeout_ms",
        MAX_INVOCATION_TIMEOUT_MS,
    )?;
    let response_idle_timeout_ms = bounded(
        raw.response_idle_timeout_ms,
        "model_gateway.response_idle_timeout_ms",
        MAX_RESPONSE_IDLE_TIMEOUT_MS,
    )?;
    if response_idle_timeout_ms > invocation_timeout_ms {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "model_gateway.response_idle_timeout_ms",
            upper: "model_gateway.invocation_timeout_ms",
        });
    }
    Ok(ModelGatewayConfig {
        max_attempts_per_invocation,
        invocation_timeout_ms,
        response_idle_timeout_ms,
    })
}

fn validate_limits(raw: RawLimits) -> Result<LimitsConfig, ConfigError> {
    Ok(LimitsConfig {
        agent: validate_agent_limits(raw.agent)?,
        tools: validate_tool_limits(raw.tools)?,
        protocol: validate_protocol_limits(raw.protocol)?,
    })
}

fn validate_agent_limits(raw: RawAgentLimits) -> Result<AgentLimits, ConfigError> {
    Ok(AgentLimits {
        max_model_steps_per_work: bounded(
            raw.max_model_steps_per_work,
            "limits.agent.max_model_steps_per_work",
            MAX_MODEL_STEPS_PER_WORK,
        )?,
        max_model_attempts_per_work: bounded(
            raw.max_model_attempts_per_work,
            "limits.agent.max_model_attempts_per_work",
            MAX_MODEL_ATTEMPTS_PER_WORK,
        )?,
        max_tool_calls_per_work: bounded(
            raw.max_tool_calls_per_work,
            "limits.agent.max_tool_calls_per_work",
            MAX_TOOL_CALLS_PER_WORK,
        )?,
        max_ordered_output_items_per_response: bounded(
            raw.max_ordered_output_items_per_response,
            "limits.agent.max_ordered_output_items_per_response",
            MAX_ORDERED_OUTPUT_ITEMS_PER_RESPONSE,
        )?,
        max_raw_tool_argument_bytes: bounded(
            raw.max_raw_tool_argument_bytes,
            "limits.agent.max_raw_tool_argument_bytes",
            MAX_RAW_TOOL_ARGUMENT_BYTES,
        )?,
        max_work_item_duration_ms: bounded(
            raw.max_work_item_duration_ms,
            "limits.agent.max_work_item_duration_ms",
            MAX_WORK_ITEM_DURATION_MS,
        )?,
    })
}

fn validate_tool_limits(raw: RawToolLimits) -> Result<ToolLimits, ConfigError> {
    let limits = ToolLimits {
        read_file_default_bytes: bounded(
            raw.read_file_default_bytes,
            "limits.tools.read_file_default_bytes",
            MAX_READ_FILE_BYTES,
        )?,
        read_file_max_bytes: bounded(
            raw.read_file_max_bytes,
            "limits.tools.read_file_max_bytes",
            MAX_READ_FILE_BYTES,
        )?,
        run_shell_command_max_bytes: bounded(
            raw.run_shell_command_max_bytes,
            "limits.tools.run_shell_command_max_bytes",
            MAX_SHELL_COMMAND_BYTES,
        )?,
        run_shell_default_timeout_ms: bounded(
            raw.run_shell_default_timeout_ms,
            "limits.tools.run_shell_default_timeout_ms",
            MAX_SHELL_TIMEOUT_MS,
        )?,
        run_shell_max_timeout_ms: bounded(
            raw.run_shell_max_timeout_ms,
            "limits.tools.run_shell_max_timeout_ms",
            MAX_SHELL_TIMEOUT_MS,
        )?,
        stdout_capture_bytes: bounded(
            raw.stdout_capture_bytes,
            "limits.tools.stdout_capture_bytes",
            MAX_CAPTURE_BYTES,
        )?,
        stderr_capture_bytes: bounded(
            raw.stderr_capture_bytes,
            "limits.tools.stderr_capture_bytes",
            MAX_CAPTURE_BYTES,
        )?,
        inline_model_result_bytes: bounded(
            raw.inline_model_result_bytes,
            "limits.tools.inline_model_result_bytes",
            MAX_INLINE_MODEL_RESULT_BYTES,
        )?,
        per_stream_projection_bytes: bounded(
            raw.per_stream_projection_bytes,
            "limits.tools.per_stream_projection_bytes",
            MAX_PER_STREAM_PROJECTION_BYTES,
        )?,
    };
    if limits.read_file_default_bytes > limits.read_file_max_bytes {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.tools.read_file_default_bytes",
            upper: "limits.tools.read_file_max_bytes",
        });
    }
    if limits.run_shell_default_timeout_ms > limits.run_shell_max_timeout_ms {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.tools.run_shell_default_timeout_ms",
            upper: "limits.tools.run_shell_max_timeout_ms",
        });
    }
    if limits.per_stream_projection_bytes > limits.stdout_capture_bytes {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.tools.per_stream_projection_bytes",
            upper: "limits.tools.stdout_capture_bytes",
        });
    }
    if limits.per_stream_projection_bytes > limits.stderr_capture_bytes {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.tools.per_stream_projection_bytes",
            upper: "limits.tools.stderr_capture_bytes",
        });
    }
    if limits.per_stream_projection_bytes.saturating_mul(2) > limits.inline_model_result_bytes {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "combined per-stream projections",
            upper: "limits.tools.inline_model_result_bytes",
        });
    }
    Ok(limits)
}

fn validate_protocol_limits(raw: RawProtocolLimits) -> Result<ProtocolLimits, ConfigError> {
    Ok(ProtocolLimits {
        websocket_durable_payload_bytes: bounded(
            raw.websocket_durable_payload_bytes,
            "limits.protocol.websocket_durable_payload_bytes",
            MAX_WEBSOCKET_DURABLE_PAYLOAD_BYTES,
        )?,
        user_text_message_bytes: bounded(
            raw.user_text_message_bytes,
            "limits.protocol.user_text_message_bytes",
            MAX_USER_TEXT_MESSAGE_BYTES,
        )?,
    })
}

fn validate_cross_field_limits(config: &ConfigData) -> Result<(), ConfigError> {
    if config.limits.agent.max_model_attempts_per_work
        < config.limits.agent.max_model_steps_per_work
    {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.agent.max_model_steps_per_work",
            upper: "limits.agent.max_model_attempts_per_work",
        });
    }
    if config.limits.agent.max_model_attempts_per_work
        < config.model_gateway.max_attempts_per_invocation
    {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "model_gateway.max_attempts_per_invocation",
            upper: "limits.agent.max_model_attempts_per_work",
        });
    }
    if config.limits.agent.max_work_item_duration_ms < config.model_gateway.invocation_timeout_ms {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "model_gateway.invocation_timeout_ms",
            upper: "limits.agent.max_work_item_duration_ms",
        });
    }
    if config.limits.agent.max_work_item_duration_ms < config.limits.tools.run_shell_max_timeout_ms
    {
        return Err(ConfigError::CrossFieldLimitInversion {
            lower: "limits.tools.run_shell_max_timeout_ms",
            upper: "limits.agent.max_work_item_duration_ms",
        });
    }
    Ok(())
}

fn validate_shell(raw: RawShell) -> Result<ShellConfig, ConfigError> {
    if raw.executable != "/bin/bash" {
        return Err(ConfigError::InvalidShell {
            reason: "V0 executable must be /bin/bash",
        });
    }
    let environment_policy = match raw.environment_policy.as_str() {
        "clean" => ShellEnvironmentPolicy::Clean,
        _ => {
            return Err(ConfigError::InvalidShell {
                reason: "V0 environment_policy must be clean",
            });
        }
    };
    if !raw.inherited_variables.is_empty() {
        return Err(ConfigError::InvalidShell {
            reason: "V0 inherited_variables must be empty",
        });
    }
    let delegated_cgroup_root = raw.delegated_cgroup_root.map(PathBuf::from);
    if delegated_cgroup_root.as_deref().is_some_and(|root| {
        !root.is_absolute()
            || root == Path::new("/")
            || root == Path::new("/sys/fs/cgroup")
            || !root.starts_with("/sys/fs/cgroup/")
            || root
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    }) {
        return Err(ConfigError::InvalidShell {
            reason: "delegated_cgroup_root must be an absolute child of /sys/fs/cgroup",
        });
    }
    Ok(ShellConfig {
        executable: PathBuf::from(raw.executable),
        environment_policy,
        inherited_variables: raw.inherited_variables,
        administrative_enabled: raw.administrative_enabled,
        delegated_cgroup_root,
    })
}

fn validate_device_auth(raw: RawDeviceAuth) -> Result<DeviceAuthConfig, ConfigError> {
    if raw.source != "provisioned_sqlite" {
        return Err(ConfigError::InvalidDeviceAuthSource);
    }
    Ok(DeviceAuthConfig {
        source: DeviceAuthSource::ProvisionedSqlite,
    })
}

fn validate_tracing(raw: RawTracing) -> Result<TracingConfig, ConfigError> {
    let format = match raw.format.as_str() {
        "pretty" => TracingFormat::Pretty,
        "json" => TracingFormat::Json,
        _ => return Err(ConfigError::InvalidTracingValue { field: "format" }),
    };
    let filter = match raw.filter.as_str() {
        "trace" => TracingFilter::Trace,
        "debug" => TracingFilter::Debug,
        "info" => TracingFilter::Info,
        "warn" => TracingFilter::Warn,
        "error" => TracingFilter::Error,
        _ => return Err(ConfigError::InvalidTracingValue { field: "filter" }),
    };
    Ok(TracingConfig { format, filter })
}

fn validate_shutdown(raw: RawShutdown) -> Result<ShutdownConfig, ConfigError> {
    if raw.grace_period_ms <= 0 {
        return Err(ConfigError::InvalidShutdownDuration);
    }
    Ok(ShutdownConfig {
        grace_period_ms: raw.grace_period_ms as u64,
    })
}

fn bounded(raw: i64, field: &'static str, maximum: u64) -> Result<u64, ConfigError> {
    if raw < 1 || raw as u64 > maximum {
        return Err(ConfigError::OutOfBounds {
            field,
            minimum: 1,
            maximum,
        });
    }
    Ok(raw as u64)
}

fn validate_logical_name(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if !is_logical_name(value) {
        return Err(ConfigError::InvalidLogicalName { field });
    }
    Ok(())
}

fn is_logical_name(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}
