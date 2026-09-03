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

The local workstation confines relative paths to the configured primary workspace and applies explicit file-read limits. Foreground shell commands run through a configured absolute shell with a clean child environment, no inherited variables, bounded command length, bounded time, bounded captured output, cancellation, and artifact handling. Administrative execution is separately configured and capability checked; it is off in the local fixture.

This boundary reduces accidental authority but is not a general-purpose sandbox. A user-mode shell process has the permissions of the Craxii server account, and a deliberately enabled administrative path carries greater risk. Run Craxii under a dedicated, least-privileged account and workspace when evaluating it.

## Durable truth and ambiguous outcomes

Canonical state and its journal facts commit transactionally. Provider attempts, tool attempts, evidence, and terminal classifications are durable. Startup recovery marks interrupted work and treats externally ambiguous provider or tool outcomes conservatively rather than blindly retrying a side effect.

Assistant drafts are deliberately ephemeral, cursorless, and lossy. They are not replayed and never replace a committed assistant message. Clients clear draft state across reconnect and converge on server-owned durable state.

## Network and observability controls

The server validates the `Host` header, applies request/body/concurrency/time limits, marks authorization as sensitive for tracing, emits bounded public errors with request IDs, and sends `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`. Logs record route and status metadata rather than request bodies, authorization values, tool arguments, or model content.

The backend does not terminate production TLS itself in the documented local flow. Any non-loopback deployment would require an independently reviewed TLS, host, process-isolation, persistence, credential, monitoring, and recovery design; no such deployment is claimed here.

## Current limitations

- Pre-alpha interfaces and operational assumptions may change.
- There is no production deployment, backup/restore, multi-tenant isolation, release-signing, or notarization contract.
- The local workstation is not a complete containment boundary.
- Live provider use sends selected context and tool results to the configured provider endpoint.
- Local SQLite and artifact data may contain conversation or execution content and must be protected as sensitive application state.

Report vulnerabilities through [the private reporting process](../SECURITY.md), not a public issue.
