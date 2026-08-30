use std::error::Error as _;
use std::path::{Path, PathBuf};

use craxii_server::bootstrap::compatibility::{
    ARCHITECTURE_VERSION, CONFIGURATION_VERSION, MAX_SUPPORTED_SCHEMA_VERSION, PROTOCOL_VERSION,
};
use craxii_server::bootstrap::config::{
    self, ConfigError, DeviceAuthSource, FailpointMode, ShellEnvironmentPolicy, TracingFilter,
    TracingFormat, WorkstationIdentitySource,
};
use craxii_server::bootstrap::credential::CredentialSourceConfig;

const LOCAL: &str = include_str!("fixtures/config/valid/local.toml");
const EC2_SHAPE: &str = include_str!("fixtures/config/valid/ec2-shape.toml");
type ErrorPredicate = fn(&ConfigError) -> bool;

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/config")
        .join(relative)
}

fn valid(input: &str) -> config::ValidatedConfig {
    match config::parse(input) {
        Ok(config) => config,
        Err(error) => panic!("expected valid configuration, got {error}"),
    }
}

fn invalid(input: &str) -> ConfigError {
    match config::parse(input) {
        Ok(_) => panic!("expected invalid configuration"),
        Err(error) => error,
    }
}

fn replace_once(input: &str, old: &str, new: &str) -> String {
    assert!(
        input.contains(old),
        "test mutation source was absent: {old}"
    );
    input.replacen(old, new, 1)
}

fn reverse_model_targets(input: &str) -> String {
    let first = input.find("[[models.targets]]").expect("first target");
    let second = input[first + 1..]
        .find("[[models.targets]]")
        .map(|index| first + 1 + index)
        .expect("second target");
    let end = input[second..]
        .find("[model_gateway]")
        .map(|index| second + index)
        .expect("model gateway section");
    format!(
        "{}{}{}{}",
        &input[..first],
        &input[second..end],
        &input[first..second],
        &input[end..]
    )
}

#[test]
fn compatibility_constants_are_exact() {
    assert_eq!(ARCHITECTURE_VERSION, "V0.0.01");
    assert_eq!(PROTOCOL_VERSION, 1);
    assert_eq!(CONFIGURATION_VERSION, 1);
    assert_eq!(MAX_SUPPORTED_SCHEMA_VERSION, 3);
}

#[test]
fn local_fixture_loads_with_normalized_values_and_defaults() {
    let config = match config::load(fixture_path("valid/local.toml")) {
        Ok(config) => config,
        Err(error) => panic!("local fixture must validate: {error}"),
    };

    assert_eq!(config.configuration_version(), 1);
    assert_eq!(config.failpoint_mode(), FailpointMode::Disabled);
    assert_eq!(config.server().bind_address().to_string(), "127.0.0.1:8080");
    assert_eq!(
        config.server().public_base_url().as_str(),
        "http://127.0.0.1:8080/"
    );
    assert_eq!(
        config.paths().state_root(),
        Path::new("/tmp/craxii-dev/state")
    );
    assert_eq!(
        config.paths().artifact_root(),
        Path::new("/tmp/craxii-dev/state/artifacts")
    );
    assert_eq!(
        config.paths().primary_workspace_root(),
        Path::new("/tmp/craxii-dev/workspaces/primary")
    );
    assert_eq!(config.sqlite().pool_connections(), 4);
    assert_eq!(config.sqlite().busy_timeout_ms(), 5_000);
    assert_eq!(config.sqlite().wal_autocheckpoint_pages(), 1_000);
    assert_eq!(
        config.workstation().identity_source(),
        WorkstationIdentitySource::StateStore
    );
    assert_eq!(config.workstation().initial_generation(), 1);
    assert_eq!(
        config.workstation().primary_workspace_logical_name(),
        "primary"
    );
    assert!(matches!(
        config.credentials().source(),
        CredentialSourceConfig::LocalDirectory { directory }
            if directory == Path::new("/tmp/craxii-dev/credentials")
    ));
    assert_eq!(config.models().default_target(), "primary");
    assert_eq!(config.models().targets().len(), 2);
    assert_eq!(config.models().targets()[0].id(), "primary");
    assert_eq!(config.models().targets()[1].id(), "secondary");
    assert_eq!(
        config.shell().environment_policy(),
        ShellEnvironmentPolicy::Clean
    );
    assert!(config.shell().inherited_variables().is_empty());
    assert!(!config.shell().administrative_enabled());
    assert_eq!(
        config.device_auth().source(),
        DeviceAuthSource::ProvisionedSqlite
    );
    assert_eq!(config.tracing().format(), TracingFormat::Pretty);
    assert_eq!(config.tracing().filter(), TracingFilter::Info);
}

