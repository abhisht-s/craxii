use std::fmt::{Debug, Display, Formatter};

use sha2::{Digest, Sha256};

use crate::bootstrap::credential::CredentialSourceConfig;

use super::validated::{
    ConfigData, DeviceAuthSource, FailpointMode, ModelProvider, ShellEnvironmentPolicy,
    TracingFilter, TracingFormat, WorkstationIdentitySource,
};

const FINGERPRINT_VERSION_MARKER: &[u8] = b"craxii-config-fingerprint-v1\0";

#[derive(Clone, Eq, PartialEq)]
pub struct ConfigFingerprint(String);

impl ConfigFingerprint {
    pub(super) fn from_config(config: &ConfigData) -> Self {
        let mut canonical = CanonicalBytes::new();
        canonical.u64("configuration_version", config.configuration_version);
        canonical.string(
            "failpoint_mode",
            match config.failpoint_mode {
                FailpointMode::Disabled => "disabled",
            },
        );

        canonical.string(
            "server.bind_address",
            &config.server.bind_address.to_string(),
        );
        canonical.string(
            "server.public_base_url",
            config.server.public_base_url.as_str(),
        );

        canonical.path("paths.state_root", &config.paths.state_root);
        canonical.path("paths.artifact_root", &config.paths.artifact_root);
        canonical.path(
            "paths.primary_workspace_root",
            &config.paths.primary_workspace_root,
        );

        canonical.u64("sqlite.pool_connections", config.sqlite.pool_connections);
        canonical.u64("sqlite.busy_timeout_ms", config.sqlite.busy_timeout_ms);
        canonical.u64(
            "sqlite.wal_autocheckpoint_pages",
            config.sqlite.wal_autocheckpoint_pages,
        );

        canonical.string(
            "workstation.identity_source",
            match config.workstation.identity_source {
                WorkstationIdentitySource::StateStore => "state_store",
            },
        );
        canonical.u64(
            "workstation.initial_generation",
            config.workstation.initial_generation,
        );
        canonical.string(
            "workstation.primary_workspace_logical_name",
            &config.workstation.primary_workspace_logical_name,
        );

        match &config.credentials.source {
            CredentialSourceConfig::LocalDirectory { directory } => {
                canonical.string("credentials.source", "local_directory");
                canonical.bool("credentials.has_directory", true);
                canonical.path("credentials.directory", directory);
            }
            CredentialSourceConfig::Systemd => {
                canonical.string("credentials.source", "systemd");
                canonical.bool("credentials.has_directory", false);
            }
        }
        canonical.count(
            "credentials.declared.count",
            config.credentials.declared.len(),
        );
        for credential in &config.credentials.declared {
            canonical.string("credentials.declared.item", credential.as_str());
        }

        canonical.string("models.default_target", &config.models.default_target);
        canonical.count("models.targets.count", config.models.targets.len());
        for target in &config.models.targets {
            canonical.string("models.targets.id", &target.id);
            canonical.u64("models.targets.config_version", target.config_version);
            canonical.bool("models.targets.enabled", target.enabled);
            canonical.string(
                "models.targets.provider",
                match target.provider {
                    ModelProvider::OpenAi => "openai",
                },
            );
            canonical.string(
                "models.targets.provider_model_id",
                &target.provider_model_id,
            );
            canonical.string("models.targets.endpoint", target.endpoint.as_str());
            canonical.string("models.targets.credential", target.credential.as_str());
            canonical.string("models.targets.token_estimator", &target.token_estimator);
            canonical.u64(
                "models.targets.context_window_tokens",
                target.context_window_tokens,
            );
            canonical.u64("models.targets.max_output_tokens", target.max_output_tokens);
            canonical.u64(
                "models.targets.requested_output_tokens",
                target.requested_output_tokens,
            );
            canonical.bool(
                "models.targets.reasoning_continuation",
                target.reasoning_continuation,
            );
            canonical.bool(
                "models.targets.capabilities.text_input",
                target.capabilities.text_input,
            );
            canonical.bool(
                "models.targets.capabilities.text_output",
                target.capabilities.text_output,
            );
            canonical.bool(
                "models.targets.capabilities.custom_tool_calling",
                target.capabilities.custom_tool_calling,
            );
            canonical.bool(
                "models.targets.capabilities.streaming",
                target.capabilities.streaming,
            );
            canonical.bool(
                "models.targets.capabilities.ordered_output_items",
                target.capabilities.ordered_output_items,
            );
            canonical.bool(
                "models.targets.capabilities.structured_output",
                target.capabilities.structured_output,
            );
            canonical.bool(
                "models.targets.capabilities.reasoning_continuation",
                target.capabilities.reasoning_continuation,
            );
        }

        canonical.u64(
            "model_gateway.max_attempts_per_invocation",
            config.model_gateway.max_attempts_per_invocation,
        );
        canonical.u64(
            "model_gateway.invocation_timeout_ms",
            config.model_gateway.invocation_timeout_ms,
        );
        canonical.u64(
            "model_gateway.response_idle_timeout_ms",
            config.model_gateway.response_idle_timeout_ms,
        );

        canonical.u64(
            "limits.agent.max_model_steps_per_work",
            config.limits.agent.max_model_steps_per_work,
        );
        canonical.u64(
            "limits.agent.max_model_attempts_per_work",
            config.limits.agent.max_model_attempts_per_work,
        );
        canonical.u64(
            "limits.agent.max_tool_calls_per_work",
            config.limits.agent.max_tool_calls_per_work,
        );
        canonical.u64(
            "limits.agent.max_ordered_output_items_per_response",
            config.limits.agent.max_ordered_output_items_per_response,
        );
        canonical.u64(
            "limits.agent.max_raw_tool_argument_bytes",
            config.limits.agent.max_raw_tool_argument_bytes,
        );
        canonical.u64(
            "limits.agent.max_work_item_duration_ms",
            config.limits.agent.max_work_item_duration_ms,
        );

        canonical.u64(
            "limits.tools.read_file_default_bytes",
            config.limits.tools.read_file_default_bytes,
        );
        canonical.u64(
            "limits.tools.read_file_max_bytes",
            config.limits.tools.read_file_max_bytes,
        );
        canonical.u64(
            "limits.tools.run_shell_command_max_bytes",
            config.limits.tools.run_shell_command_max_bytes,
        );
        canonical.u64(
            "limits.tools.run_shell_default_timeout_ms",
            config.limits.tools.run_shell_default_timeout_ms,
        );
        canonical.u64(
            "limits.tools.run_shell_max_timeout_ms",
            config.limits.tools.run_shell_max_timeout_ms,
        );
        canonical.u64(
            "limits.tools.stdout_capture_bytes",
            config.limits.tools.stdout_capture_bytes,
        );
        canonical.u64(
            "limits.tools.stderr_capture_bytes",
            config.limits.tools.stderr_capture_bytes,
        );
        canonical.u64(
            "limits.tools.inline_model_result_bytes",
            config.limits.tools.inline_model_result_bytes,
        );
        canonical.u64(
            "limits.tools.per_stream_projection_bytes",
            config.limits.tools.per_stream_projection_bytes,
        );

        canonical.u64(
            "limits.protocol.websocket_durable_payload_bytes",
            config.limits.protocol.websocket_durable_payload_bytes,
        );
        canonical.u64(
            "limits.protocol.user_text_message_bytes",
            config.limits.protocol.user_text_message_bytes,
        );

        canonical.path("shell.executable", &config.shell.executable);
        canonical.string(
            "shell.environment_policy",
            match config.shell.environment_policy {
                ShellEnvironmentPolicy::Clean => "clean",
            },
        );
        canonical.count(
            "shell.inherited_variables.count",
            config.shell.inherited_variables.len(),
        );
        for variable in &config.shell.inherited_variables {
            canonical.string("shell.inherited_variables.item", variable);
        }
        canonical.bool(
            "shell.administrative_enabled",
            config.shell.administrative_enabled,
        );

        canonical.string(
            "device_auth.source",
            match config.device_auth.source {
                DeviceAuthSource::ProvisionedSqlite => "provisioned_sqlite",
            },
        );
        canonical.string(
            "tracing.format",
            match config.tracing.format {
                TracingFormat::Pretty => "pretty",
                TracingFormat::Json => "json",
            },
        );
        canonical.string(
            "tracing.filter",
            match config.tracing.filter {
                TracingFilter::Trace => "trace",
                TracingFilter::Debug => "debug",
                TracingFilter::Info => "info",
                TracingFilter::Warn => "warn",
                TracingFilter::Error => "error",
            },
        );
        canonical.u64("shutdown.grace_period_ms", config.shutdown.grace_period_ms);

        let digest = Sha256::digest(canonical.finish());
        let mut value = String::with_capacity("sha256:".len() + digest.len() * 2);
        value.push_str("sha256:");
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ConfigFingerprint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Debug for ConfigFingerprint {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ConfigFingerprint")
            .field(&self.0)
            .finish()
    }
}

struct CanonicalBytes {
    bytes: Vec<u8>,
}

impl CanonicalBytes {
    fn new() -> Self {
        Self {
            bytes: FINGERPRINT_VERSION_MARKER.to_vec(),
        }
    }

    fn field(&mut self, name: &str) {
        self.variable(name.as_bytes());
    }

    fn string(&mut self, name: &str, value: &str) {
        self.field(name);
        self.variable(value.as_bytes());
    }

    fn path(&mut self, name: &str, value: &std::path::Path) {
        self.string(name, &value.to_string_lossy());
    }

    fn u64(&mut self, name: &str, value: u64) {
        self.field(name);
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn count(&mut self, name: &str, value: usize) {
        self.u64(
            name,
            u64::try_from(value).expect("configuration collection length fits u64"),
        );
    }

    fn bool(&mut self, name: &str, value: bool) {
        self.field(name);
        self.bytes.push(u8::from(value));
    }

    fn variable(&mut self, value: &[u8]) {
        let length = u64::try_from(value.len()).expect("configuration field length fits u64");
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(value);
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
