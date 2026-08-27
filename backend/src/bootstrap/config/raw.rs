use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawConfig {
    pub(super) configuration_version: i64,
    #[serde(default)]
    pub(super) failpoint_mode: RawFailpointMode,
    pub(super) server: RawServer,
    pub(super) paths: RawPaths,
    pub(super) sqlite: RawSqlite,
    pub(super) workstation: RawWorkstation,
    pub(super) credentials: RawCredentials,
    pub(super) models: RawModels,
    pub(super) model_gateway: RawModelGateway,
    pub(super) limits: RawLimits,
    pub(super) shell: RawShell,
    pub(super) device_auth: RawDeviceAuth,
    pub(super) tracing: RawTracing,
    pub(super) shutdown: RawShutdown,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawFailpointMode {
    #[default]
    Disabled,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawServer {
    pub(super) bind_address: String,
    pub(super) public_base_url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPaths {
    pub(super) state_root: String,
    pub(super) artifact_root: String,
    pub(super) primary_workspace_root: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSqlite {
    #[serde(default = "default_pool_connections")]
    pub(super) pool_connections: i64,
    #[serde(default = "default_busy_timeout_ms")]
    pub(super) busy_timeout_ms: i64,
    #[serde(default = "default_wal_autocheckpoint_pages")]
    pub(super) wal_autocheckpoint_pages: i64,
}

const fn default_pool_connections() -> i64 {
    4
}

const fn default_busy_timeout_ms() -> i64 {
    5_000
}

const fn default_wal_autocheckpoint_pages() -> i64 {
    1_000
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawWorkstation {
    pub(super) identity_source: String,
    #[serde(default = "default_initial_generation")]
    pub(super) initial_generation: i64,
    pub(super) primary_workspace_logical_name: String,
}

const fn default_initial_generation() -> i64 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawCredentials {
    pub(super) source: String,
    pub(super) directory: Option<String>,
    #[serde(default)]
    pub(super) declared: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawModels {
    pub(super) default_target: String,
    pub(super) targets: Vec<RawModelTarget>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawModelTarget {
    pub(super) id: String,
    pub(super) config_version: i64,
    pub(super) enabled: bool,
    pub(super) provider: String,
    pub(super) provider_model_id: String,
    pub(super) endpoint: String,
    pub(super) credential: String,
    pub(super) token_estimator: String,
    pub(super) context_window_tokens: i64,
    pub(super) max_output_tokens: i64,
    pub(super) requested_output_tokens: i64,
    pub(super) reasoning_continuation: bool,
    pub(super) capabilities: RawModelCapabilities,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawModelCapabilities {
    pub(super) text_input: bool,
    pub(super) text_output: bool,
    pub(super) custom_tool_calling: bool,
    pub(super) streaming: bool,
    pub(super) ordered_output_items: bool,
    pub(super) structured_output: bool,
    pub(super) reasoning_continuation: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawModelGateway {
    pub(super) max_attempts_per_invocation: i64,
    pub(super) invocation_timeout_ms: i64,
    pub(super) response_idle_timeout_ms: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawLimits {
    pub(super) agent: RawAgentLimits,
    pub(super) tools: RawToolLimits,
    pub(super) protocol: RawProtocolLimits,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawAgentLimits {
    pub(super) max_model_steps_per_work: i64,
    pub(super) max_model_attempts_per_work: i64,
    pub(super) max_tool_calls_per_work: i64,
    pub(super) max_ordered_output_items_per_response: i64,
    pub(super) max_raw_tool_argument_bytes: i64,
    pub(super) max_work_item_duration_ms: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawToolLimits {
    pub(super) read_file_default_bytes: i64,
    pub(super) read_file_max_bytes: i64,
    pub(super) run_shell_command_max_bytes: i64,
    pub(super) run_shell_default_timeout_ms: i64,
    pub(super) run_shell_max_timeout_ms: i64,
    pub(super) stdout_capture_bytes: i64,
    pub(super) stderr_capture_bytes: i64,
    pub(super) inline_model_result_bytes: i64,
    pub(super) per_stream_projection_bytes: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProtocolLimits {
    pub(super) websocket_durable_payload_bytes: i64,
    pub(super) user_text_message_bytes: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawShell {
    pub(super) executable: String,
    pub(super) environment_policy: String,
    #[serde(default)]
    pub(super) inherited_variables: Vec<String>,
    pub(super) administrative_enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDeviceAuth {
    pub(super) source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTracing {
    pub(super) format: String,
    pub(super) filter: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawShutdown {
    pub(super) grace_period_ms: i64,
}