#[test]
fn ec2_shape_fixture_loads_but_remains_loopback_and_non_secret() {
    let config = match config::load(fixture_path("valid/ec2-shape.toml")) {
        Ok(config) => config,
        Err(error) => panic!("EC2-shaped fixture must validate: {error}"),
    };

    assert!(config.server().bind_address().ip().is_loopback());
    assert_eq!(
        config.server().public_base_url().as_str(),
        "https://craxii.example.invalid/"
    );
    assert_eq!(config.paths().state_root(), Path::new("/var/lib/craxii"));
    assert_eq!(
        config.paths().primary_workspace_root(),
        Path::new("/srv/craxii/workspaces/primary")
    );
    assert!(matches!(
        config.credentials().source(),
        CredentialSourceConfig::Systemd
    ));
    assert!(config.shell().administrative_enabled());
    assert_eq!(config.tracing().format(), TracingFormat::Json);
}

#[test]
fn defaults_expand_before_fingerprinting() {
    let omitted = LOCAL
        .replace("pool_connections = 4\n", "")
        .replace("busy_timeout_ms = 5000\n", "")
        .replace("wal_autocheckpoint_pages = 1000\n", "")
        .replace("initial_generation = 1\n", "")
        .replace("failpoint_mode = \"disabled\"\n", "")
        .replace("inherited_variables = []\n", "");
    let explicit = valid(LOCAL);
    let omitted = valid(&omitted);

    assert_eq!(omitted.sqlite().pool_connections(), 4);
    assert_eq!(omitted.sqlite().busy_timeout_ms(), 5_000);
    assert_eq!(omitted.sqlite().wal_autocheckpoint_pages(), 1_000);
    assert_eq!(omitted.workstation().initial_generation(), 1);
    assert_eq!(omitted.failpoint_mode(), FailpointMode::Disabled);
    assert!(omitted.shell().inherited_variables().is_empty());
    assert_eq!(omitted.fingerprint(), explicit.fingerprint());
}

#[test]
fn fingerprint_is_stable_across_toml_and_set_ordering() {
    let reordered_keys = replace_once(
        LOCAL,
        "bind_address = \"127.0.0.1:8080\"\npublic_base_url = \"http://127.0.0.1:8080\"",
        "public_base_url = \"http://127.0.0.1:8080\"\nbind_address = \"127.0.0.1:8080\"",
    );
    let reordered_declarations = reordered_keys.replace(
        "declared = [\"openai_primary\", \"openai_secondary\"]",
        "declared = [\"openai_secondary\", \"openai_primary\"]",
    );
    let reordered_targets = reverse_model_targets(&reordered_declarations);

    assert_eq!(
        valid(LOCAL).fingerprint(),
        valid(&reordered_targets).fingerprint()
    );
}

#[test]
fn lexical_normalization_precedes_fingerprinting() {
    let alternate = LOCAL
        .replace("/tmp/craxii-dev/state", "/tmp//craxii-dev/state/")
        .replace(
            "/tmp/craxii-dev/workspaces/primary",
            "/tmp//craxii-dev/workspaces/primary/",
        )
        .replace(
            "/tmp/craxii-dev/credentials",
            "/tmp//craxii-dev/credentials/",
        );
    assert_eq!(valid(LOCAL).fingerprint(), valid(&alternate).fingerprint());
}

#[test]
fn local_fixture_fingerprint_is_an_exact_stable_sha256() {
    let parsed = valid(LOCAL);
    let loaded = match config::load(fixture_path("valid/local.toml")) {
        Ok(config) => config,
        Err(error) => panic!("local fixture must load: {error}"),
    };
    assert_eq!(parsed.fingerprint(), loaded.fingerprint());
    assert_eq!(
        parsed.fingerprint().as_str(),
        "sha256:43bb265b4219927aba58b30d5faed036321feed35e3061fa3203cab9de8191c5"
    );
}

