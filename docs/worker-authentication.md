# Worker authentication

The distribution `worker_auth` module is separate from native device authentication,
Telegram, model credentials, the scheduler and customer SQLite. Authentication grants
status access; it does not grant execution or spending authority. Runtime integration
and deployed provider trust require their own accepted configuration.

`KeyStore::open` uses the fixed OS-root directory `/var/lib/craxii-worker/identity`.
Provision this directory as mode 0700 for the trusted service UID, with root-owned
nonwritable parents. The model UID must differ. Directory traversal uses held file
descriptors, no-follow opens and owner/mode checks. Key and lock files are single-link
regular mode 0600 files. Descriptors are close-on-exec. There is no signer IPC or
model-facing signing API. Same-UID execution cannot provide this boundary, and root
or kernel compromise is outside it.

Explicit first enrollment generates OS-random Ed25519 PKCS8 material. A durable
creation sentinel and exclusive lock prevent replacement after ambiguous publication
or key loss. A complete temporary file is fsynced before no-replace publication; the
parent is fsynced afterward. Restart recovers the same key. Corrupt, missing or unsafe
keys require support and central replacement with a new installation/generation.
Neither keys nor credentials belong in a machine image or customer snapshot.

The small protected identity state contains canonical nonsecret references, key ID,
highest observed epoch, binding, terminal disposition and exact pending signed enrollment context. Identity/epoch rollback is
denied. Sessions remain in memory and are discarded at restart. The client negotiates
again and proves possession to renew after sleep or expiry; it never falls back to
anonymous identity or silently enrolls another key. Transport/unavailable failures
are distinct from session expiry, stale generation and revoked authority.

Signing uses the frozen distribution preimages and exact evidence-byte hashes.
Challenges must match the protected customer/agent/installation/generation, selected
protocol, audience and approved scopes before signing. Enrollment acknowledgement
reconciliation regenerates the same deterministic proof from protected pending context and matching retained evidence; possession recovery confirms the existing
binding without a retained bootstrap secret.

HTTPS verifies the certificate chain and hostname. The client rejects plaintext,
userinfo, query/fragment configuration, redirects and proxy discovery. Requests and
responses, timeouts and idle connections are bounded. Local monotonic and wall-clock
checks can expire a session early; central time and current authority remain decisive.
Clock rollback and expired authority never extend a grant.

Isolated path/TLS constructors and the authentication-only example require debug
`test-failpoints`; release builds cannot enable that feature. Synthetic instance
identity and local certificate fixtures are tests, not provider or deployment trust.
The fixed development audience is deliberately not production configuration.
