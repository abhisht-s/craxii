use std::fmt::{Debug, Display, Formatter};
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{Dispatch, Level, Metadata};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

use crate::bootstrap::config::{TracingConfig, TracingFilter, TracingFormat};
use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::{ProcessMetadata, format_utc_timestamp};

#[derive(Clone)]
pub struct Telemetry {
    sink: Arc<dyn TelemetrySink>,
    sink_failed: Arc<AtomicBool>,
}

impl Telemetry {
    pub fn initialize_global(config: &TracingConfig) -> Result<Self, TelemetryError> {
        let sink: Arc<dyn TelemetrySink> = Arc::new(StdoutSink);
        let telemetry = Self::new(sink);
        let dispatch = build_dispatch(
            config,
            telemetry.writer(),
            matches!(config.format(), TracingFormat::Pretty) && io::stdout().is_terminal(),
        );
        tracing::dispatcher::set_global_default(dispatch)
            .map_err(|_| TelemetryError::GlobalSubscriberConflict)?;
        Ok(telemetry)
    }

    pub fn emit_startup_evidence(
        &self,
        process: &ProcessMetadata,
        health: &Health,
    ) -> Result<(), TelemetryError> {
        let build = process.build();
        let build_timestamp = build
            .reproducible_build_timestamp()
            .map(format_utc_timestamp)
            .unwrap_or_else(|| "not_supplied".to_owned());
        let build_timestamp_status = if build.reproducible_build_timestamp().is_some() {
            "provided"
        } else {
            "not_supplied"
        };
        let process_started_at = format_utc_timestamp(process.process_started_at_utc());
        let snapshot = health.snapshot();

        let span = tracing::info_span!(
            target: "craxii::bootstrap",
            "bootstrap.startup",
            subsystem = "bootstrap"
        );
        let _entered = span.enter();
        tracing::event!(
            name: "bootstrap.startup",
            target: "craxii::bootstrap",
            Level::INFO,
            event_name = "startup",
            subsystem = "bootstrap",
            package_version = build.package_version(),
            git_revision = build.git_revision(),
            git_dirty = build.is_dirty(),
            build_target = build.target_triple(),
            build_timestamp_status,
            build_timestamp,
            architecture_version = build.architecture_version(),
            protocol_version = build.protocol_version(),
            configuration_version = build.configuration_version(),
            max_supported_schema_version = build.max_supported_schema_version(),
            configuration_fingerprint = process.configuration_fingerprint(),
            process_started_at_utc = process_started_at.as_str(),
            health_state = snapshot.state().as_str(),
            health_reason = snapshot.reason().as_str(),
            live = snapshot.is_live(),
            ready = snapshot.is_ready(),
            evidence_role = "operational_only",
            recovery_truth = false,
            "Craxii startup"
        );

        self.verify_sink()
    }

    pub fn verify_sink(&self) -> Result<(), TelemetryError> {
        if self.sink.flush().is_err() {
            self.sink_failed.store(true, Ordering::Release);
        }
        if self.sink_failed.load(Ordering::Acquire) {
            Err(TelemetryError::SinkFailure)
        } else {
            Ok(())
        }
    }

    fn new(sink: Arc<dyn TelemetrySink>) -> Self {
        Self {
            sink,
            sink_failed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn writer(&self) -> SinkWriterFactory {
        SinkWriterFactory {
            sink: Arc::clone(&self.sink),
            sink_failed: Arc::clone(&self.sink_failed),
        }
    }
}

impl Debug for Telemetry {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Telemetry")
            .field("sink_failed", &self.sink_failed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryError {
    GlobalSubscriberConflict,
    SinkFailure,
}

impl Display for TelemetryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::GlobalSubscriberConflict => "telemetry global subscriber conflict",
            Self::SinkFailure => "telemetry output sink failed",
        })
    }
}

impl std::error::Error for TelemetryError {}

trait TelemetrySink: Send + Sync + 'static {
    fn write_all(&self, bytes: &[u8]) -> io::Result<()>;

    fn flush(&self) -> io::Result<()>;
}

struct StdoutSink;

impl TelemetrySink for StdoutSink {
    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        io::stdout().lock().write_all(bytes)
    }

    fn flush(&self) -> io::Result<()> {
        io::stdout().lock().flush()
    }
}

#[derive(Clone)]
struct SinkWriterFactory {
    sink: Arc<dyn TelemetrySink>,
    sink_failed: Arc<AtomicBool>,
}

impl<'writer> MakeWriter<'writer> for SinkWriterFactory {
    type Writer = SinkWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        SinkWriter {
            sink: Arc::clone(&self.sink),
            sink_failed: Arc::clone(&self.sink_failed),
        }
    }
}

struct SinkWriter {
    sink: Arc<dyn TelemetrySink>,
    sink_failed: Arc<AtomicBool>,
}

