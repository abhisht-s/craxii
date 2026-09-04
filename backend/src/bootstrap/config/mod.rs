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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage23_configuration_debug_and_errors_exclude_urls_paths_and_source_text() {
        let input = include_str!("../../../tests/fixtures/config/valid/local.toml")
            .replace("api.openai.example.invalid", "SENTINEL_URL_HOST_23.invalid")
            .replace("/tmp/craxii-dev", "/Users/SENTINEL_ABSOLUTE_PATH_23");
        let config = parse(&input).unwrap();
        let rendered = format!(
            "{:?}{:?}{:?}{:?}{:?}",
            config.server(),
            config.paths(),
            config.credentials(),
            config.models(),
            config.shell(),
        );
        for sentinel in ["SENTINEL_URL_HOST_23", "SENTINEL_ABSOLUTE_PATH_23"] {
            assert!(
                !rendered.contains(sentinel),
                "leaked {sentinel}: {rendered}"
            );
        }

        let missing = Path::new("/Users/SENTINEL_CONFIG_PATH_23/missing.toml");
        let error = match load(missing) {
            Ok(_) => panic!("missing configuration unexpectedly loaded"),
            Err(error) => error,
        };
        assert!(!format!("{error:?}").contains("SENTINEL_CONFIG_PATH_23"));
        assert!(!error.to_string().contains("SENTINEL_CONFIG_PATH_23"));
        assert!(std::error::Error::source(&error).is_none());
    }
}
