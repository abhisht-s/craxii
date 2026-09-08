# Security model

Craxii is pre-alpha. These are implemented security boundaries, not a claim of production hardening or a substitute for deployment-specific review.

## Authentication and credentials

All `/v1` routes require a provisioned device bearer token; only liveness and readiness are unprotected. Tokens are high-entropy, write-once at provisioning, stored as verification material in SQLite, and accepted through a strict single `Authorization: Bearer` header. Revoked devices cannot authenticate.

The backend loads provider credentials from explicitly referenced, restricted files. It rejects symlinks, unsafe permissions, ownership/link inconsistencies, empty or oversized values, and malformed text. Secret wrappers redact formatting and serialization. Configuration records logical references, not credential values.

The macOS client stores a device token in Keychain. Its local persisted session state is non-secret and replaceable. Release client endpoints require HTTPS; debug HTTP is limited to explicit loopback hosts.

## Canonical server authority

The server validates identities, lifecycle, limits, authorization, and state transitions. Clients submit intent and rebuild views from bootstrap plus durable events. Command receipts do not let a client fabricate canonical messages or work state.

Message and cancellation requests carry client-generated UUIDv7 identities. The same value is required in the `Idempotency-Key` header. Repeating identical command material returns the committed result; reusing a key for different material returns a conflict.

## Workstation and tool boundary

Models do not call the operating system directly. The agent loop can request only registered tools. Inputs are schema-validated, size-bounded, subject to authority evaluation, persisted around dispatch, and executed through the workstation port.

The local workstation confines relative paths to the configured primary workspace and applies explicit file-read limits. Foreground shell commands run through a configured absolute shell with a clean child environment, no inherited variables, bounded command length, bounded time, bounded captured output, cancellation, and artifact handling.

The production Linux contract separates the trusted `craxii-server` service identity from the
model-controlled `craxii` workstation identity. A fixed root-owned launcher, executable only by the
service identity, clears supplementary groups, drops all real/effective/saved user and group IDs,
clears capabilities, sets `no_new_privs`, closes unrelated descriptors, and executes only the fixed
shell or bounded-reader operation. Model-facing file reads use the same dropped identity. The
credential-bearing systemd configuration rejects administrative execution; `craxii` must not have
sudo, Docker-socket, or trusted-service control.

The trusted backend service retains `CAP_KILL` only so TERM/KILL supervision continues to work
across the UID boundary. The launcher clears effective, permitted, inheritable, and ambient
capabilities before model-controlled code begins.

This is a same-kernel non-root boundary, not a multi-tenant sandbox or protection against host root.
The older broad administrative suite is limited to an explicitly credential-free disposable test
host and is not a production service contract.

## Durable truth and ambiguous outcomes

Canonical state and its journal facts commit transactionally. Provider attempts, tool attempts, evidence, and terminal classifications are durable. Startup recovery marks interrupted work and treats externally ambiguous provider or tool outcomes conservatively rather than blindly retrying a side effect.

Assistant drafts are deliberately ephemeral, cursorless, and lossy. They are not replayed and never replace a committed assistant message. Clients clear draft state across reconnect and converge on server-owned durable state.

## Network and observability controls

The server validates the `Host` header, applies request/body/concurrency/time limits, marks authorization as sensitive for tracing, emits bounded public errors with request IDs, and sends `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. Logs record route and status metadata rather than request bodies, authorization values, tool arguments, or model content.

Telemetry uses typed allowlisted summaries. It may include Craxii-generated identifiers, validated provider/model/tool identifiers, fixed result and error classes, numeric statuses, counts, timings, booleans, approved evidence hashes, and one-way digests of provider correlation identifiers. URLs become scheme/host-class/port/route summaries; paths become logical containment classifications; process observations contain counts and lifecycle metadata only. Arbitrary error chains are collapsed to stable safe fields.

Normal backend traces and native `os.Logger` diagnostics intentionally exclude credentials and headers, user or model content, prompts and drafts, tool arguments, shell commands, environment values, stdout/stderr and file contents, raw provider bodies/errors, URL userinfo/query/fragment, absolute paths, Keychain values, database URLs, and raw configuration. There is no conversational-content logging mode in V0.0.01.

The offline evidence commands read canonical state and artifact integrity metadata but produce deterministic, versioned, redacted, noncanonical reports. They do not expose journal payload bodies, normalized model output, tool output, environment, or content. Deeper content inspection requires direct, separately authorized local access to the sensitive SQLite/artifact stores; copy only the minimum necessary material and redact it before sharing.

The backend does not terminate production TLS itself in the documented local flow. The checked-in
Stage 27 assets prepare only the local Linux user/process/filesystem boundary and do not claim a
deployed or externally reachable service.

## Current limitations

- Pre-alpha interfaces and operational assumptions may change.
- There is no production deployment, backup/restore, multi-tenant isolation, release-signing, or notarization contract.
- The local workstation is not a complete containment boundary.
- Live provider use sends selected context and tool results to the configured provider endpoint.
- Local SQLite and artifact data may contain conversation or execution content and must be protected as sensitive application state.

Report vulnerabilities through [the private reporting process](../SECURITY.md), not a public issue.