impl Write for SinkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.sink.write_all(bytes) {
            Ok(()) => Ok(bytes.len()),
            Err(error) => {
                self.sink_failed.store(true, Ordering::Release);
                Err(redacted_sink_error(error.kind()))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.sink.flush() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.sink_failed.store(true, Ordering::Release);
                Err(redacted_sink_error(error.kind()))
            }
        }
    }
}

fn build_dispatch(config: &TracingConfig, writer: SinkWriterFactory, ansi: bool) -> Dispatch {
    let configured_filter = config.filter();
    match config.format() {
        TracingFormat::Pretty => Dispatch::new(
            tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_timer(UtcTime::rfc_3339())
                    .with_ansi(ansi)
                    .with_target(false)
                    .with_file(false)
                    .with_line_number(false)
                    .with_filter(filter_fn(move |metadata| {
                        should_record(configured_filter, metadata)
                    })),
            ),
        ),
        TracingFormat::Json => Dispatch::new(
            tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_timer(UtcTime::rfc_3339())
                    .with_ansi(false)
                    .with_target(false)
                    .with_file(false)
                    .with_line_number(false)
                    .json()
                    .flatten_event(true)
                    .with_filter(filter_fn(move |metadata| {
                        should_record(configured_filter, metadata)
                    })),
            ),
        ),
    }
}

fn redacted_sink_error(kind: io::ErrorKind) -> io::Error {
    io::Error::new(kind, "redacted telemetry sink failure")
}

