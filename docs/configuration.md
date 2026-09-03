# Configuration

The backend accepts exactly one command-line option:

```sh
craxii-server --config <path-to-config.toml>
```

Configuration is loaded from a versioned TOML file. Runtime settings are not overlaid from environment variables. Provider secrets are referenced by logical name and loaded from restricted files; secret values do not belong in configuration.

The safe, complete local example is `backend/tests/fixtures/config/valid/local.toml`. It uses only localhost, temporary development paths, fixture model identifiers, and reserved `.invalid` provider endpoints. `backend/tests/fixtures/config/valid/ec2-shape.toml` is shape evidence only and is explicitly non-deployable.

## Current sections

| Section | Implemented purpose |
| --- | --- |
| Root | `configuration_version = 1`; `failpoint_mode` must be `disabled` outside test-only builds. |
| `server` | Socket `bind_address` and absolute `public_base_url`. Plain HTTP is accepted only for loopback development; unsafe bind/base-URL combinations are rejected. |
| `paths` | Absolute state, artifact, and primary-workspace roots. The roots must obey separation and safety checks. |
| `sqlite` | Connection count, busy timeout, and WAL autocheckpoint pages. |
| `workstation` | State-store identity, initial generation, and the logical primary-workspace name. |
| `credentials` | `local_directory` with an absolute directory, or `systemd`; plus declared logical credential names. |
| `models` | Default target and one or more validated model targets. |
| `model_gateway` | Attempt count, overall invocation timeout, and response-idle timeout. |
| `limits.agent` | Work duration, model-step/attempt, tool-call, output-item, and tool-argument limits. |
| `limits.tools` | File-read, command, timeout, output-capture, inline-result, and stream-projection limits. |
| `limits.protocol` | Durable WebSocket payload and user-message limits, bounded by compiled protocol maxima. |
| `shell` | Absolute executable, clean child environment, no inherited variables, optional administrative execution, and optional delegated cgroup root. |
| `device_auth` | The implemented source is provisioned credentials stored in SQLite. |
| `tracing` | `pretty` or `json` format and a validated filter. |
| `shutdown` | Grace-period duration. |

Unknown keys and unknown enum values are rejected. Required sections cannot be omitted. Cross-field validation rejects inconsistent timeouts, size limits, model capability declarations, default-target references, path layouts, URLs, shell settings, and unsupported configuration versions.

## Model targets

The implemented runtime provider is OpenAI. A target declares an internal target ID, provider model ID, HTTPS endpoint, credential reference, conservative token estimator, context/output limits, and capability flags. The current runtime requires the capability combination exercised by the checked-in fixture: text input/output, custom tool calling, streaming, and ordered output items; structured output and reasoning continuation are currently disabled.

Use `.invalid` endpoints and fixture model identifiers in public examples. A real optional configuration must deliberately replace them with an authorized HTTPS provider endpoint and model ID.

## Credential files

For `local_directory`, the directory must not be a symlink and must not be group- or world-writable. On Unix, each credential must be a regular non-symlink file owned consistently with the directory, have one link, and deny all group/other permission bits. Credential files are bounded to 16 KiB and may not be empty or contain leading, trailing, or control characters after trailing newline removal.

A safe preparation pattern is:

```sh
mkdir -p /tmp/craxii-dev/credentials
chmod 700 /tmp/craxii-dev/credentials
install -m 600 /dev/null /tmp/craxii-dev/credentials/openai_primary
```

Enter the credential through a secure local mechanism that does not commit it or expose it in shell history. The server wraps loaded values in redacting types and does not accept provider keys directly from the TOML file.

With `source = "systemd"`, credentials are read from `/run/credentials/craxii` using the same logical references.

## Client endpoints

The native client requires an absolute endpoint without user information, query, or fragment. Release builds require HTTPS. Debug builds may explicitly allow HTTP only for `localhost`, `127.0.0.1`, or `::1`. HTTP endpoints map to `ws`, and HTTPS endpoints map to `wss`, for `/v1/events`.

The server validates the request `Host` against authorities derived from its configured bind address and public base URL. Keep the client endpoint and server public base URL consistent.