#[test]
fn required_invalid_fixtures_report_the_expected_error_classes() {
    let cases: &[(&str, ErrorPredicate)] = &[
        ("invalid/unknown-key.toml", |error| {
            matches!(error, ConfigError::TomlSyntaxOrShape)
        }),
        ("invalid/unsupported-version.toml", |error| {
            matches!(error, ConfigError::UnsupportedConfigurationVersion { .. })
        }),
        ("invalid/unsafe-bind.toml", |error| {
            matches!(error, ConfigError::UnsafeBind { .. })
        }),
        ("invalid/missing-reference.toml", |error| {
            matches!(error, ConfigError::UndeclaredCredentialRef { .. })
        }),
        ("invalid/limit-inversion.toml", |error| {
            matches!(error, ConfigError::CrossFieldLimitInversion { .. })
        }),
    ];

    for (path, predicate) in cases {
        let error = match config::load(fixture_path(path)) {
            Ok(_) => panic!("fixture {path} should fail"),
            Err(error) => error,
        };
        assert!(predicate(&error), "unexpected error for {path}: {error}");
    }
}

#[test]
fn unknown_nested_keys_and_active_failpoint_modes_are_rejected() {
    let nested = replace_once(LOCAL, "[server]\n", "[server]\nunknown_nested = true\n");
    let failpoint = replace_once(
        LOCAL,
        "failpoint_mode = \"disabled\"",
        "failpoint_mode = \"kill\"",
    );
    for input in [&nested, &failpoint] {
        assert!(matches!(invalid(input), ConfigError::TomlSyntaxOrShape));
    }
}

