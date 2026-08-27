# Stage 3.3 Normalized Error Contract Decision

## Date

2026-08-27

## Status

Accepted

## Context / problem

The Stage 3.3 audit found two durable ambiguities that blocked implementation:

- `SourceStatus` had no exact durable representation.
- `DomainValidationError` had no frozen contextual projection category.

Implementing the normalized error model safely also required durable choices for
`ErrorCode` extensibility, the `SafeMessage` policy, the `InternalDetail` redaction
boundary, and the normalized-error source-retention policy.

## Options considered

### SourceStatus options

- A generic arbitrary provider or operating-system string.
- A giant future-proof status enum.
- A narrow structured numeric V0 model.

### DomainValidationError options

- A blanket `From<DomainValidationError>` conversion.
- One fixed category in every context.
- An explicit contextual mapper.

### ErrorCode options

- A closed exhaustive enum.
- An arbitrary validated string.
- An allowlisted opaque stable-code value.

### Safe diagnostics

- Retain raw source and library errors.
- Sanitize only at serialization.
- Drop raw sources at the normalization boundary and retain only safe structured
  status plus trace-safe closed diagnostics.

## Decision

### SourceStatus V0

- `provider_http { code: 100..=599 }`
- `os_errno { code: 1..=i32::MAX }`
- These variants are the exact safe structured numeric representation.
- They contain no response body, reason, path, or operating-system message.
- A public Craxii HTTP status is not a `SourceStatus`.

### DomainValidationError

- `DomainValidationError` remains a precise local validation error.
- It has no blanket `From` conversion into `NormalizedError`.
- The client-boundary mapper produces:
  - category `client_protocol_error`;
  - code `domain_validation`;
  - retryability `never`;
  - certainty `definite`; and
  - safe message `The supplied value is invalid.`
- Other boundaries may classify the same validation error differently.

### ErrorCode

- `ErrorCode` is an opaque allowlisted stable-code value.
- Its current allowlist is the Stage 3 generic vocabulary.
- Later owning stages add explicit codes.
- Arbitrary adapter or user strings cannot become error codes.

### SafeMessage

- `SafeMessage` permits fixed allowlisted static text only.

### InternalDetail

- `InternalDetail` contains trace-only closed sanitized diagnostics.
- It has no `Display`, `Debug`, Serde, or source exposure.
- It is excluded from semantic equality.
- It contains no raw path, content, provider, SQL, command, output, token, or
  backtrace material.

### NormalizedError

- `NormalizedError` retains no raw adapter or library source.
- Its `source()` returns `None`.
- Its `Display`, `Debug`, and Serde surfaces expose only safe fields.
- Certainty and retryability are explicit classifications; they are not inferred
  from error strings.

## Rationale

The decision prevents accidental secret, content, path, and provider leakage while
keeping normalized errors dependency-neutral. It ties certainty to side-effect
boundaries rather than library wording, avoids premature design for future provider
statuses, preserves the contextual meaning of validation failures, and permits later
leaf codes without imposing a closed exhaustive enum contract.

## Consequences / tradeoffs

- `NormalizedError` itself carries less raw diagnostic detail.
- Adapters must classify and redact once at their boundary.
- New `SourceStatus` variants require explicit durable-contract changes.
- New error codes require explicit allowlist additions.
- Richer debugging belongs in tracing and observability, not in serialized
  normalized errors.

## Rollback / change path

- `SourceStatus` can be versioned or deliberately extended later.
- Later code constants can be added without changing the basic `ErrorCode`
  representation.
- Contextual mapping functions can be added per boundary.
- Changing existing serialized literals or shapes is a compatibility-sensitive
  architecture change.

## Scope

This decision does not define:

- the HTTP error envelope;
- SQLx or storage mappings;
- provider-native textual codes;
- workstation leaf codes;
- retry loops;
- recovery policy; or
- client localization and rendering.

## References

- [`docs/craxii-v0.0.01-architecture.md`](../craxii-v0.0.01-architecture.md)
- [`docs/craxii-v0.0.01-implementation-plan.md`](../craxii-v0.0.01-implementation-plan.md)
- [`backend/src/domain/error.rs`](../../backend/src/domain/error.rs)