fn should_record(filter: TracingFilter, metadata: &Metadata<'_>) -> bool {
    match filter {
        TracingFilter::Trace => true,
        TracingFilter::Debug => *metadata.level() != Level::TRACE,
        TracingFilter::Info => {
            matches!(*metadata.level(), Level::INFO | Level::WARN | Level::ERROR)
        }
        TracingFilter::Warn => matches!(*metadata.level(), Level::WARN | Level::ERROR),
        TracingFilter::Error => *metadata.level() == Level::ERROR,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::Value;
    use time::OffsetDateTime;

    use super::*;
    use crate::bootstrap::config;
    use crate::bootstrap::metadata::BuildMetadata;
    use crate::ports::clock::TestClock;

    const REVISION: &str = "2a69e5dd8d0a4f5f923405245e1c75d07ddc73c1";
    const SECRET_SENTINEL: &str = "telemetry-secret-sentinel";

    struct MemorySink(Mutex<Vec<u8>>);

    impl MemorySink {
        fn new() -> Self {
            Self(Mutex::new(Vec::new()))
        }

        fn output(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl TelemetrySink for MemorySink {
        fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }

        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingSink;

    impl TelemetrySink for FailingSink {
        fn write_all(&self, _bytes: &[u8]) -> io::Result<()> {
            Err(io::Error::other(SECRET_SENTINEL))
        }

        fn flush(&self) -> io::Result<()> {
            Err(io::Error::other(SECRET_SENTINEL))
        }
    }

    fn tracing_config(format: &str, filter: &str) -> config::TracingConfig {
        let local = include_str!("../../tests/fixtures/config/valid/local.toml");
        let input = local
            .replace("format = \"pretty\"", &format!("format = \"{format}\""))
            .replace("filter = \"info\"", &format!("filter = \"{filter}\""));
        config::parse(&input).unwrap().tracing().clone()
    }

    fn process_metadata() -> ProcessMetadata {
        let config =
            config::parse(include_str!("../../tests/fixtures/config/valid/local.toml")).unwrap();
        let build = BuildMetadata::from_values(
            "0.0.1",
            REVISION,
            "false",
            "aarch64-apple-darwin",
            "1700000000",
        )
        .unwrap();
        let clock = TestClock::new(
            OffsetDateTime::from_unix_timestamp(1_700_000_123).unwrap(),
            Duration::ZERO,
        );
        ProcessMetadata::capture(build, config.fingerprint(), &clock).unwrap()
    }

    fn capture_startup(format: &str) -> String {
        capture_startup_with_filter(format, "info")
    }

    fn capture_startup_with_filter(format: &str, filter: &str) -> String {
        let sink = Arc::new(MemorySink::new());
        let erased_sink: Arc<dyn TelemetrySink> = sink.clone();
        let telemetry = Telemetry::new(erased_sink);
        let dispatch = build_dispatch(&tracing_config(format, filter), telemetry.writer(), false);
        tracing::dispatcher::with_default(&dispatch, || {
            telemetry
                .emit_startup_evidence(&process_metadata(), &Health::new())
                .unwrap();
        });
        sink.output()
    }

    #[test]
    fn pretty_and_json_have_semantically_identical_safe_startup_fields() {
        let pretty = capture_startup("pretty");
        let json = capture_startup("json");
        let record: Value = serde_json::from_str(json.trim()).unwrap();

        for (field, expected) in [
            ("event_name", "startup"),
            ("subsystem", "bootstrap"),
            ("package_version", "0.0.1"),
            ("git_revision", REVISION),
            ("build_target", "aarch64-apple-darwin"),
            ("build_timestamp_status", "provided"),
            ("architecture_version", "V0.0.01"),
            ("health_state", "live_unready"),
            ("health_reason", "starting"),
            ("evidence_role", "operational_only"),
        ] {
            assert_eq!(record[field], expected);
            assert!(pretty.contains(field));
            assert!(pretty.contains(expected));
        }
        assert_eq!(record["git_dirty"], false);
        assert_eq!(record["live"], true);
        assert_eq!(record["ready"], false);
        assert_eq!(record["recovery_truth"], false);
    }

    #[test]
    fn json_is_newline_delimited_and_structurally_parseable() {
        let output = capture_startup("json");
        assert!(output.ends_with('\n'));
        assert_eq!(output.lines().count(), 1);
        let record: Value = serde_json::from_str(output.trim()).unwrap();
        assert!(record["timestamp"].as_str().unwrap().ends_with('Z'));
        assert_eq!(record["level"], "INFO");
    }

    #[test]
    fn all_five_closed_filters_apply_without_environment_directives() {
        for (filter, expected) in [
            ("trace", vec!["trace", "debug", "info", "warn", "error"]),
            ("debug", vec!["debug", "info", "warn", "error"]),
            ("info", vec!["info", "warn", "error"]),
            ("warn", vec!["warn", "error"]),
            ("error", vec!["error"]),
        ] {
            let sink = Arc::new(MemorySink::new());
            let erased_sink: Arc<dyn TelemetrySink> = sink.clone();
            let telemetry = Telemetry::new(erased_sink);
            let dispatch =
                build_dispatch(&tracing_config("json", filter), telemetry.writer(), false);
            tracing::dispatcher::with_default(&dispatch, || {
                tracing::trace!(event_name = "trace");
                tracing::debug!(event_name = "debug");
                tracing::info!(event_name = "info");
                tracing::warn!(event_name = "warn");
                tracing::error!(event_name = "error");
            });
            let actual: Vec<_> = sink
                .output()
                .lines()
                .map(|line| {
                    let value: Value = serde_json::from_str(line).unwrap();
                    value["event_name"].as_str().unwrap().to_owned()
                })
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn startup_operation_succeeds_and_info_respects_all_five_filters() {
        for (filter, emitted) in [
            ("trace", true),
            ("debug", true),
            ("info", true),
            ("warn", false),
            ("error", false),
        ] {
            let output = capture_startup_with_filter("json", filter);
            if emitted {
                let record: Value = serde_json::from_str(output.trim()).unwrap();
                assert_eq!(record["event_name"], "startup");
                assert_eq!(record["level"], "INFO");
            } else {
                assert!(output.is_empty(), "{filter} emitted {output}");
            }
        }
    }

    #[test]
    fn startup_output_excludes_raw_configuration_and_secret_material() {
        for output in [capture_startup("pretty"), capture_startup("json")] {
            for forbidden in [
                SECRET_SENTINEL,
                "fixture-primary-model",
                "/tmp/craxii-dev/credentials",
                "authorization",
                "provider payload",
            ] {
                assert!(
                    !output
                        .to_ascii_lowercase()
                        .contains(&forbidden.to_ascii_lowercase())
                );
            }
        }
    }

    #[test]
    fn sink_failure_is_typed_even_when_startup_info_is_filtered() {
        for filter in ["info", "error"] {
            let sink: Arc<dyn TelemetrySink> = Arc::new(FailingSink);
            let telemetry = Telemetry::new(sink);
            let dispatch =
                build_dispatch(&tracing_config("json", filter), telemetry.writer(), false);
            let result = tracing::dispatcher::with_default(&dispatch, || {
                telemetry.emit_startup_evidence(&process_metadata(), &Health::new())
            });
            assert_eq!(result, Err(TelemetryError::SinkFailure));
            assert!(!format!("{result:?}").contains(SECRET_SENTINEL));
        }
    }

    #[test]
    fn trace_records_declare_noncanonical_operational_role() {
        let output = capture_startup("json");
        let record: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(record["evidence_role"], "operational_only");
        assert_eq!(record["recovery_truth"], false);
        assert!(record.get("journal_offset").is_none());
        assert!(record.get("journal_event").is_none());
    }

    #[test]
    fn unknown_preinstalled_global_subscriber_is_a_typed_conflict() {
        tracing::dispatcher::set_global_default(Dispatch::new(
            tracing::subscriber::NoSubscriber::default(),
        ))
        .unwrap();
        assert!(matches!(
            Telemetry::initialize_global(&tracing_config("pretty", "info")),
            Err(TelemetryError::GlobalSubscriberConflict)
        ));
    }
}