#[test]
fn server_rejects_unsafe_bind_and_url_forms() {
    let cases = [
        (
            "bind_address = \"127.0.0.1:8080\"",
            "bind_address = \"0.0.0.0:8080\"",
        ),
        (
            "bind_address = \"127.0.0.1:8080\"",
            "bind_address = \"127.0.0.1:0\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"http://192.0.2.10:8080\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"https://user@example.invalid/\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"https://example.invalid/?query=yes\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"https://example.invalid/#fragment\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"https://example.invalid/base\"",
        ),
        (
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"https://example.invalid/%2F\"",
        ),
    ];

    for (old, new) in cases {
        let error = invalid(&replace_once(LOCAL, old, new));
        assert!(
            matches!(
                error,
                ConfigError::UnsafeBind { .. } | ConfigError::InvalidPublicUrl { .. }
            ),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn loopback_and_public_url_edge_cases_are_explicit() {
    let ipv6 = LOCAL
        .replace(
            "bind_address = \"127.0.0.1:8080\"",
            "bind_address = \"[::1]:8080\"",
        )
        .replace(
            "public_base_url = \"http://127.0.0.1:8080\"",
            "public_base_url = \"http://[::1]:8080\"",
        );
    let ipv6 = valid(&ipv6);
    assert_eq!(ipv6.server().bind_address().to_string(), "[::1]:8080");
    assert_eq!(
        ipv6.server().public_base_url().as_str(),
        "http://[::1]:8080/"
    );

    let localhost = replace_once(
        LOCAL,
        "public_base_url = \"http://127.0.0.1:8080\"",
        "public_base_url = \"HTTP://LOCALHOST:8080\"",
    );
    assert_eq!(
        valid(&localhost).server().public_base_url().as_str(),
        "http://localhost:8080/"
    );

    let mapped = replace_once(
        LOCAL,
        "bind_address = \"127.0.0.1:8080\"",
        "bind_address = \"[::ffff:127.0.0.1]:8080\"",
    );
    assert!(matches!(invalid(&mapped), ConfigError::UnsafeBind { .. }));
}

#[test]
fn paths_reject_relative_root_traversal_and_workspace_overlap() {
    let cases = [
        (
            "state_root = \"/tmp/craxii-dev/state\"",
            "state_root = \"relative/state\"",
        ),
        (
            "state_root = \"/tmp/craxii-dev/state\"",
            "state_root = \"/\"",
        ),
        (
            "state_root = \"/tmp/craxii-dev/state\"",
            "state_root = \"/tmp/craxii-dev/../state\"",
        ),
        (
            "state_root = \"/tmp/craxii-dev/state\"",
            "state_root = \"/tmp/craxii-dev/./state\"",
        ),
        (
            "primary_workspace_root = \"/tmp/craxii-dev/workspaces/primary\"",
            "primary_workspace_root = \"/tmp/craxii-dev/state/workspace\"",
        ),
        (
            "primary_workspace_root = \"/tmp/craxii-dev/workspaces/primary\"",
            "primary_workspace_root = \"/tmp/craxii-dev\"",
        ),
        (
            "artifact_root = \"/tmp/craxii-dev/state/artifacts\"",
            "artifact_root = \"/tmp/craxii-dev/state\"",
        ),
    ];

    for (old, new) in cases {
        assert!(matches!(
            invalid(&replace_once(LOCAL, old, new)),
            ConfigError::InvalidPath { .. }
        ));
    }

    valid(LOCAL);
    let disjoint_artifacts = replace_once(
        LOCAL,
        "artifact_root = \"/tmp/craxii-dev/state/artifacts\"",
        "artifact_root = \"/tmp/craxii-dev/artifacts\"",
    );
    valid(&disjoint_artifacts);
}

#[test]
fn path_overlap_uses_components_not_string_prefixes() {
    let disjoint = LOCAL
        .replace(
            "state_root = \"/tmp/craxii-dev/state\"",
            "state_root = \"/a\"",
        )
        .replace(
            "artifact_root = \"/tmp/craxii-dev/state/artifacts\"",
            "artifact_root = \"/a/artifacts\"",
        )
        .replace(
            "primary_workspace_root = \"/tmp/craxii-dev/workspaces/primary\"",
            "primary_workspace_root = \"/a2\"",
        );
    valid(&disjoint);
}

#[test]
fn path_validation_performs_no_filesystem_mutation() {
    let sentinel_root = PathBuf::from(format!(
        "/tmp/craxii-config-validation-no-create-{}",
        std::process::id()
    ));
    assert!(!sentinel_root.exists(), "test sentinel unexpectedly exists");

    let input = LOCAL
        .replace(
            "/tmp/craxii-dev/state",
            &format!("{}/state", sentinel_root.display()),
        )
        .replace(
            "/tmp/craxii-dev/workspaces/primary",
            &format!("{}/workspaces/primary", sentinel_root.display()),
        )
        .replace(
            "/tmp/craxii-dev/credentials",
            &format!("{}/credentials", sentinel_root.display()),
        );
    valid(&input);
    assert!(!sentinel_root.exists());
}

#[test]
fn credential_declarations_and_references_are_strict() {
    let duplicate = LOCAL.replace(
        "declared = [\"openai_primary\", \"openai_secondary\"]",
        "declared = [\"openai_primary\", \"openai_primary\"]",
    );
    assert!(matches!(
        invalid(&duplicate),
        ConfigError::DuplicateCredentialDeclaration { .. }
    ));

    let bad_name = LOCAL.replace(
        "declared = [\"openai_primary\", \"openai_secondary\"]",
        "declared = [\"bad credential\", \"openai_secondary\"]",
    );
    assert!(matches!(
        invalid(&bad_name),
        ConfigError::InvalidCredentialRef { .. }
    ));

    let missing = match config::load(fixture_path("invalid/missing-reference.toml")) {
        Ok(_) => panic!("missing reference must fail"),
        Err(error) => error,
    };
    assert!(matches!(
        missing,
        ConfigError::UndeclaredCredentialRef { .. }
    ));
}

#[test]
fn credential_source_shapes_are_enforced_without_loading_secrets() {
    let local_without_directory =
        LOCAL.replace("directory = \"/tmp/craxii-dev/credentials\"\n", "");
    assert!(matches!(
        invalid(&local_without_directory),
        ConfigError::InvalidCredentialSource { .. }
    ));

    let systemd_with_directory = replace_once(
        EC2_SHAPE,
        "source = \"systemd\"\n",
        "source = \"systemd\"\ndirectory = \"/run/credentials/craxii\"\n",
    );
    assert!(matches!(
        invalid(&systemd_with_directory),
        ConfigError::InvalidCredentialSource { .. }
    ));

    let unknown_source = replace_once(
        LOCAL,
        "source = \"local_directory\"",
        "source = \"environment\"",
    );
    assert!(matches!(
        invalid(&unknown_source),
        ConfigError::InvalidCredentialSource { .. }
    ));
}

#[test]
fn model_target_identity_default_and_enabled_rules_are_enforced() {
    let duplicate = replace_once(LOCAL, "id = \"secondary\"", "id = \"primary\"");
    assert!(matches!(
        invalid(&duplicate),
        ConfigError::DuplicateModelTarget { .. }
    ));

    let missing = replace_once(
        LOCAL,
        "default_target = \"primary\"",
        "default_target = \"missing\"",
    );
    assert!(matches!(
        invalid(&missing),
        ConfigError::MissingDefaultTarget { .. }
    ));

    let disabled = replace_once(LOCAL, "enabled = true", "enabled = false");
    assert!(matches!(
        invalid(&disabled),
        ConfigError::DisabledDefaultTarget { .. }
    ));
}

#[test]
fn model_strings_endpoints_and_output_relationships_are_enforced() {
    let cases = [
        ("provider = \"openai\"", "provider = \"other\""),
        (
            "provider_model_id = \"fixture-primary-model\"",
            "provider_model_id = \"\"",
        ),
        (
            "provider_model_id = \"fixture-primary-model\"",
            "provider_model_id = \"bad\\nmodel\"",
        ),
        (
            "endpoint = \"https://api.openai.example.invalid/v1\"",
            "endpoint = \"http://api.openai.example.invalid/v1\"",
        ),
        (
            "endpoint = \"https://api.openai.example.invalid/v1\"",
            "endpoint = \"https://user@api.openai.example.invalid/v1\"",
        ),
        (
            "endpoint = \"https://api.openai.example.invalid/v1\"",
            "endpoint = \"https://api.openai.example.invalid/v1?q=1\"",
        ),
        (
            "endpoint = \"https://api.openai.example.invalid/v1\"",
            "endpoint = \"https://api.openai.example.invalid/v1#fragment\"",
        ),
        (
            "token_estimator = \"conservative_v1\"",
            "token_estimator = \"bad estimator\"",
        ),
        (
            "requested_output_tokens = 8192",
            "requested_output_tokens = 20000",
        ),
        ("max_output_tokens = 16384", "max_output_tokens = 128000"),
    ];

    for (old, new) in cases {
        let error = invalid(&replace_once(LOCAL, old, new));
        assert!(
            matches!(
                error,
                ConfigError::InvalidModelTarget { .. } | ConfigError::InvalidProviderUrl { .. }
            ),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn provider_endpoint_scheme_host_and_trailing_slash_are_normalized() {
    let input = replace_once(
        LOCAL,
        "endpoint = \"https://api.openai.example.invalid/v1\"",
        "endpoint = \"HTTPS://API.OPENAI.EXAMPLE.INVALID\"",
    );
    let config = valid(&input);
    let primary = config
        .models()
        .targets()
        .iter()
        .find(|target| target.id() == "primary")
        .expect("primary target");
    assert_eq!(
        primary.endpoint().as_str(),
        "https://api.openai.example.invalid/"
    );
}

#[test]
fn reasoning_continuation_requirement_needs_the_capability() {
    let inconsistent = replace_once(
        LOCAL,
        "structured_output = true\nreasoning_continuation = true\n\n[[models.targets]]",
        "structured_output = true\nreasoning_continuation = false\n\n[[models.targets]]",
    );
    assert!(matches!(
        invalid(&inconsistent),
        ConfigError::InvalidModelCapabilityRelationship { .. }
    ));
}

#[test]
fn exact_architecture_maxima_are_accepted() {
    let maxima = LOCAL
        .replace(
            "read_file_default_bytes = 1048576",
            "read_file_default_bytes = 8388608",
        )
        .replace(
            "run_shell_default_timeout_ms = 120000",
            "run_shell_default_timeout_ms = 900000",
        );
    valid(&maxima);
}

#[test]
fn values_above_architecture_maxima_are_rejected() {
    let cases = [
        ("pool_connections = 4", "pool_connections = 5"),
        (
            "max_attempts_per_invocation = 3",
            "max_attempts_per_invocation = 4",
        ),
        (
            "invocation_timeout_ms = 300000",
            "invocation_timeout_ms = 300001",
        ),
        (
            "response_idle_timeout_ms = 60000",
            "response_idle_timeout_ms = 60001",
        ),
        (
            "max_model_steps_per_work = 16",
            "max_model_steps_per_work = 17",
        ),
        (
            "max_model_attempts_per_work = 32",
            "max_model_attempts_per_work = 33",
        ),
        (
            "max_tool_calls_per_work = 32",
            "max_tool_calls_per_work = 33",
        ),
        (
            "max_ordered_output_items_per_response = 64",
            "max_ordered_output_items_per_response = 65",
        ),
        (
            "max_raw_tool_argument_bytes = 65536",
            "max_raw_tool_argument_bytes = 65537",
        ),
        (
            "max_work_item_duration_ms = 1800000",
            "max_work_item_duration_ms = 1800001",
        ),
        (
            "read_file_max_bytes = 8388608",
            "read_file_max_bytes = 8388609",
        ),
        (
            "run_shell_command_max_bytes = 65536",
            "run_shell_command_max_bytes = 65537",
        ),
        (
            "run_shell_max_timeout_ms = 900000",
            "run_shell_max_timeout_ms = 900001",
        ),
        (
            "stdout_capture_bytes = 8388608",
            "stdout_capture_bytes = 8388609",
        ),
        (
            "stderr_capture_bytes = 8388608",
            "stderr_capture_bytes = 8388609",
        ),
        (
            "inline_model_result_bytes = 65536",
            "inline_model_result_bytes = 65537",
        ),
        (
            "per_stream_projection_bytes = 32768",
            "per_stream_projection_bytes = 32769",
        ),
        (
            "websocket_durable_payload_bytes = 262144",
            "websocket_durable_payload_bytes = 262145",
        ),
        (
            "user_text_message_bytes = 65536",
            "user_text_message_bytes = 65537",
        ),
    ];

    for (old, new) in cases {
        let error = invalid(&replace_once(LOCAL, old, new));
        assert!(
            matches!(
                error,
                ConfigError::OutOfBounds { .. } | ConfigError::InvalidSqliteTuning { .. }
            ),
            "unexpected error for {new}: {error}"
        );
    }
}

#[test]
fn zero_is_rejected_for_nonzero_configuration_values() {
    let cases = [
        ("pool_connections = 4", "pool_connections = 0"),
        ("busy_timeout_ms = 5000", "busy_timeout_ms = 0"),
        (
            "wal_autocheckpoint_pages = 1000",
            "wal_autocheckpoint_pages = 0",
        ),
        ("initial_generation = 1", "initial_generation = 0"),
        ("config_version = 1", "config_version = 0"),
        (
            "context_window_tokens = 128000",
            "context_window_tokens = 0",
        ),
        ("max_output_tokens = 16384", "max_output_tokens = 0"),
        (
            "requested_output_tokens = 8192",
            "requested_output_tokens = 0",
        ),
        (
            "max_attempts_per_invocation = 3",
            "max_attempts_per_invocation = 0",
        ),
        (
            "invocation_timeout_ms = 300000",
            "invocation_timeout_ms = 0",
        ),
        (
            "response_idle_timeout_ms = 60000",
            "response_idle_timeout_ms = 0",
        ),
        (
            "max_model_steps_per_work = 16",
            "max_model_steps_per_work = 0",
        ),
        (
            "max_model_attempts_per_work = 32",
            "max_model_attempts_per_work = 0",
        ),
        (
            "max_tool_calls_per_work = 32",
            "max_tool_calls_per_work = 0",
        ),
        (
            "max_ordered_output_items_per_response = 64",
            "max_ordered_output_items_per_response = 0",
        ),
        (
            "max_raw_tool_argument_bytes = 65536",
            "max_raw_tool_argument_bytes = 0",
        ),
        (
            "max_work_item_duration_ms = 1800000",
            "max_work_item_duration_ms = 0",
        ),
        (
            "read_file_default_bytes = 1048576",
            "read_file_default_bytes = 0",
        ),
        ("read_file_max_bytes = 8388608", "read_file_max_bytes = 0"),
        (
            "run_shell_command_max_bytes = 65536",
            "run_shell_command_max_bytes = 0",
        ),
        (
            "run_shell_default_timeout_ms = 120000",
            "run_shell_default_timeout_ms = 0",
        ),
        (
            "run_shell_max_timeout_ms = 900000",
            "run_shell_max_timeout_ms = 0",
        ),
        ("stdout_capture_bytes = 8388608", "stdout_capture_bytes = 0"),
        ("stderr_capture_bytes = 8388608", "stderr_capture_bytes = 0"),
        (
            "inline_model_result_bytes = 65536",
            "inline_model_result_bytes = 0",
        ),
        (
            "per_stream_projection_bytes = 32768",
            "per_stream_projection_bytes = 0",
        ),
        (
            "websocket_durable_payload_bytes = 262144",
            "websocket_durable_payload_bytes = 0",
        ),
        (
            "user_text_message_bytes = 65536",
            "user_text_message_bytes = 0",
        ),
        ("grace_period_ms = 10000", "grace_period_ms = 0"),
    ];

    for (old, new) in cases {
        invalid(&replace_once(LOCAL, old, new));
    }
}

#[test]
fn cross_field_limit_inversions_are_rejected() {
    let cases = [
        LOCAL
            .replace(
                "read_file_default_bytes = 1048576",
                "read_file_default_bytes = 8388608",
            )
            .replace(
                "read_file_max_bytes = 8388608",
                "read_file_max_bytes = 1048576",
            ),
        LOCAL.replace(
            "invocation_timeout_ms = 300000",
            "invocation_timeout_ms = 30000",
        ),
        LOCAL
            .replace(
                "run_shell_default_timeout_ms = 120000",
                "run_shell_default_timeout_ms = 800000",
            )
            .replace(
                "run_shell_max_timeout_ms = 900000",
                "run_shell_max_timeout_ms = 700000",
            ),
        replace_once(
            LOCAL,
            "max_model_attempts_per_work = 32",
            "max_model_attempts_per_work = 15",
        ),
        LOCAL
            .replace(
                "max_model_steps_per_work = 16",
                "max_model_steps_per_work = 1",
            )
            .replace(
                "max_model_attempts_per_work = 32",
                "max_model_attempts_per_work = 2",
            ),
        LOCAL
            .replace(
                "stdout_capture_bytes = 8388608",
                "stdout_capture_bytes = 16384",
            )
            .replace(
                "stderr_capture_bytes = 8388608",
                "stderr_capture_bytes = 16384",
            ),
        replace_once(
            LOCAL,
            "inline_model_result_bytes = 65536",
            "inline_model_result_bytes = 32768",
        ),
    ];

    for input in cases {
        let error = invalid(&input);
        assert!(
            matches!(
                error,
                ConfigError::CrossFieldLimitInversion { .. } | ConfigError::OutOfBounds { .. }
            ),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn sqlite_fixed_tuning_and_workstation_logical_values_are_enforced() {
    let sqlite_cases = [
        ("busy_timeout_ms = 5000", "busy_timeout_ms = 4999"),
        (
            "wal_autocheckpoint_pages = 1000",
            "wal_autocheckpoint_pages = 999",
        ),
    ];
    for (old, new) in sqlite_cases {
        assert!(matches!(
            invalid(&replace_once(LOCAL, old, new)),
            ConfigError::InvalidSqliteTuning { .. }
        ));
    }

    let bad_workspace_name = replace_once(
        LOCAL,
        "primary_workspace_logical_name = \"primary\"",
        "primary_workspace_logical_name = \"bad workspace\"",
    );
    assert!(matches!(
        invalid(&bad_workspace_name),
        ConfigError::InvalidLogicalName { .. }
    ));

    let unknown_identity = replace_once(
        LOCAL,
        "identity_source = \"state_store\"",
        "identity_source = \"toml_uuid\"",
    );
    assert!(matches!(
        invalid(&unknown_identity),
        ConfigError::InvalidLogicalName { .. }
    ));
}

#[test]
fn unbounded_i64_shutdown_and_generation_values_are_accepted() {
    let large = LOCAL
        .replace(
            "initial_generation = 1",
            "initial_generation = 9223372036854775807",
        )
        .replace(
            "grace_period_ms = 10000",
            "grace_period_ms = 9223372036854775807",
        );
    let config = valid(&large);
    assert_eq!(config.workstation().initial_generation(), i64::MAX as u64);
    assert_eq!(config.shutdown().grace_period_ms(), i64::MAX as u64);
}

#[test]
fn work_duration_covers_provider_and_shell_maxima() {
    let below_provider = replace_once(
        LOCAL,
        "max_work_item_duration_ms = 1800000",
        "max_work_item_duration_ms = 299999",
    );
    assert!(matches!(
        invalid(&below_provider),
        ConfigError::CrossFieldLimitInversion { .. }
    ));

    let below_shell = LOCAL
        .replace(
            "invocation_timeout_ms = 300000",
            "invocation_timeout_ms = 100000",
        )
        .replace(
            "max_work_item_duration_ms = 1800000",
            "max_work_item_duration_ms = 800000",
        );
    assert!(matches!(
        invalid(&below_shell),
        ConfigError::CrossFieldLimitInversion { .. }
    ));
}

#[test]
fn shell_auth_tracing_and_shutdown_values_are_closed_enums_or_positive() {
    let cases = [
        ("executable = \"/bin/bash\"", "executable = \"/bin/zsh\""),
        (
            "environment_policy = \"clean\"",
            "environment_policy = \"inherit\"",
        ),
        (
            "inherited_variables = []",
            "inherited_variables = [\"PATH\"]",
        ),
        (
            "source = \"provisioned_sqlite\"",
            "source = \"raw_bearer_token\"",
        ),
        ("format = \"pretty\"", "format = \"yaml\""),
        ("filter = \"info\"", "filter = \"verbose\""),
        ("grace_period_ms = 10000", "grace_period_ms = 0"),
    ];

    for (old, new) in cases {
        invalid(&replace_once(LOCAL, old, new));
    }
}

#[test]
fn delegated_cgroup_root_is_an_absolute_normalized_child_of_cgroup_v2() {
    let configured = replace_once(
        LOCAL,
        "administrative_enabled = false",
        "administrative_enabled = false\ndelegated_cgroup_root = \"/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions\"",
    );
    assert_eq!(
        valid(&configured).shell().delegated_cgroup_root(),
        Some(std::path::Path::new(
            "/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions"
        ))
    );
    for unsafe_root in [
        "relative/craxii-executions",
        "/sys/fs/cgroup",
        "/sys/fs/cgroup/system.slice/../../tmp/craxii-executions",
    ] {
        let candidate = replace_once(
            LOCAL,
            "administrative_enabled = false",
            &format!("administrative_enabled = false\ndelegated_cgroup_root = \"{unsafe_root}\""),
        );
        assert!(matches!(
            invalid(&candidate),
            ConfigError::InvalidShell { .. }
        ));
    }
}

#[test]
fn malformed_toml_secret_like_text_is_redacted_from_display_and_debug() {
    const SENTINEL: &str = "TOML-SENTINEL-SECRET-MUST-NOT-ECHO-92f7";
    let input = replace_once(
        LOCAL,
        "configuration_version = 1\n",
        &format!("configuration_version = 1\nunexpected_secret = \"{SENTINEL}\"\n"),
    );
    let error = invalid(&input);
    assert!(!format!("{error}").contains(SENTINEL));
    assert!(!format!("{error:?}").contains(SENTINEL));
    assert!(!format!("{error:?}").contains("unexpected_secret"));
}

#[test]
fn invalid_semantic_credential_refs_are_redacted_from_all_error_surfaces() {
    const DECLARATION_SENTINEL: &str = "DECLARATION SENTINEL SECRET MUST NOT ECHO 63d1";
    const TARGET_SENTINEL: &str = "TARGET SENTINEL SECRET MUST NOT ECHO c084";
    let cases = [
        (
            LOCAL.replace(
                "declared = [\"openai_primary\", \"openai_secondary\"]",
                &format!("declared = [\"{DECLARATION_SENTINEL}\", \"openai_secondary\"]"),
            ),
            DECLARATION_SENTINEL,
            "credentials.declared",
        ),
        (
            replace_once(
                LOCAL,
                "credential = \"openai_primary\"",
                &format!("credential = \"{TARGET_SENTINEL}\""),
            ),
            TARGET_SENTINEL,
            "models.targets.credential",
        ),
    ];

    for (input, sentinel, expected_field) in cases {
        let error = invalid(&input);
        assert!(matches!(
            error,
            ConfigError::InvalidCredentialRef { field } if field == expected_field
        ));
        assert!(!format!("{error}").contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));

        let mut source = error.source();
        while let Some(cause) = source {
            assert!(!format!("{cause}").contains(sentinel));
            assert!(!format!("{cause:?}").contains(sentinel));
            source = cause.source();
        }
    }
}

#[test]
fn fingerprint_is_fixed_format_and_contains_no_raw_configuration_material() {
    let config = valid(LOCAL);
    let fingerprint = config.fingerprint().as_str();
    assert_eq!(fingerprint.len(), 71);
    assert!(fingerprint.starts_with("sha256:"));
    assert!(
        fingerprint[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(fingerprint[7..], fingerprint[7..].to_ascii_lowercase());
    for raw in [
        "fixture-primary-model",
        "openai_primary",
        "/tmp/craxii-dev/credentials",
        "configuration_version",
    ] {
        assert!(!fingerprint.contains(raw));
    }

    let renamed = LOCAL
        .replace("openai_primary", "renamed_logical_credential")
        .replace("openai_secondary", "renamed_secondary_credential");
    assert_ne!(config.fingerprint(), valid(&renamed).fingerprint());
}

#[test]
fn read_errors_are_typed_without_configuration_contents() {
    let path = fixture_path("does-not-exist.toml");
    let error = match config::load(&path) {
        Ok(_) => panic!("missing file should fail"),
        Err(error) => error,
    };
    assert!(matches!(error, ConfigError::Read { .. }));
}
