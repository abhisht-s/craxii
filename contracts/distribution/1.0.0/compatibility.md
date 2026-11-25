# Compatibility and rollout

Supported worker protocol: exactly `craxii.worker` major 1, minor 0. Supported
canonicalization: `craxii.cjson/1`. Digest framing: `craxii.digest/1`. Signature
framing: `craxii.signature/1`. Bundle release: `1.0.0`. Fixture format:
`craxii.state-fixture/1`. None implies another version domain's compatibility.

Select the highest exact supported intersection, subject to explicit approved
release digests and required capabilities. Only 1.0 has a handler. A peer may
advertise 1.1 alongside 1.0; it still receives 1.0. Unsupported major/minor,
canonicalization, signature or required capability rejects before authority.
Do not silently downgrade a selected protocol. Bind selection identity,
algorithms, audience and scope into session proofs. Release eligibility uses an
explicit digest allowlist, never lexicographic version comparison.

Every core object is closed, including optional-looking fields. Unknown enum,
message or capability rejects. Only the predeclared `extensions` dictionary allows
unknown annotations: up to eight `x_` keys, primitive values, 128 UTF-8 bytes per
string and 1024 canonical bytes total. Authentication, credentials, routing,
policy, limits, provider configuration and executable controls are prohibited.
Annotations are included in digests and never affect authorization or execution.

Breaking changes include required fields, removed variants, narrower bounds,
changed identity/time/units/digest/signature/acknowledgement semantics. They need a
new major contract, handlers and explicit recovery/upgrade evidence. Minor changes
are optional negotiated capabilities with unchanged old-peer behavior. Patches
change documentation or test packaging only. Existing vectors and stored immutable
material retain their original codec and digest version.

Deploy compatible control-plane handlers first, then update workers at a safe
point or wake. Keep supported sleeping-worker handlers and stored receipt/export
bridges. Current installation/generation checks still apply to old peers. Reject
security-ineligible artifacts while preserving their state. Never downgrade a
binary onto unsupported SQLite or infer database compatibility from wire equality.
Actual released older binaries and persistence migrations require separate
integration evidence; these pure vectors do not supply that proof.
