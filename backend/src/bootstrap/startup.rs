use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Display, Formatter};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::adapters::system_clock::SystemClock;
use crate::adapters::telemetry::{Telemetry, TelemetryError};
use crate::application::ApplicationShell;
use crate::bootstrap::config;
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{BuildMetadata, ProcessMetadata};

pub fn run_from_env() -> Result<ApplicationShell, StartupError> {
    run(std::env::args_os())
}

pub fn run(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ApplicationShell, StartupError> {
    let cli = Cli::parse(arguments)?;
    let config = config::load(&cli.config_path).map_err(|_| StartupError::Configuration)?;
    let clock = SystemClock::new();
    let build = BuildMetadata::embedded().map_err(|_| StartupError::BuildMetadata)?;
    let process = ProcessMetadata::capture(build, config.fingerprint(), &clock)
        .map_err(|_| StartupError::Clock)?;
    let health = Health::new();
    let telemetry =
        Telemetry::initialize_global(config.tracing()).map_err(StartupError::Telemetry)?;
    telemetry
        .emit_startup_evidence(&process, &health)
        .map_err(StartupError::Telemetry)?;

    Ok(ApplicationShell::new(process, health))
}

pub fn write_fatal_diagnostic(writer: &mut impl Write, error: &StartupError) -> io::Result<()> {
    writeln!(writer, "craxii fatal: {}", error.code())
}

struct Cli {
    config_path: PathBuf,
}

impl Cli {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, StartupError> {
        let mut arguments = arguments.into_iter();
        let _program = arguments.next();
        let option = arguments.next().ok_or(StartupError::Cli)?;
        if option != OsStr::new("--config") {
            return Err(StartupError::Cli);
        }
        let value = arguments.next().ok_or(StartupError::Cli)?;
        if value.is_empty() || arguments.next().is_some() {
            return Err(StartupError::Cli);
        }
        Ok(Self {
            config_path: PathBuf::from(value),
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum StartupError {
    Cli,
    Configuration,
    BuildMetadata,
    Clock,
    Telemetry(TelemetryError),
}

impl StartupError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cli => "invalid_cli",
            Self::Configuration => "invalid_configuration",
            Self::BuildMetadata => "invalid_build_metadata",
            Self::Clock => "clock_failure",
            Self::Telemetry(TelemetryError::GlobalSubscriberConflict) => {
                "telemetry_subscriber_conflict"
            }
            Self::Telemetry(TelemetryError::SinkFailure) => "telemetry_sink_failure",
        }
    }
}

impl Display for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Debug for StartupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for StartupError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_requires_exact_config_pair() {
        assert!(Cli::parse(["server".into(), "--config".into(), "config.toml".into()]).is_ok());
        for arguments in [
            vec!["server".into()],
            vec!["server".into(), "config.toml".into()],
            vec!["server".into(), "--config".into()],
            vec!["server".into(), "--config".into(), "".into()],
            vec![
                "server".into(),
                "--config".into(),
                "config.toml".into(),
                "extra".into(),
            ],
        ] {
            assert!(matches!(Cli::parse(arguments), Err(StartupError::Cli)));
        }
    }

    #[test]
    fn fatal_diagnostics_are_fixed_safe_codes() {
        let mut output = Vec::new();
        write_fatal_diagnostic(&mut output, &StartupError::Configuration).unwrap();
        assert_eq!(output, b"craxii fatal: invalid_configuration\n");
    }
}
