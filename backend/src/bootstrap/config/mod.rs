mod error;
mod fingerprint;
mod raw;
mod validated;

use std::fs;
use std::path::Path;

pub use error::ConfigError;
pub use fingerprint::ConfigFingerprint;
pub use validated::{
    AgentLimits, CredentialsConfig, DeviceAuthConfig, DeviceAuthSource, FailpointMode,
    LimitsConfig, ModelCapabilities, ModelGatewayConfig, ModelProvider, ModelTargetConfig,
    ModelsConfig, NormalizedUrl, PathsConfig, ProtocolLimits, ServerConfig, ShellConfig,
    ShellEnvironmentPolicy, ShutdownConfig, SqliteConfig, ToolLimits, TracingConfig, TracingFilter,
    TracingFormat, ValidatedConfig, WorkstationConfig, WorkstationIdentitySource,
};

pub fn parse(input: &str) -> Result<ValidatedConfig, ConfigError> {
    let raw =
        toml::from_str::<raw::RawConfig>(input).map_err(|_| ConfigError::TomlSyntaxOrShape)?;
    ValidatedConfig::from_raw(raw)
}

pub fn load(path: impl AsRef<Path>) -> Result<ValidatedConfig, ConfigError> {
    let path = path.as_ref();
    let input = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse(&input)
}
